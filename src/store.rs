use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{PgPool, Postgres, Row, Transaction, postgres::PgRow};
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct NewEvent {
    pub room_id: Uuid,
    pub request_id: Uuid,
    pub event_type: String,
    pub actor_id: Uuid,
    pub actor_role: String,
    pub payload: Value,
    pub occurred_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RoomEvent {
    pub event_id: Uuid,
    pub room_id: Uuid,
    pub sequence: i64,
    pub request_id: Uuid,
    pub event_type: String,
    pub actor_id: Uuid,
    pub actor_role: String,
    pub payload: Value,
    pub occurred_at: DateTime<Utc>,
}

impl RoomEvent {
    pub fn to_wire_value(&self) -> Value {
        serde_json::json!({
            "contractVersion": "n2n.room.v1",
            "requestId": self.request_id,
            "roomId": self.room_id,
            "occurredAt": self.occurred_at,
            "sequence": self.sequence,
            "eventId": self.event_id,
            "eventType": self.event_type,
            "actor": { "id": self.actor_id, "role": self.actor_role },
            "payload": self.payload,
        })
    }
}

#[derive(Debug)]
pub enum StoreError {
    InvalidEvent,
    Database(sqlx::Error),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidEvent => formatter.write_str("invalid room event"),
            Self::Database(error) => write!(formatter, "database error: {error}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<sqlx::Error> for StoreError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

/// Atomically allocates the next sequence for a room and inserts one immutable event.
pub async fn append_event(pool: &PgPool, event: NewEvent) -> Result<RoomEvent, StoreError> {
    let mut transaction = pool.begin().await?;
    lock_room(&mut transaction, event.room_id).await?;
    let persisted = append_event_in_transaction(&mut transaction, event).await?;
    transaction.commit().await?;
    Ok(persisted)
}

pub(crate) async fn lock_room(
    transaction: &mut Transaction<'_, Postgres>,
    room_id: Uuid,
) -> Result<(), StoreError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::text, 0))")
        .bind(room_id)
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

pub(crate) async fn append_event_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    event: NewEvent,
) -> Result<RoomEvent, StoreError> {
    let event_id = Uuid::new_v4();
    if !crate::protocol::is_valid_room_event(event_id, &event) {
        return Err(StoreError::InvalidEvent);
    }
    let row = sqlx::query(
        r#"
        INSERT INTO room_events (
            event_id, room_id, sequence, request_id, event_type,
            actor_id, actor_role, payload, occurred_at
        )
        SELECT $1, $2, COALESCE(MAX(sequence), 0) + 1, $3, $4, $5, $6, $7, $8
        FROM room_events
        WHERE room_id = $2
        RETURNING event_id, room_id, sequence, request_id, event_type,
                  actor_id, actor_role, payload, occurred_at
        "#,
    )
    .bind(event_id)
    .bind(event.room_id)
    .bind(event.request_id)
    .bind(event.event_type)
    .bind(event.actor_id)
    .bind(event.actor_role)
    .bind(event.payload)
    .bind(event.occurred_at)
    .fetch_one(&mut **transaction)
    .await?;
    room_event_from_row(row)
}

pub async fn events_after(
    pool: &PgPool,
    room_id: Uuid,
    after_sequence: i64,
) -> Result<Vec<RoomEvent>, StoreError> {
    let rows = sqlx::query(
        r#"
        SELECT event_id, room_id, sequence, request_id, event_type,
               actor_id, actor_role, payload, occurred_at
        FROM room_events
        WHERE room_id = $1 AND sequence > $2
        ORDER BY sequence ASC
        "#,
    )
    .bind(room_id)
    .bind(after_sequence)
    .fetch_all(pool)
    .await?;
    rows.into_iter().map(room_event_from_row).collect()
}

pub(crate) async fn prior_request(
    transaction: &mut Transaction<'_, Postgres>,
    room_id: Uuid,
    request_id: Uuid,
) -> Result<Option<(Value, Vec<RoomEvent>)>, StoreError> {
    let record = sqlx::query(
        "SELECT request_fingerprint, event_ids FROM room_requests WHERE room_id = $1 AND request_id = $2",
    )
    .bind(room_id)
    .bind(request_id)
    .fetch_optional(&mut **transaction)
    .await?;
    let Some(record) = record else {
        return Ok(None);
    };
    let fingerprint: Value = record.try_get("request_fingerprint")?;
    let event_ids: Vec<Uuid> = record.try_get("event_ids")?;
    let rows = sqlx::query(
        r#"
        SELECT event_id, room_id, sequence, request_id, event_type,
               actor_id, actor_role, payload, occurred_at
        FROM room_events
        WHERE event_id = ANY($1)
        ORDER BY sequence ASC
        "#,
    )
    .bind(event_ids)
    .fetch_all(&mut **transaction)
    .await?;
    let events = rows
        .into_iter()
        .map(room_event_from_row)
        .collect::<Result<_, _>>()?;
    Ok(Some((fingerprint, events)))
}

pub(crate) async fn record_request(
    transaction: &mut Transaction<'_, Postgres>,
    room_id: Uuid,
    request_id: Uuid,
    fingerprint: Value,
    events: &[RoomEvent],
) -> Result<(), StoreError> {
    let event_ids = events
        .iter()
        .map(|event| event.event_id)
        .collect::<Vec<_>>();
    sqlx::query(
        "INSERT INTO room_requests (room_id, request_id, request_fingerprint, event_ids) VALUES ($1, $2, $3, $4)",
    )
    .bind(room_id)
    .bind(request_id)
    .bind(fingerprint)
    .bind(event_ids)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

fn room_event_from_row(row: PgRow) -> Result<RoomEvent, StoreError> {
    Ok(RoomEvent {
        event_id: row.try_get("event_id")?,
        room_id: row.try_get("room_id")?,
        sequence: row.try_get("sequence")?,
        request_id: row.try_get("request_id")?,
        event_type: row.try_get("event_type")?,
        actor_id: row.try_get("actor_id")?,
        actor_role: row.try_get("actor_role")?,
        payload: row.try_get("payload")?,
        occurred_at: row.try_get("occurred_at")?,
    })
}
