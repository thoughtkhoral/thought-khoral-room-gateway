use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{PgPool, Row};
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
    let event_id = Uuid::new_v4();
    if !crate::protocol::is_valid_room_event(event_id, &event) {
        return Err(StoreError::InvalidEvent);
    }

    let mut transaction = pool.begin().await?;

    // A transaction-scoped advisory lock serializes sequence allocation per room without
    // blocking appends to other rooms. It is released automatically on commit or rollback.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::text, 0))")
        .bind(event.room_id)
        .execute(&mut *transaction)
        .await?;

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
    .fetch_one(&mut *transaction)
    .await?;

    transaction.commit().await?;

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
