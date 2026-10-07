use crate::{
    Actor, NewEvent,
    conversation_context::{build_context, source_manifest},
    conversation_protocol::{
        CODEX_AGENT_ID, ConversationError, MAX_SAFE_INTEGER, PROFILE_VERSION,
        validate_profile_value, validate_turn_request,
    },
    rooms::authorize_conversation_turn,
    store::{append_event_in_transaction, lock_room, prior_request, record_request},
};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Row, Transaction, postgres::PgRow};
use std::collections::HashSet;
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationModel {
    pub id: String,
    pub reasoning_efforts: Vec<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
pub struct ConversationPolicy {
    pub enabled: bool,
    pub policy_revision: String,
    pub guidance_revision: String,
    pub catalog_revision: String,
    pub model: String,
    pub reasoning_effort: String,
    pub models: Vec<ConversationModel>,
    pub max_context_bytes: usize,
    pub max_records: usize,
    pub turn_deadline_seconds: i64,
    pub interrupt_grace_seconds: i64,
}
impl Default for ConversationPolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            policy_revision: String::new(),
            guidance_revision: String::new(),
            catalog_revision: String::new(),
            model: String::new(),
            reasoning_effort: String::new(),
            models: vec![],
            max_context_bytes: 1_048_576,
            max_records: 2000,
            turn_deadline_seconds: 180,
            interrupt_grace_seconds: 5,
        }
    }
}
impl ConversationPolicy {
    pub fn validate(&self) -> Result<(), ConversationError> {
        let valid_id = |id: &str| !id.is_empty() && id.chars().count() <= 128;
        if self.max_context_bytes == 0
            || self.max_context_bytes > 1_048_576
            || self.max_records == 0
            || self.max_records > 2000
            || !(1..=180).contains(&self.turn_deadline_seconds)
            || !(1..=5).contains(&self.interrupt_grace_seconds)
        {
            return Err(ConversationError::InvalidTaskInput);
        }
        if !self.enabled {
            return Ok(());
        }
        if ![
            &self.policy_revision,
            &self.guidance_revision,
            &self.catalog_revision,
            &self.model,
            &self.reasoning_effort,
        ]
        .into_iter()
        .all(|id| valid_id(id))
            || self.models.is_empty()
            || self.models.len() > 100
        {
            return Err(ConversationError::InvalidTaskInput);
        }
        let mut ids = HashSet::new();
        for model in &self.models {
            let mut efforts = HashSet::new();
            if !valid_id(&model.id)
                || !ids.insert(&model.id)
                || model.reasoning_efforts.is_empty()
                || model.reasoning_efforts.len() > 100
                || !model
                    .reasoning_efforts
                    .iter()
                    .all(|id| valid_id(id) && efforts.insert(id))
            {
                return Err(ConversationError::InvalidTaskInput);
            }
        }
        self.resolve_settings(None).map(|_| ())
    }
    fn resolve_settings(&self, settings: Option<&Value>) -> Result<Value, ConversationError> {
        let defaults = json!({"model":self.model,"reasoningEffort":self.reasoning_effort,"catalogRevision":self.catalog_revision});
        let selected = settings.unwrap_or(&defaults);
        if selected["catalogRevision"] != self.catalog_revision
            || !self.models.iter().any(|model| {
                selected["model"] == model.id
                    && model
                        .reasoning_efforts
                        .iter()
                        .any(|effort| selected["reasoningEffort"] == *effort)
            })
        {
            return Err(ConversationError::InvalidTaskInput);
        }
        Ok(selected.clone())
    }
}
#[derive(Clone)]
pub struct ConversationStore {
    pool: PgPool,
    policy: ConversationPolicy,
}
impl ConversationStore {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            policy: ConversationPolicy::default(),
        }
    }
    pub fn with_policy(mut self, policy: ConversationPolicy) -> Result<Self, ConversationError> {
        policy.validate()?;
        self.policy = policy;
        Ok(self)
    }
    pub async fn reserve(
        &self,
        actor: &Actor,
        request: &Value,
        authorization_expires_at: DateTime<Utc>,
    ) -> Result<Value, ConversationError> {
        self.reserve_inner(actor, request, authorization_expires_at, None, None)
            .await
            .map(|outcome| outcome.accepted)
    }
    pub(crate) async fn reserve_mediated(
        &self,
        actor: &Actor,
        request: &Value,
        authorization_expires_at: DateTime<Utc>,
        catalog: &dyn crate::conversation_service::CatalogQuery,
        state: &crate::GatewayState,
    ) -> Result<ConversationReservation, ConversationError> {
        self.reserve_inner(
            actor,
            request,
            authorization_expires_at,
            Some(catalog),
            Some(state),
        )
        .await
    }
    async fn reserve_inner(
        &self,
        actor: &Actor,
        request: &Value,
        authorization_expires_at: DateTime<Utc>,
        catalog: Option<&dyn crate::conversation_service::CatalogQuery>,
        state: Option<&crate::GatewayState>,
    ) -> Result<ConversationReservation, ConversationError> {
        let minimum = Duration::seconds(
            self.policy.turn_deadline_seconds + self.policy.interrupt_grace_seconds,
        );
        authorize_conversation_turn(actor, authorization_expires_at, Utc::now(), minimum)?;
        if !self.policy.enabled {
            return Err(ConversationError::RuntimeUnavailable);
        }
        let request = validate_turn_request(request)?;
        let room = uuid_field(&request, "roomId")?;
        let request_id = uuid_field(&request, "requestId")?;
        let fingerprint =
            json!({"method":"agent-conversation.turn","requesterId":actor.id,"intent":request});
        let mut tx = self.pool.begin().await?;
        lock_room(&mut tx, room).await?;
        // Lock acquisition can wait behind another writer. Recheck token authority here.
        let now = Utc::now();
        let authorization_expires_at =
            authorize_conversation_turn(actor, authorization_expires_at, now, minimum)?;
        if let Some((prior, _)) = prior_request(&mut tx, room, request_id).await? {
            if prior != fingerprint {
                return Err(ConversationError::DuplicateConflict);
            }
            let accepted: Option<Value>=sqlx::query_scalar("SELECT accepted_turn FROM conversation_tasks WHERE room_id=$1 AND request_id=$2 AND requester_id=$3")
                .bind(room).bind(request_id).bind(actor.id).fetch_optional(&mut *tx).await?;
            return accepted
                .map(|accepted| ConversationReservation {
                    accepted,
                    prompt: None,
                })
                .ok_or(ConversationError::DuplicateConflict);
        }
        if let Some(state) = state {
            state
                .validate_conversation_mentions(&mut tx, room, &request["mentions"])
                .await?;
        }
        let current=sqlx::query("SELECT * FROM agent_conversations WHERE room_id=$1 AND agent_id=$2 AND state <> 'superseded'")
            .bind(room).bind(CODEX_AGENT_ID).fetch_optional(&mut *tx).await?;
        if current
            .as_ref()
            .is_some_and(|row| matches!(row.get::<&str, _>("state"), "reserved" | "running"))
        {
            return Err(ConversationError::ConversationBusy);
        }
        let explicit_mode = request
            .pointer("/conversation/mode")
            .and_then(Value::as_str);
        let mode = explicit_mode.unwrap_or(if current.is_some() { "continue" } else { "new" });
        let (conversation_id, generation, base) = if mode == "continue" {
            let current = current
                .as_ref()
                .ok_or(ConversationError::ConversationStale)?;
            let id: Uuid = current.try_get("conversation_id")?;
            let generation: i64 = current.try_get("generation")?;
            if current.get::<&str, _>("state") != "ready"
                || current.get::<&str, _>("policy_revision") != self.policy.policy_revision
                || current.get::<&str, _>("guidance_revision") != self.policy.guidance_revision
                || explicit_mode.is_some_and(|_| {
                    request["conversation"]["id"] != json!(id)
                        || request["conversation"]["generation"] != generation
                })
            {
                return Err(ConversationError::ConversationStale);
            }
            let consumed = current.try_get::<i64, _>("consumed_revision")?;
            if consumed <= 0 {
                return Err(ConversationError::ContextMismatch);
            }
            (id, generation, consumed)
        } else {
            let previous:i64=sqlx::query_scalar("SELECT COALESCE(MAX(generation),0) FROM agent_conversations WHERE room_id=$1 AND agent_id=$2")
                .bind(room).bind(CODEX_AGENT_ID).fetch_one(&mut *tx).await?;
            if previous as u64 >= MAX_SAFE_INTEGER {
                return Err(ConversationError::ConversationStale);
            }
            (Uuid::new_v4(), previous + 1, 0)
        };
        // Accepted choices are shared continuation defaults. Refresh only their
        // catalog revision; current policy/catalog still validate the exact pair.
        let shared = if mode == "continue" {
            let previous: Value = current
                .as_ref()
                .ok_or(ConversationError::ConversationStale)?
                .try_get("selected_settings")?;
            Some(
                json!({"model":previous["model"],"reasoningEffort":previous["reasoningEffort"],"catalogRevision":self.policy.catalog_revision}),
            )
        } else {
            None
        };
        let settings = self
            .policy
            .resolve_settings(request.get("settings").or(shared.as_ref()))?;
        if let Some(catalog) = catalog {
            tokio::time::timeout(
                std::time::Duration::from_secs(5),
                crate::conversation_service::validate_catalog_selection(
                    catalog,
                    &self.policy,
                    &settings,
                ),
            )
            .await
            .map_err(|_| ConversationError::RuntimeUnavailable)??;
        }
        let task_id = Uuid::new_v4();
        let prompt=append_event_in_transaction(&mut tx, NewEvent {
            room_id:room,request_id,event_type:"message.created".into(),actor_id:actor.id,actor_role:"human".into(),actor_display_name:Some(actor.display_name.clone()),
            payload:json!({"text":request["text"],"mentions":request["mentions"],"delivery":"room","audienceIds":[]}),
            occurred_at:DateTime::parse_from_rfc3339(request["occurredAt"].as_str().ok_or(ConversationError::InvalidTaskInput)?).map_err(|_| ConversationError::InvalidTaskInput)?.with_timezone(&Utc),
        }).await?;
        if prompt.sequence as u64 > MAX_SAFE_INTEGER {
            return Err(ConversationError::ContextMismatch);
        }
        let context = build_context(
            &mut tx,
            room,
            conversation_id,
            generation,
            base,
            prompt.event_id,
            &self.policy,
        )
        .await?;
        let expires = now + Duration::seconds(self.policy.turn_deadline_seconds);
        let frozen = json!({"profileVersion":PROFILE_VERSION,"taskId":task_id,"roomId":room,"requesterId":actor.id,"agentId":CODEX_AGENT_ID,"skillId":"chat","conversation":{"id":conversation_id,"generation":generation,"mode":mode},"triggerEventId":prompt.event_id,"context":context,"guidanceRevision":self.policy.guidance_revision,"issuedAt":now,"expiresAt":expires,"authorizationExpiresAt":authorization_expires_at,"model":settings["model"],"reasoningEffort":settings["reasoningEffort"],"catalogRevision":settings["catalogRevision"]});
        validate_profile_value("reserved-input", &frozen)?;
        let accepted = json!({"profileVersion":PROFILE_VERSION,"roomId":room,"taskId":task_id,"conversationId":conversation_id,"generation":generation,"triggerEventId":prompt.event_id,"contextRevision":prompt.sequence,"selectedSettings":settings});
        validate_profile_value("task", &accepted)?;
        let sources = source_manifest(&context);
        record_request(
            &mut tx,
            room,
            request_id,
            fingerprint,
            std::slice::from_ref(&prompt),
        )
        .await?;
        if mode == "new" {
            if let Some(previous) = current {
                sqlx::query("UPDATE agent_conversations SET state='superseded', active_task_id=NULL, updated_at=$2 WHERE conversation_id=$1")
                    .bind(previous.get::<Uuid,_>("conversation_id")).bind(now).execute(&mut *tx).await?;
            }
            sqlx::query("INSERT INTO agent_conversations (conversation_id,room_id,agent_id,generation,state,policy_revision,guidance_revision,selected_settings,active_task_id,created_at,updated_at) VALUES ($1,$2,$3,$4,'reserved',$5,$6,$7,$8,$9,$9)")
                .bind(conversation_id).bind(room).bind(CODEX_AGENT_ID).bind(generation).bind(&self.policy.policy_revision).bind(&self.policy.guidance_revision).bind(&settings).bind(task_id).bind(now).execute(&mut *tx).await?;
        } else {
            sqlx::query("UPDATE agent_conversations SET state='reserved',active_task_id=$2,selected_settings=$3,updated_at=$4 WHERE conversation_id=$1")
                .bind(conversation_id).bind(task_id).bind(&settings).bind(now).execute(&mut *tx).await?;
        }
        sqlx::query("INSERT INTO conversation_tasks (task_id,room_id,request_id,requester_id,agent_id,conversation_id,generation,trigger_event_id,context_revision,state,frozen_input,selected_settings,source_manifest,policy_revision,authorization_expires_at,issued_at,expires_at,created_at,updated_at,accepted_turn) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,'reserved',$10,$11,$12,$13,$14,$15,$16,$15,$15,$17)")
            .bind(task_id).bind(room).bind(request_id).bind(actor.id).bind(CODEX_AGENT_ID).bind(conversation_id).bind(generation).bind(uuid_field(&accepted,"triggerEventId")?).bind(accepted["contextRevision"].as_i64()).bind(frozen).bind(settings).bind(sources).bind(&self.policy.policy_revision).bind(authorization_expires_at).bind(now).bind(expires).bind(&accepted).execute(&mut *tx).await?;
        // Snapshot work may have consumed the authorization grace. Never commit stale authority.
        authorize_conversation_turn(actor, authorization_expires_at, Utc::now(), minimum)?;
        tx.commit().await?;
        Ok(ConversationReservation {
            accepted,
            prompt: Some(prompt),
        })
    }
}

