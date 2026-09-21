use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, Row, Transaction, postgres::PgRow};
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct NewEvent {
    pub room_id: Uuid,
    pub request_id: Uuid,
    pub event_type: String,
    pub actor_id: Uuid,
    pub actor_role: String,
    pub actor_display_name: Option<String>,
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
    pub actor_display_name: Option<String>,
    pub payload: Value,
    pub occurred_at: DateTime<Utc>,
}

pub const AGENT_TASK_LEASE_DURATION: chrono::Duration = chrono::Duration::minutes(5);

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AgentSkillId {
    SummarizeContext,
    ExtractActionItems,
}

impl AgentSkillId {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SummarizeContext => "summarize-context",
            Self::ExtractActionItems => "extract-action-items",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentTaskState {
    Queued,
    Running,
    AwaitingExternalInput,
    Succeeded,
    Failed,
}

impl AgentTaskState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::AwaitingExternalInput => "awaiting_external_input",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
        }
    }

    fn parse(value: &str) -> Result<Self, StoreError> {
        match value {
            "queued" => Ok(Self::Queued),
            "running" => Ok(Self::Running),
            "awaiting_external_input" => Ok(Self::AwaitingExternalInput),
            "succeeded" => Ok(Self::Succeeded),
            "failed" => Ok(Self::Failed),
            _ => Err(StoreError::InvalidAgentTask),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentTaskStart {
    pub id: String,
    pub contract_version: String,
    pub request_id: Uuid,
    pub room_id: Uuid,
    pub occurred_at: DateTime<Utc>,
    pub agent_id: Uuid,
    pub skill_id: AgentSkillId,
    pub input: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentTaskRecord {
    pub task_id: Uuid,
    pub room_id: Uuid,
    pub request_id: Uuid,
    pub requester_id: Uuid,
    pub agent_id: Uuid,
    pub skill_id: AgentSkillId,
    pub input: String,
    pub context_revision: i64,
    pub state: AgentTaskState,
    pub lease_owner: Option<Uuid>,
    pub lease_expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentTaskLease {
    pub task_id: Uuid,
    pub owner_id: Uuid,
    pub expires_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentTaskContext {
    pub task: AgentTaskRecord,
    pub events: Vec<RoomEvent>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentTaskUpdate {
    pub update_id: Uuid,
    pub event_type: String,
    pub payload: Value,
    pub occurred_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentTaskStartResult {
    pub task_id: Uuid,
    pub events: Vec<RoomEvent>,
}

/// The internal broker needs to distinguish an idempotent retry from new persisted output so it
/// never publishes a replay to room subscribers.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AgentTaskUpdateResult {
    pub task_id: Uuid,
    pub events: Vec<RoomEvent>,
    pub duplicate: bool,
}

#[derive(Clone)]
pub struct AgentTaskStore {
    pool: PgPool,
    requester_id: Uuid,
    requester_display_name: String,
}

impl AgentTaskStore {
    pub fn new(
        pool: PgPool,
        requester_id: Uuid,
        requester_display_name: impl Into<String>,
    ) -> Self {
        Self {
            pool,
            requester_id,
            requester_display_name: requester_display_name.into(),
        }
    }

    pub async fn start_agent_task(
        &self,
        start: AgentTaskStart,
    ) -> Result<AgentTaskStartResult, StoreError> {
        let mut transaction = self.pool.begin().await?;
        let result = start_agent_task_in_transaction(
            &mut transaction,
            self.requester_id,
            &self.requester_display_name,
            start,
            true,
        )
        .await?;
        transaction.commit().await?;
        Ok(result)
    }

    pub async fn agent_task(&self, task_id: Uuid) -> Result<AgentTaskRecord, StoreError> {
        agent_task(&self.pool, task_id).await
    }

    pub async fn claim_agent_task(
        &self,
        task_id: Uuid,
        owner_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<Option<AgentTaskLease>, StoreError> {
        claim_agent_task(&self.pool, task_id, owner_id, now).await
    }

    pub async fn context_for_lease(
        &self,
        lease: &AgentTaskLease,
    ) -> Result<Option<AgentTaskContext>, StoreError> {
        context_for_lease(&self.pool, lease).await
    }

    pub async fn record_agent_task_update(
        &self,
        lease: &AgentTaskLease,
        update: AgentTaskUpdate,
    ) -> Result<AgentTaskStartResult, StoreError> {
        record_agent_task_update(&self.pool, lease, update).await
    }

    pub async fn room_event_count(&self, room_id: Uuid) -> Result<i64, StoreError> {
        Ok(
            sqlx::query_scalar("SELECT COUNT(*) FROM room_events WHERE room_id = $1")
                .bind(room_id)
                .fetch_one(&self.pool)
                .await?,
        )
    }
}

impl RoomEvent {
    pub fn to_wire_value(&self) -> Value {
        let mut actor = serde_json::json!({
            "id": self.actor_id,
            "role": self.actor_role,
        });
        if let Some(display_name) = &self.actor_display_name {
            actor["displayName"] = serde_json::json!(display_name);
        }
        serde_json::json!({
            "contractVersion": "n2n.room.v1",
            "requestId": self.request_id,
            "roomId": self.room_id,
            "occurredAt": self.occurred_at,
            "sequence": self.sequence,
            "eventId": self.event_id,
            "eventType": self.event_type,
            "actor": actor,
            "payload": self.payload,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{RoomEvent, claim_oldest_queued_agent_task};
    use chrono::{Duration, TimeZone, Utc};
    use serde_json::json;
    use sqlx::postgres::PgPoolOptions;
    use tokio::time::timeout;
    use uuid::Uuid;

    #[test]
    fn wire_event_includes_a_trusted_display_name_when_available() {
        let event = RoomEvent {
            event_id: Uuid::new_v4(),
            room_id: Uuid::new_v4(),
            sequence: 1,
            request_id: Uuid::new_v4(),
            event_type: "message.created".to_owned(),
            actor_id: Uuid::new_v4(),
            actor_role: "human".to_owned(),
            actor_display_name: Some("Maya Chen".to_owned()),
            payload: json!({ "text": "Hello" }),
            occurred_at: Utc.timestamp_opt(0, 0).single().unwrap(),
        };

        assert_eq!(event.to_wire_value()["actor"]["displayName"], "Maya Chen");
    }

    // This fails if a second worker skips a row lock and claims a later task before the oldest.
    #[tokio::test]
    async fn queued_claim_waits_for_the_locked_oldest_task() {
        let pool = PgPoolOptions::new()
            .max_connections(3)
            .connect(
                &std::env::var("DATABASE_URL")
                    .expect("DATABASE_URL must name a migrated PostgreSQL database"),
            )
            .await
            .unwrap();
        let agent_id = Uuid::new_v4();
        let requester_id = Uuid::new_v4();
        let older_task_id = Uuid::new_v4();
        let newer_task_id = Uuid::new_v4();
        let enqueued_at = Utc::now() - Duration::seconds(2);

        for (task_id, created_at) in [
            (older_task_id, enqueued_at),
            (newer_task_id, enqueued_at + Duration::seconds(1)),
        ] {
            sqlx::query(
                r#"
                INSERT INTO agent_tasks (
                    task_id, room_id, request_id, requester_id, agent_id, skill_id, input,
                    context_revision, state, lease_owner, lease_expires_at, created_at, updated_at
                ) VALUES ($1, $2, $3, $4, $5, 'summarize-context', $6, 0, 'queued', NULL, NULL, $7, $7)
                "#,
            )
            .bind(task_id)
            .bind(Uuid::new_v4())
            .bind(Uuid::new_v4())
            .bind(requester_id)
            .bind(agent_id)
            .bind("FIFO test task")
            .bind(created_at)
            .execute(&pool)
            .await
            .unwrap();
        }

        let mut oldest_lock = pool.begin().await.unwrap();
        sqlx::query("SELECT task_id FROM agent_tasks WHERE task_id = $1 FOR UPDATE")
            .bind(older_task_id)
            .fetch_one(&mut *oldest_lock)
            .await
            .unwrap();

        let claim_pool = pool.clone();
        let mut blocked_claim = tokio::spawn(async move {
            claim_oldest_queued_agent_task(&claim_pool, agent_id, Uuid::new_v4(), Utc::now())
                .await
                .unwrap()
                .unwrap()
        });
        assert!(
            timeout(
                Duration::milliseconds(100).to_std().unwrap(),
                &mut blocked_claim
            )
            .await
            .is_err(),
            "a strict FIFO claim must wait for the oldest row lock"
        );

        oldest_lock.commit().await.unwrap();
        assert_eq!(blocked_claim.await.unwrap().task_id, older_task_id);
        assert_eq!(
            claim_oldest_queued_agent_task(&pool, agent_id, Uuid::new_v4(), Utc::now())
                .await
                .unwrap()
                .unwrap()
                .task_id,
            newer_task_id
        );
    }
}

#[derive(Debug)]
pub enum StoreError {
    InvalidEvent,
    InvalidAgentTask,
    ConflictingDuplicate,
    Database(sqlx::Error),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidEvent => formatter.write_str("invalid room event"),
            Self::InvalidAgentTask => formatter.write_str("invalid agent task"),
            Self::ConflictingDuplicate => {
                formatter.write_str("duplicate request with a different payload")
            }
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
            , actor_display_name
        )
        SELECT $1, $2, COALESCE(MAX(sequence), 0) + 1, $3, $4, $5, $6, $7, $8, $9
        FROM room_events
        WHERE room_id = $2
        RETURNING event_id, room_id, sequence, request_id, event_type,
                  actor_id, actor_role, actor_display_name, payload, occurred_at
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
    .bind(event.actor_display_name)
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
               actor_id, actor_role, actor_display_name, payload, occurred_at
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
               actor_id, actor_role, actor_display_name, payload, occurred_at
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

fn agent_task_fingerprint(start: &AgentTaskStart) -> Value {
    json!({
        "method": "agent.task.start",
        "contractVersion": start.contract_version,
        "requestId": start.request_id,
        "roomId": start.room_id,
        "occurredAt": start.occurred_at,
        "agentId": start.agent_id,
        "skillId": start.skill_id,
        "input": start.input,
    })
}

fn requested_task_payload(
    task_id: Uuid,
    requester_id: Uuid,
    start: &AgentTaskStart,
    context_revision: i64,
) -> Value {
    json!({
        "taskId": task_id,
        "agentId": start.agent_id,
        "requesterId": requester_id,
        "skillId": start.skill_id,
        "contextRevision": context_revision,
    })
}

pub(crate) async fn start_agent_task_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    requester_id: Uuid,
    requester_display_name: &str,
    start: AgentTaskStart,
    record_idempotency: bool,
) -> Result<AgentTaskStartResult, StoreError> {
    lock_room(transaction, start.room_id).await?;
    let fingerprint = agent_task_fingerprint(&start);
    if let Some((prior_fingerprint, events)) =
        prior_request(transaction, start.room_id, start.request_id).await?
    {
        if prior_fingerprint != fingerprint {
            return Err(StoreError::ConflictingDuplicate);
        }
        let task_id = sqlx::query_scalar(
            "SELECT task_id FROM agent_tasks WHERE room_id = $1 AND request_id = $2",
        )
        .bind(start.room_id)
        .bind(start.request_id)
        .fetch_one(&mut **transaction)
        .await?;
        return Ok(AgentTaskStartResult { task_id, events });
    }

    let task_id = Uuid::new_v4();
    // The room advisory lock reserves the next room-local sequence for this transaction.
    // That makes the requested event (and the context snapshot ending at it) agree without
    // ever mutating the append-only event after insert.
    let context_revision: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(sequence), 0) + 1 FROM room_events WHERE room_id = $1",
    )
    .bind(start.room_id)
    .fetch_one(&mut **transaction)
    .await?;
    let requested = append_event_in_transaction(
        transaction,
        NewEvent {
            room_id: start.room_id,
            request_id: start.request_id,
            event_type: "agent.task.requested".to_owned(),
            actor_id: requester_id,
            actor_role: "human".to_owned(),
            actor_display_name: Some(requester_display_name.to_owned()),
            payload: requested_task_payload(task_id, requester_id, &start, context_revision),
            occurred_at: start.occurred_at,
        },
    )
    .await?;
    if requested.sequence != context_revision {
        return Err(StoreError::InvalidAgentTask);
    }
    sqlx::query(
        r#"
        INSERT INTO agent_tasks (
            task_id, room_id, request_id, requester_id, agent_id, skill_id, input,
            context_revision, state, lease_owner, lease_expires_at, created_at, updated_at
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'queued', NULL, NULL, NOW(), NOW())
        "#,
    )
    .bind(task_id)
    .bind(start.room_id)
    .bind(start.request_id)
    .bind(requester_id)
    .bind(start.agent_id)
    .bind(start.skill_id.as_str())
    .bind(start.input)
    .bind(context_revision)
    .execute(&mut **transaction)
    .await?;
    if record_idempotency {
        record_request(
            transaction,
            start.room_id,
            start.request_id,
            fingerprint,
            std::slice::from_ref(&requested),
        )
        .await?;
    }
    Ok(AgentTaskStartResult {
        task_id,
        events: vec![requested],
    })
}

pub async fn agent_task(pool: &PgPool, task_id: Uuid) -> Result<AgentTaskRecord, StoreError> {
    let row = sqlx::query(
        r#"
        SELECT task_id, room_id, request_id, requester_id, agent_id, skill_id, input,
               context_revision, state, lease_owner, lease_expires_at, created_at, updated_at
        FROM agent_tasks WHERE task_id = $1
        "#,
    )
    .bind(task_id)
    .fetch_one(pool)
    .await?;
    agent_task_from_row(row)
}

pub async fn claim_agent_task(
    pool: &PgPool,
    task_id: Uuid,
    owner_id: Uuid,
    now: DateTime<Utc>,
) -> Result<Option<AgentTaskLease>, StoreError> {
    let expires_at = now + AGENT_TASK_LEASE_DURATION;
    let row = sqlx::query(
        r#"
        UPDATE agent_tasks
        SET lease_owner = $2, lease_expires_at = $3,
            state = CASE WHEN state = 'queued' THEN 'running' ELSE state END,
            updated_at = $1
        WHERE task_id = $4
          AND state IN ('queued', 'running', 'awaiting_external_input')
          AND (lease_expires_at IS NULL OR lease_expires_at <= $1)
        RETURNING task_id, lease_owner, lease_expires_at
        "#,
    )
    .bind(now)
    .bind(owner_id)
    .bind(expires_at)
    .bind(task_id)
    .fetch_optional(pool)
    .await?;
    row.map(|row| {
        Ok(AgentTaskLease {
            task_id: row.try_get("task_id")?,
            owner_id: row.try_get("lease_owner")?,
            expires_at: row.try_get("lease_expires_at")?,
        })
    })
    .transpose()
}

/// Atomically claims the oldest queued task for one registered external agent.  Selection and
/// lease acquisition share one statement so competing agent-gateway processes cannot observe and
/// then steal the same task.
pub(crate) async fn claim_oldest_queued_agent_task(
    pool: &PgPool,
    agent_id: Uuid,
    owner_id: Uuid,
    now: DateTime<Utc>,
) -> Result<Option<AgentTaskLease>, StoreError> {
    let expires_at = now + AGENT_TASK_LEASE_DURATION;
    let mut transaction = pool.begin().await?;
    // Claims for one agent share a transaction-scoped lock.  This prevents a concurrent worker
    // from skipping the head of that agent's queue while it is being claimed.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::text, 1))")
        .bind(agent_id)
        .execute(&mut *transaction)
        .await?;
    let row = sqlx::query(
        r#"
        WITH claimable AS (
            SELECT task_id
            FROM agent_tasks
            WHERE agent_id = $1
              AND state = 'queued'
              AND (lease_expires_at IS NULL OR lease_expires_at <= $2)
            ORDER BY created_at ASC, task_id ASC
            LIMIT 1
            FOR UPDATE
        )
        UPDATE agent_tasks
        SET lease_owner = $3, lease_expires_at = $4, state = 'running', updated_at = $2
        FROM claimable
        WHERE agent_tasks.task_id = claimable.task_id
        RETURNING agent_tasks.task_id, agent_tasks.lease_owner, agent_tasks.lease_expires_at
        "#,
    )
    .bind(agent_id)
    .bind(now)
    .bind(owner_id)
    .bind(expires_at)
    .fetch_optional(&mut *transaction)
    .await?;
    let lease: Option<AgentTaskLease> = row
        .map(|row| -> Result<AgentTaskLease, StoreError> {
            Ok(AgentTaskLease {
                task_id: row.try_get("task_id")?,
                owner_id: row.try_get("lease_owner")?,
                expires_at: row.try_get("lease_expires_at")?,
            })
        })
        .transpose()?;
    transaction.commit().await?;
    Ok(lease)
}

pub async fn context_for_lease(
    pool: &PgPool,
    lease: &AgentTaskLease,
) -> Result<Option<AgentTaskContext>, StoreError> {
    let task = sqlx::query(
        r#"
        SELECT task_id, room_id, request_id, requester_id, agent_id, skill_id, input,
               context_revision, state, lease_owner, lease_expires_at, created_at, updated_at
        FROM agent_tasks
        WHERE task_id = $1 AND lease_owner = $2 AND lease_expires_at = $3
          AND lease_expires_at > NOW()
        "#,
    )
    .bind(lease.task_id)
    .bind(lease.owner_id)
    .bind(lease.expires_at)
    .fetch_optional(pool)
    .await?
    .map(agent_task_from_row)
    .transpose()?;
    let Some(task) = task else {
        return Ok(None);
    };
    let rows = sqlx::query(
        r#"
        SELECT event_id, room_id, sequence, request_id, event_type,
               actor_id, actor_role, actor_display_name, payload, occurred_at
        FROM room_events
        WHERE room_id = $1 AND sequence <= $2
        ORDER BY sequence ASC
        "#,
    )
    .bind(task.room_id)
    .bind(task.context_revision)
    .fetch_all(pool)
    .await?;
    Ok(Some(AgentTaskContext {
        task,
        events: rows
            .into_iter()
            .map(room_event_from_row)
            .collect::<Result<_, _>>()?,
    }))
}

pub async fn record_agent_task_update(
    pool: &PgPool,
    lease: &AgentTaskLease,
    update: AgentTaskUpdate,
) -> Result<AgentTaskStartResult, StoreError> {
    let result = record_agent_task_update_with_outcome(pool, lease, update).await?;
    Ok(AgentTaskStartResult {
        task_id: result.task_id,
        events: result.events,
    })
}

pub(crate) async fn record_agent_task_update_with_outcome(
    pool: &PgPool,
    lease: &AgentTaskLease,
    update: AgentTaskUpdate,
) -> Result<AgentTaskUpdateResult, StoreError> {
    let mut transaction = pool.begin().await?;
    let task = sqlx::query(
        r#"
        SELECT task_id, room_id, request_id, requester_id, agent_id, skill_id, input,
               context_revision, state, lease_owner, lease_expires_at, created_at, updated_at
        FROM agent_tasks
        WHERE task_id = $1 AND lease_owner = $2 AND lease_expires_at = $3
          AND lease_expires_at > NOW()
        FOR UPDATE
        "#,
    )
    .bind(lease.task_id)
    .bind(lease.owner_id)
    .bind(lease.expires_at)
    .fetch_optional(&mut *transaction)
    .await?
    .map(agent_task_from_row)
    .transpose()?;
    let Some(task) = task else {
        transaction.rollback().await?;
        return Err(StoreError::InvalidAgentTask);
    };
    lock_room(&mut transaction, task.room_id).await?;
    let payload = normalized_agent_task_update_payload(&task, update.payload.clone())?;
    if let Some(event_ids) = sqlx::query_scalar::<_, Vec<Uuid>>(
        "SELECT event_ids FROM agent_task_updates WHERE task_id = $1 AND update_id = $2",
    )
    .bind(task.task_id)
    .bind(update.update_id)
    .fetch_optional(&mut *transaction)
    .await?
    {
        let events = events_by_id(&mut transaction, &event_ids).await?;
        transaction.rollback().await?;
        if !is_matching_agent_task_update(&events, &update, &payload) {
            return Err(StoreError::ConflictingDuplicate);
        }
        return Ok(AgentTaskUpdateResult {
            task_id: task.task_id,
            events,
            duplicate: true,
        });
    }
    if !matches!(
        task.state,
        AgentTaskState::Queued | AgentTaskState::Running | AgentTaskState::AwaitingExternalInput
    ) {
        transaction.rollback().await?;
        return Err(StoreError::InvalidAgentTask);
    }
    let state = state_for_update(&update.event_type)?;
    let event = append_event_in_transaction(
        &mut transaction,
        NewEvent {
            room_id: task.room_id,
            request_id: Uuid::new_v4(),
            event_type: update.event_type,
            actor_id: task.agent_id,
            actor_role: "agent".to_owned(),
            actor_display_name: None,
            payload,
            occurred_at: update.occurred_at,
        },
    )
    .await?;
    sqlx::query(
        "INSERT INTO agent_task_updates (task_id, update_id, event_ids) VALUES ($1, $2, $3)",
    )
    .bind(task.task_id)
    .bind(update.update_id)
    .bind(vec![event.event_id])
    .execute(&mut *transaction)
    .await?;
    sqlx::query("UPDATE agent_tasks SET state = $1, updated_at = $2 WHERE task_id = $3")
        .bind(state.as_str())
        .bind(update.occurred_at)
        .bind(task.task_id)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(AgentTaskUpdateResult {
        task_id: task.task_id,
        events: vec![event],
        duplicate: false,
    })
}

fn normalized_agent_task_update_payload(
    task: &AgentTaskRecord,
    mut payload: Value,
) -> Result<Value, StoreError> {
    let core = json!({
        "taskId": task.task_id,
        "agentId": task.agent_id,
        "requesterId": task.requester_id,
        "skillId": task.skill_id,
        "contextRevision": task.context_revision,
    });
    let object = payload
        .as_object_mut()
        .ok_or(StoreError::InvalidAgentTask)?;
    for (key, value) in core.as_object().expect("JSON object") {
        object.insert(key.clone(), value.clone());
    }
    Ok(payload)
}

fn is_matching_agent_task_update(
    events: &[RoomEvent],
    update: &AgentTaskUpdate,
    normalized_payload: &Value,
) -> bool {
    events.len() == 1
        && events[0].event_type == update.event_type
        && events[0].payload == *normalized_payload
        // PostgreSQL timestamps are stored at microsecond precision.
        && events[0].occurred_at.timestamp_micros() == update.occurred_at.timestamp_micros()
}

fn state_for_update(event_type: &str) -> Result<AgentTaskState, StoreError> {
    match event_type {
        "agent.task.progressed" => Ok(AgentTaskState::Running),
        "agent.task.awaiting_external_input" => Ok(AgentTaskState::AwaitingExternalInput),
        "agent.task.succeeded" => Ok(AgentTaskState::Succeeded),
        "agent.task.failed" => Ok(AgentTaskState::Failed),
        _ => Err(StoreError::InvalidAgentTask),
    }
}

async fn events_by_id(
    transaction: &mut Transaction<'_, Postgres>,
    event_ids: &[Uuid],
) -> Result<Vec<RoomEvent>, StoreError> {
    let rows = sqlx::query(
        r#"
        SELECT event_id, room_id, sequence, request_id, event_type,
               actor_id, actor_role, actor_display_name, payload, occurred_at
        FROM room_events WHERE event_id = ANY($1) ORDER BY sequence ASC
        "#,
    )
    .bind(event_ids)
    .fetch_all(&mut **transaction)
    .await?;
    rows.into_iter().map(room_event_from_row).collect()
}

fn agent_task_from_row(row: PgRow) -> Result<AgentTaskRecord, StoreError> {
    let skill_id = match row.try_get::<String, _>("skill_id")?.as_str() {
        "summarize-context" => AgentSkillId::SummarizeContext,
        "extract-action-items" => AgentSkillId::ExtractActionItems,
        _ => return Err(StoreError::InvalidAgentTask),
    };
    let state = AgentTaskState::parse(&row.try_get::<String, _>("state")?)?;
    Ok(AgentTaskRecord {
        task_id: row.try_get("task_id")?,
        room_id: row.try_get("room_id")?,
        request_id: row.try_get("request_id")?,
        requester_id: row.try_get("requester_id")?,
        agent_id: row.try_get("agent_id")?,
        skill_id,
        input: row.try_get("input")?,
        context_revision: row.try_get("context_revision")?,
        state,
        lease_owner: row.try_get("lease_owner")?,
        lease_expires_at: row.try_get("lease_expires_at")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
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
        actor_display_name: row.try_get("actor_display_name")?,
        payload: row.try_get("payload")?,
        occurred_at: row.try_get("occurred_at")?,
    })
}