pub(crate) struct ConversationUpdateOutcome {
    pub response: Value,
    pub event: Option<crate::RoomEvent>,
}
pub(crate) struct ConversationReservation {
    pub accepted: Value,
    pub prompt: Option<crate::RoomEvent>,
}

impl ConversationStore {
    pub(crate) fn policy(&self) -> &ConversationPolicy {
        &self.policy
    }

    pub(crate) async fn conversation_view(
        &self,
        room: Uuid,
        agent: Uuid,
    ) -> Result<Value, ConversationError> {
        self.require_enabled(agent)?;
        let row=sqlx::query("SELECT c.*,t.effective_settings,t.usage FROM agent_conversations c LEFT JOIN LATERAL (SELECT effective_settings,usage FROM conversation_tasks WHERE conversation_id=c.conversation_id ORDER BY created_at DESC,task_id DESC LIMIT 1) t ON true WHERE c.room_id=$1 AND c.agent_id=$2 AND c.state<>'superseded'").bind(room).bind(agent).fetch_optional(&self.pool).await?;
        let conversation=row.map(|row| json!({"id":row.get::<Uuid,_>("conversation_id"),"generation":row.get::<i64,_>("generation"),"state":row.get::<&str,_>("state"),"activeTaskId":row.get::<Option<Uuid>,_>("active_task_id"),"consumedRevision":row.get::<i64,_>("consumed_revision"),"policyRevision":row.get::<&str,_>("policy_revision"),"guidanceRevision":row.get::<&str,_>("guidance_revision"),"selectedSettings":row.get::<Value,_>("selected_settings"),"effectiveSettings":row.get::<Option<Value>,_>("effective_settings"),"usage":row.get::<Option<Value>,_>("usage")}));
        let view = json!({"profileVersion":PROFILE_VERSION,"roomId":room,"agentId":agent,"conversation":conversation});
        validate_profile_value("view", &view)?;
        Ok(view)
    }

    pub(crate) async fn task_view(
        &self,
        room: Uuid,
        task: Uuid,
    ) -> Result<Value, ConversationError> {
        self.require_enabled(CODEX_AGENT_ID)?;
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await?;
        let row = sqlx::query(
            "SELECT * FROM conversation_tasks WHERE task_id=$1 AND room_id=$2 AND agent_id=$3",
        )
        .bind(task)
        .bind(room)
        .bind(CODEX_AGENT_ID)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(ConversationError::SessionUnavailable)?;
        let updates = sqlx::query(
            "SELECT * FROM conversation_updates WHERE task_id=$1 ORDER BY ordinal LIMIT 256",
        )
        .bind(task)
        .fetch_all(&mut *tx)
        .await?;
        let updates:Vec<Value>=updates.into_iter().map(|row|json!({"updateId":row.get::<Uuid,_>("update_id"),"ordinal":row.get::<i64,_>("ordinal"),"kind":row.get::<&str,_>("kind"),"occurredAt":row.get::<DateTime<Utc>,_>("occurred_at"),"data":row.get::<Value,_>("data")})).collect();
        let view = json!({"profileVersion":PROFILE_VERSION,"roomId":room,"taskId":task,"conversationId":row.get::<Uuid,_>("conversation_id"),"generation":row.get::<i64,_>("generation"),"requesterId":row.get::<Uuid,_>("requester_id"),"state":row.get::<&str,_>("state"),"selectedSettings":row.get::<Value,_>("selected_settings"),"effectiveSettings":row.get::<Option<Value>,_>("effective_settings"),"usage":row.get::<Option<Value>,_>("usage"),"updates":updates,"result":row.get::<Option<Value>,_>("result"),"replyEventId":row.get::<Option<Uuid>,_>("reply_event_id"),"failure":row.get::<Option<Value>,_>("failure")});
        validate_profile_value("task", &view)?;
        tx.commit().await?;
        Ok(view)
    }

    fn require_enabled(&self, agent: Uuid) -> Result<(), ConversationError> {
        if agent != CODEX_AGENT_ID {
            return Err(ConversationError::Forbidden);
        }
        if !self.policy.enabled {
            return Err(ConversationError::RuntimeUnavailable);
        }
        Ok(())
    }

    pub(crate) async fn claim(&self, owner: &str) -> Result<Option<Value>, ConversationError> {
        self.require_enabled(CODEX_AGENT_ID)?;
        if owner.is_empty() || owner.chars().count() > 128 {
            return Err(ConversationError::InvalidTaskInput);
        }
        for _ in 0..100 {
            let mut tx = self.pool.begin().await?;
            sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('agent-conversations:claim:codex',0))").execute(&mut *tx).await?;
            let candidate=sqlx::query("SELECT task_id,room_id FROM conversation_tasks WHERE agent_id=$1 AND (state='reserved' OR (state='running' AND (lease_expires_at<=CURRENT_TIMESTAMP OR expires_at<=CURRENT_TIMESTAMP OR authorization_expires_at<=CURRENT_TIMESTAMP))) ORDER BY created_at,task_id LIMIT 1").bind(CODEX_AGENT_ID).fetch_optional(&mut *tx).await?;
            let Some(candidate) = candidate else {
                return Ok(None);
            };
            lock_room(&mut tx, candidate.get("room_id")).await?;
            let row = task_in_transaction(&mut tx, candidate.get("task_id")).await?;
            if let Err(error) = self.task_authority(&row, None, true) {
                fail_task(&mut tx, &row, error).await?;
                tx.commit().await?;
                continue;
            }
            let token = Uuid::new_v4();
            let deadline: DateTime<Utc> = row.try_get("expires_at")?;
            let auth: DateTime<Utc> = row.try_get("authorization_expires_at")?;
            let lease_expiry =
                (deadline + Duration::seconds(self.policy.interrupt_grace_seconds)).min(auth);
            sqlx::query("UPDATE conversation_tasks SET state='running',lease_owner=$2,lease_token=$3,lease_expires_at=$4,updated_at=CURRENT_TIMESTAMP WHERE task_id=$1").bind(row.get::<Uuid,_>("task_id")).bind(owner).bind(token).bind(lease_expiry).execute(&mut *tx).await?;
            sqlx::query("UPDATE agent_conversations SET state='running',updated_at=CURRENT_TIMESTAMP WHERE conversation_id=$1").bind(row.get::<Uuid,_>("conversation_id")).execute(&mut *tx).await?;
            let mut packet: Value = row.try_get("frozen_input")?;
            packet["leaseOwner"] = json!(owner);
            packet["leaseExpiresAt"] = json!(lease_expiry);
            validate_profile_value("input", &packet)?;
            tx.commit().await?;
            return Ok(Some(json!({"packet":packet,"leaseToken":token})));
        }
        Ok(None)
    }

    fn task_authority(
        &self,
        row: &PgRow,
        lease: Option<Uuid>,
        claim: bool,
    ) -> Result<(), ConversationError> {
        self.require_enabled(row.try_get("agent_id")?)?;
        let now = Utc::now();
        let frozen: Value = row.try_get("frozen_input")?;
        if row.get::<&str, _>("policy_revision") != self.policy.policy_revision
            || frozen["guidanceRevision"] != self.policy.guidance_revision
            || !self.policy.models.iter().any(|model| {
                frozen["model"] == model.id
                    && model
                        .reasoning_efforts
                        .iter()
                        .any(|effort| frozen["reasoningEffort"] == *effort)
            })
        {
            return Err(ConversationError::Forbidden);
        }
        if row.get::<DateTime<Utc>, _>("authorization_expires_at") <= now {
            return Err(ConversationError::AuthenticationRequired);
        }
        if row.get::<DateTime<Utc>, _>("expires_at") <= now {
            return Err(ConversationError::Timeout);
        }
        if !matches!(row.get::<&str, _>("state"), "reserved" | "running")
            || row.get::<Option<Uuid>, _>("active_task_id") != Some(row.get("task_id"))
            || !matches!(
                row.get::<&str, _>("conversation_state"),
                "reserved" | "running"
            )
        {
            return Err(ConversationError::ConversationStale);
        }
        if claim {
            if row.get::<&str, _>("state") != "reserved"
                || row.get::<Option<Uuid>, _>("lease_token").is_some()
            {
                return Err(ConversationError::ConversationInterrupted);
            }
        } else if lease.is_none() || row.get::<Option<Uuid>, _>("lease_token") != lease {
            return Err(ConversationError::Forbidden);
        } else if row
            .get::<Option<DateTime<Utc>>, _>("lease_expires_at")
            .is_none_or(|expiry| expiry <= now)
        {
            return Err(ConversationError::ConversationInterrupted);
        }
        Ok(())
    }

    pub(crate) async fn packet(&self, task: Uuid, lease: Uuid) -> Result<Value, ConversationError> {
        let mut tx = self.pool.begin().await?;
        let room: Option<Uuid> = sqlx::query_scalar(
            "SELECT room_id FROM conversation_tasks WHERE task_id=$1 AND agent_id=$2",
        )
        .bind(task)
        .bind(CODEX_AGENT_ID)
        .fetch_optional(&mut *tx)
        .await?;
        lock_room(&mut tx, room.ok_or(ConversationError::SessionUnavailable)?).await?;
        let row = task_in_transaction(&mut tx, task).await?;
        if let Err(error) = self.task_authority(&row, Some(lease), false) {
            if row.get::<Option<Uuid>, _>("lease_token") == Some(lease)
                && matches!(
                    error,
                    ConversationError::Timeout
                        | ConversationError::AuthenticationRequired
                        | ConversationError::ConversationInterrupted
                        | ConversationError::Forbidden
                        | ConversationError::RuntimeUnavailable
                )
                && matches!(row.get::<&str, _>("state"), "reserved" | "running")
            {
                fail_task(&mut tx, &row, error).await?;
                tx.commit().await?;
            }
            return Err(error);
        }
        let mut packet: Value = row.try_get("frozen_input")?;
        packet["leaseOwner"] = json!(row.get::<Option<String>, _>("lease_owner"));
        packet["leaseExpiresAt"] = json!(row.get::<Option<DateTime<Utc>>, _>("lease_expires_at"));
        validate_profile_value("input", &packet)?;
        Ok(packet)
    }

    pub(crate) async fn receipt(&self, task: Uuid) -> Result<Value, ConversationError> {
        self.require_enabled(CODEX_AGENT_ID)?;
        let row = sqlx::query("SELECT * FROM conversation_tasks WHERE task_id=$1 AND agent_id=$2")
            .bind(task)
            .bind(CODEX_AGENT_ID)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(ConversationError::SessionUnavailable)?;
        Ok(
            json!({"profileVersion":PROFILE_VERSION,"taskId":task,"conversationId":row.get::<Uuid,_>("conversation_id"),"generation":row.get::<i64,_>("generation"),"state":row.get::<&str,_>("state"),"acknowledgement":row.get::<Option<Value>,_>("receipt_acknowledgement"),"result":row.get::<Option<Value>,_>("result")}),
        )
    }

    pub(crate) async fn record_update(
        &self,
        task: Uuid,
        lease: Uuid,
        request: &Value,
    ) -> Result<ConversationUpdateOutcome, ConversationError> {
        validate_profile_value("update", request)?;
        if request["taskId"] != json!(task) {
            return Err(ConversationError::ContextMismatch);
        }
        let mut tx = self.pool.begin().await?;
        let room: Option<Uuid> = sqlx::query_scalar(
            "SELECT room_id FROM conversation_tasks WHERE task_id=$1 AND agent_id=$2",
        )
        .bind(task)
        .bind(CODEX_AGENT_ID)
        .fetch_optional(&mut *tx)
        .await?;
        let room = room.ok_or(ConversationError::SessionUnavailable)?;
        lock_room(&mut tx, room).await?;
        let row = task_in_transaction(&mut tx, task).await?;
        self.require_enabled(CODEX_AGENT_ID)?;
        let frozen: Value = row.try_get("frozen_input")?;
        if request["generation"] != row.get::<i64, _>("generation")
            || request["contextDigest"] != frozen["context"]["digest"]
        {
            return Err(ConversationError::ContextMismatch);
        }
        if row.get::<Option<Uuid>, _>("lease_token") != Some(lease) {
            return Err(ConversationError::Forbidden);
        }
        let update_id = uuid_field(request, "updateId")?;
        let prior=sqlx::query("SELECT request,response FROM conversation_update_requests WHERE task_id=$1 AND update_id=$2").bind(task).bind(update_id).fetch_optional(&mut *tx).await?;
        if let Some(prior) = prior {
            if prior.get::<Value, _>("request") != *request {
                return Err(ConversationError::DuplicateConflict);
            }
            // Terminal replay is lease-bound; receipt is the expiry recovery path.
            if matches!(row.get::<&str, _>("state"), "completed" | "failed") {
                self.require_enabled(CODEX_AGENT_ID)?;
                if row
                    .get::<Option<DateTime<Utc>>, _>("lease_expires_at")
                    .is_none_or(|expiry| expiry <= Utc::now())
                {
                    return Err(ConversationError::ConversationInterrupted);
                }
                if row.get::<DateTime<Utc>, _>("authorization_expires_at") <= Utc::now() {
                    return Err(ConversationError::AuthenticationRequired);
                }
                if row.get::<&str, _>("conversation_state") == "superseded"
                    || row.get::<DateTime<Utc>, _>("expires_at") <= Utc::now()
                {
                    return Err(ConversationError::ConversationStale);
                }
                if row.get::<&str, _>("policy_revision") != self.policy.policy_revision
                    || frozen["guidanceRevision"] != self.policy.guidance_revision
                    || !self.policy.models.iter().any(|model| {
                        frozen["model"] == model.id
                            && model
                                .reasoning_efforts
                                .iter()
                                .any(|effort| frozen["reasoningEffort"] == *effort)
                    })
                {
                    return Err(ConversationError::Forbidden);
                }
                return Ok(ConversationUpdateOutcome {
                    response: prior.get("response"),
                    event: None,
                });
            }
            self.task_authority(&row, Some(lease), false)?;
            return Ok(ConversationUpdateOutcome {
                response: prior.get("response"),
                event: None,
            });
        }
        if let Err(error) = self.task_authority(&row, Some(lease), false) {
            if matches!(row.get::<&str, _>("state"), "reserved" | "running") {
                fail_task(&mut tx, &row, error).await?;
                tx.commit().await?;
            }
            return Err(error);
        }
        let ordinal = request["ordinal"]
            .as_i64()
            .ok_or(ConversationError::InvalidTaskInput)?;
        if ordinal <= row.get::<i64, _>("last_update_ordinal") {
            return Err(ConversationError::DuplicateConflict);
        }
        let kind = request["kind"]
            .as_str()
            .ok_or(ConversationError::InvalidTaskInput)?;
        if ordinal == MAX_SAFE_INTEGER as i64 && !matches!(kind, "completed" | "failed") {
            // A running task must retain a representable terminal successor.
            return Err(ConversationError::InvalidTaskInput);
        }
        let data = &request["data"];
        match kind {
            "settings" => validate_metadata(Some(data), None)?,
            "usage" => validate_metadata(None, Some(data))?,
            "completed" => validate_metadata(
                optional_json(&data["effectiveSettings"]),
                optional_json(&data["usage"]),
            )?,
            _ => {}
        }

        let now = Utc::now();
        let mut event = None;
        let response = if kind == "completed" {
            validate_profile_value("result", data)?;
            if data["conversationId"] != json!(row.get::<Uuid, _>("conversation_id"))
                || data["generation"] != row.get::<i64, _>("generation")
                || data["consumedRevision"] != row.get::<i64, _>("context_revision")
                || data["contextDigest"] != frozen["context"]["digest"]
            {
                return Err(ConversationError::ContextMismatch);
            }
            if data["assistantText"]
                .as_str()
                .ok_or(ConversationError::InvalidTaskInput)?
                .len()
                > 65536
            {
                return Err(ConversationError::ContextTooLarge);
            }
            self.validate_citations(&mut tx, &row, data).await?;
            let reply=append_event_in_transaction(&mut tx,NewEvent{room_id:room,request_id:row.get("request_id"),event_type:"message.created".into(),actor_id:CODEX_AGENT_ID,actor_role:"agent".into(),actor_display_name:Some("Codex Agent".into()),payload:json!({"text":data["assistantText"],"delivery":"room","mentions":[],"audienceIds":[]}),occurred_at:now}).await?;
            let acknowledgement = json!({"profileVersion":PROFILE_VERSION,"taskId":task,"conversationId":row.get::<Uuid,_>("conversation_id"),"generation":row.get::<i64,_>("generation"),"replyEventId":reply.event_id,"replySequence":reply.sequence,"textDigest":format!("{:x}",Sha256::digest(data["assistantText"].as_str().expect("validated text").as_bytes())),"consumedRevision":row.get::<i64,_>("context_revision"),"contextDigest":frozen["context"]["digest"]});
            validate_profile_value("ack", &acknowledgement)?;
            sqlx::query("UPDATE conversation_tasks SET state='completed',result=$2,reply_event_id=$3,receipt_acknowledgement=$4,effective_settings=$5,usage=$6,updated_at=$7 WHERE task_id=$1").bind(task).bind(data).bind(reply.event_id).bind(&acknowledgement).bind(optional_json(&data["effectiveSettings"])).bind(optional_json(&data["usage"])).bind(now).execute(&mut *tx).await?;
            sqlx::query("UPDATE agent_conversations SET state='ready',active_task_id=NULL,consumed_revision=$2,updated_at=$3 WHERE conversation_id=$1").bind(row.get::<Uuid,_>("conversation_id")).bind(row.get::<i64,_>("context_revision")).bind(now).execute(&mut *tx).await?;
            for source in row
                .get::<Value, _>("source_manifest")
                .as_array()
                .ok_or(ConversationError::ContextMismatch)?
            {
                sqlx::query("INSERT INTO conversation_disclosures(conversation_id,room_id,agent_id,generation,source_id,source_kind,first_task_id,policy_revision,disclosed_revision) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9) ON CONFLICT(conversation_id,source_id) DO NOTHING").bind(row.get::<Uuid,_>("conversation_id")).bind(room).bind(CODEX_AGENT_ID).bind(row.get::<i64,_>("generation")).bind(uuid_field(source,"sourceId")?).bind(source["sourceKind"].as_str()).bind(task).bind(&self.policy.policy_revision).bind(row.get::<i64,_>("context_revision")).execute(&mut *tx).await?;
            }
            event = Some(reply);
            acknowledgement
        } else {
            if kind == "failed" {
                let error = failure_code(
                    data["code"]
                        .as_str()
                        .ok_or(ConversationError::InvalidTaskInput)?,
                )?;
                fail_task_state(&mut tx, &row, error).await?;
            }
            if kind == "settings" || kind == "usage" {
                let column = if kind == "settings" {
                    "effective_settings"
                } else {
                    "usage"
                };
                sqlx::query(&format!(
                    "UPDATE conversation_tasks SET {column}=$2,updated_at=$3 WHERE task_id=$1"
                ))
                .bind(task)
                .bind(data)
                .bind(now)
                .execute(&mut *tx)
                .await?;
            }
            json!({"profileVersion":PROFILE_VERSION,"taskId":task,"updateId":update_id,"ordinal":ordinal})
        };
        let acceptance = kind == "progress" && data["phase"] == "accepted";
        let prior_acceptance: bool = if acceptance {
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM conversation_updates WHERE task_id=$1 AND kind='progress' AND data->>'phase'='accepted')").bind(task).fetch_one(&mut *tx).await?
        } else {
            false
        };
        let coalesced = kind == "progress"
            && if acceptance {
                prior_acceptance
            } else {
                row.get::<Option<DateTime<Utc>>, _>("last_progress_at")
                    .is_some_and(|time| now - time < Duration::seconds(1))
            };
        if !coalesced {
            if matches!(kind, "settings" | "usage") {
                sqlx::query("DELETE FROM conversation_updates WHERE task_id=$1 AND kind=$2")
                    .bind(task)
                    .bind(kind)
                    .execute(&mut *tx)
                    .await?;
            }
            if kind == "progress" {
                sqlx::query("DELETE FROM conversation_updates WHERE task_id=$1 AND kind='progress' AND data->>'phase'<>'accepted'").bind(task).execute(&mut *tx).await?;
            }
            sqlx::query("INSERT INTO conversation_updates(task_id,conversation_id,room_id,agent_id,generation,update_id,ordinal,kind,occurred_at,data) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)").bind(task).bind(row.get::<Uuid,_>("conversation_id")).bind(room).bind(CODEX_AGENT_ID).bind(row.get::<i64,_>("generation")).bind(update_id).bind(ordinal).bind(kind).bind(now).bind(data).execute(&mut *tx).await?;
            if kind == "progress" {
                sqlx::query("UPDATE conversation_tasks SET last_progress_at=$2 WHERE task_id=$1")
                    .bind(task)
                    .bind(now)
                    .execute(&mut *tx)
                    .await?;
            }
        }
        sqlx::query("UPDATE conversation_tasks SET last_update_ordinal=$2 WHERE task_id=$1")
            .bind(task)
            .bind(ordinal)
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO conversation_update_requests(task_id,conversation_id,room_id,agent_id,generation,update_id,ordinal,request,response,received_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)").bind(task).bind(row.get::<Uuid,_>("conversation_id")).bind(room).bind(CODEX_AGENT_ID).bind(row.get::<i64,_>("generation")).bind(update_id).bind(ordinal).bind(request).bind(&response).bind(now).execute(&mut *tx).await?;
        self.task_authority(&row, Some(lease), false)?;
        tx.commit().await?;
        Ok(ConversationUpdateOutcome { response, event })
    }

    async fn validate_citations(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        row: &PgRow,
        reply: &Value,
    ) -> Result<(), ConversationError> {
        let mut sources = row
            .get::<Value, _>("source_manifest")
            .as_array()
            .ok_or(ConversationError::ContextMismatch)?
            .clone();
        let cited_ids = reply["citations"]
            .as_array()
            .ok_or(ConversationError::InvalidTaskInput)?
            .iter()
            .map(|id| {
                id.as_str()
                    .and_then(|id| Uuid::parse_str(id).ok())
                    .ok_or(ConversationError::InvalidTaskInput)
            })
            .collect::<Result<Vec<_>, _>>()?;
        for source in sqlx::query("SELECT source_id,source_kind FROM conversation_disclosures WHERE conversation_id=$1 AND policy_revision=$2 AND source_id=ANY($3)").bind(row.get::<Uuid,_>("conversation_id")).bind(&self.policy.policy_revision).bind(cited_ids).fetch_all(&mut **tx).await? {sources.push(json!({"sourceId":source.get::<Uuid,_>("source_id"),"sourceKind":source.get::<&str,_>("source_kind")}));}
        for citation in reply["citations"]
            .as_array()
            .ok_or(ConversationError::InvalidTaskInput)?
        {
            let source = sources
                .iter()
                .find(|source| source["sourceId"] == *citation)
                .ok_or(ConversationError::Forbidden)?;
            let id = uuid_field(source, "sourceId")?;
            let room: Uuid = row.try_get("room_id")?;
            let visible: bool = if source["sourceKind"] == "event" {
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM room_events WHERE event_id=$1 AND room_id=$2 AND (payload->>'delivery' IS NULL OR payload->>'delivery'='room') AND (payload->'audienceIds' IS NULL OR payload->'audienceIds'='[]'::jsonb))").bind(id).bind(room).fetch_one(&mut **tx).await?
            } else {
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM decisions d WHERE d.decision_id=$1 AND d.room_id=$2 AND NOT EXISTS(SELECT 1 FROM unnest(d.source_event_ids) AS source(id) WHERE NOT EXISTS(SELECT 1 FROM room_events e WHERE e.event_id=source.id AND e.room_id=d.room_id AND (e.payload->>'delivery' IS NULL OR e.payload->>'delivery'='room') AND (e.payload->'audienceIds' IS NULL OR e.payload->'audienceIds'='[]'::jsonb))))").bind(id).bind(room).fetch_one(&mut **tx).await?
            };
            if !visible {
                return Err(ConversationError::Forbidden);
            }
        }
        Ok(())
    }
}

fn validate_metadata(
    settings: Option<&Value>,
    usage: Option<&Value>,
) -> Result<(), ConversationError> {
    if settings.is_some_and(|settings| {
        settings["confirmation"] == "confirmed"
            && (settings["model"].is_null() || settings["reasoningEffort"].is_null())
    }) || usage.is_some_and(|usage| {
        usage["modelContextWindow"].is_null() && usage["freshness"] != "unavailable"
    }) {
        return Err(ConversationError::InvalidTaskInput);
    }
    Ok(())
}

fn optional_json(value: &Value) -> Option<&Value> {
    (!value.is_null()).then_some(value)
}
async fn task_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    task: Uuid,
) -> Result<PgRow, ConversationError> {
    sqlx::query("SELECT t.*,c.state AS conversation_state,c.active_task_id FROM conversation_tasks t JOIN agent_conversations c ON c.conversation_id=t.conversation_id AND c.room_id=t.room_id AND c.agent_id=t.agent_id AND c.generation=t.generation WHERE t.task_id=$1 AND t.agent_id=$2 FOR UPDATE OF t,c").bind(task).bind(CODEX_AGENT_ID).fetch_optional(&mut **tx).await?.ok_or(ConversationError::SessionUnavailable)
}
async fn fail_task(
    tx: &mut Transaction<'_, Postgres>,
    row: &PgRow,
    error: ConversationError,
) -> Result<(), ConversationError> {
    fail_task_state(tx, row, error).await?;
    let ordinal = row
        .get::<i64, _>("last_update_ordinal")
        .checked_add(1)
        .filter(|ordinal| *ordinal <= MAX_SAFE_INTEGER as i64)
        .ok_or(ConversationError::ExecutionFailed)?;
    sqlx::query("INSERT INTO conversation_updates(task_id,conversation_id,room_id,agent_id,generation,update_id,ordinal,kind,occurred_at,data) VALUES ($1,$2,$3,$4,$5,$6,$7,'failed',CURRENT_TIMESTAMP,$8)")
      .bind(row.get::<Uuid,_>("task_id")).bind(row.get::<Uuid,_>("conversation_id")).bind(row.get::<Uuid,_>("room_id")).bind(CODEX_AGENT_ID).bind(row.get::<i64,_>("generation")).bind(Uuid::new_v4()).bind(ordinal).bind(json!({"code":error.code()})).execute(&mut **tx).await?;
    sqlx::query("UPDATE conversation_tasks SET last_update_ordinal=$2 WHERE task_id=$1")
        .bind(row.get::<Uuid, _>("task_id"))
        .bind(ordinal)
        .execute(&mut **tx)
        .await?;
    Ok(())
}
async fn fail_task_state(
    tx: &mut Transaction<'_, Postgres>,
    row: &PgRow,
    error: ConversationError,
) -> Result<(), ConversationError> {
    sqlx::query("UPDATE conversation_tasks SET state='failed',failure=$2,updated_at=CURRENT_TIMESTAMP WHERE task_id=$1 AND state IN('reserved','running')").bind(row.get::<Uuid,_>("task_id")).bind(json!({"code":error.code(),"message":error.code()})).execute(&mut **tx).await?;
    sqlx::query("UPDATE agent_conversations SET state='unusable',active_task_id=NULL,updated_at=CURRENT_TIMESTAMP WHERE conversation_id=$1 AND active_task_id=$2").bind(row.get::<Uuid,_>("conversation_id")).bind(row.get::<Uuid,_>("task_id")).execute(&mut **tx).await?;
    Ok(())
}
fn failure_code(code: &str) -> Result<ConversationError, ConversationError> {
    [
        ConversationError::InvalidTaskInput,
        ConversationError::Forbidden,
        ConversationError::ConversationBusy,
        ConversationError::ConversationStale,
        ConversationError::ContextMismatch,
        ConversationError::ContextTooLarge,
        ConversationError::RuntimeUnavailable,
        ConversationError::AuthenticationRequired,
        ConversationError::SessionUnavailable,
        ConversationError::Timeout,
        ConversationError::ConversationInterrupted,
        ConversationError::ExecutionFailed,
        ConversationError::DuplicateConflict,
    ]
    .into_iter()
    .find(|error| error.code() == code)
    .ok_or(ConversationError::InvalidTaskInput)
}

fn uuid_field(value: &Value, key: &str) -> Result<Uuid, ConversationError> {
    value[key]
        .as_str()
        .and_then(|id| Uuid::parse_str(id).ok())
        .ok_or(ConversationError::InvalidTaskInput)
}
