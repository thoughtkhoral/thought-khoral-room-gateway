use crate::{
    Actor, NewEvent,
    conversation_protocol::{
        CODEX_AGENT_ID, ConversationError, MAX_SAFE_INTEGER, PROFILE_VERSION, canonical_bytes,
        validate_profile_value, validate_turn_request,
    },
    rooms::authorize_conversation_turn,
    store::{append_event_in_transaction, lock_room, prior_request, record_request},
};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Row, Transaction};
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
            return accepted.ok_or(ConversationError::DuplicateConflict);
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
        let settings = self.policy.resolve_settings(request.get("settings"))?;
        let task_id = Uuid::new_v4();
        let prompt=append_event_in_transaction(&mut tx, NewEvent {
            room_id:room,request_id,event_type:"message.created".into(),actor_id:actor.id,actor_role:"human".into(),actor_display_name:Some(actor.display_name.clone()),
            payload:json!({"text":request["text"],"mentions":request["mentions"],"delivery":"room","audienceIds":[]}),
            occurred_at:DateTime::parse_from_rfc3339(request["occurredAt"].as_str().ok_or(ConversationError::InvalidTaskInput)?).map_err(|_| ConversationError::InvalidTaskInput)?.with_timezone(&Utc),
        }).await?;
        if prompt.sequence as u64 > MAX_SAFE_INTEGER {
            return Err(ConversationError::ContextMismatch);
        }
        let mut context = freeze_context(
            &mut tx,
            room,
            conversation_id,
            generation,
            base,
            prompt.sequence,
            &self.policy,
        )
        .await?;
        let preimage = json!({"roomId":room,"agentId":CODEX_AGENT_ID,"conversationId":conversation_id,"generation":generation,"triggerEventId":prompt.event_id,"guidanceRevision":self.policy.guidance_revision,"context":context});
        let bytes = canonical_bytes(&preimage)?;
        if bytes.len() > self.policy.max_context_bytes {
            return Err(ConversationError::ContextTooLarge);
        }
        context["digest"] = json!(format!("{:x}", Sha256::digest(bytes)));
        let expires = now + Duration::seconds(self.policy.turn_deadline_seconds);
        let frozen = json!({"profileVersion":PROFILE_VERSION,"taskId":task_id,"roomId":room,"requesterId":actor.id,"agentId":CODEX_AGENT_ID,"skillId":"chat","conversation":{"id":conversation_id,"generation":generation,"mode":mode},"triggerEventId":prompt.event_id,"context":context,"guidanceRevision":self.policy.guidance_revision,"issuedAt":now,"expiresAt":expires,"authorizationExpiresAt":authorization_expires_at,"model":settings["model"],"reasoningEffort":settings["reasoningEffort"],"catalogRevision":settings["catalogRevision"]});
        validate_profile_value("reserved-input", &frozen)?;
        let accepted = json!({"profileVersion":PROFILE_VERSION,"roomId":room,"taskId":task_id,"conversationId":conversation_id,"generation":generation,"triggerEventId":prompt.event_id,"contextRevision":prompt.sequence,"selectedSettings":settings});
        validate_profile_value("task", &accepted)?;
        let sources = source_manifest(&context);
        record_request(&mut tx, room, request_id, fingerprint, &[prompt]).await?;
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
        Ok(accepted)
    }
}

fn uuid_field(value: &Value, key: &str) -> Result<Uuid, ConversationError> {
    value[key]
        .as_str()
        .and_then(|id| Uuid::parse_str(id).ok())
        .ok_or(ConversationError::InvalidTaskInput)
}

// Only messages with room delivery and no targeted audience are disclosable.
// Legacy room-wide records omitted delivery/audienceIds; they remain public.
async fn freeze_context(
    tx: &mut Transaction<'_, Postgres>,
    room: Uuid,
    conversation: Uuid,
    generation: i64,
    base: i64,
    revision: i64,
    policy: &ConversationPolicy,
) -> Result<Value, ConversationError> {
    // This is only a lower bound of disclosed fields. Mentions/delivery metadata
    // are excluded from context. Candidate native replies become verified bindings.
    let totals=sqlx::query("SELECT count(*) AS count, COALESCE(sum(CASE WHEN $2>0 AND e.actor_id=$6 AND e.actor_role='agent' AND t.receipt_acknowledgement IS NOT NULL THEN 0 ELSE octet_length(e.payload->>'text') + COALESCE(octet_length(e.actor_display_name),0) END),0)::bigint AS bytes, COALESCE(bool_or($2>0 AND e.actor_id=$6 AND e.actor_role='agent' AND t.receipt_acknowledgement IS NOT NULL AND octet_length(e.payload->>'text')>65536),false) AS oversized_reply FROM room_events e LEFT JOIN conversation_tasks t ON t.reply_event_id=e.event_id AND t.room_id=e.room_id AND t.conversation_id=$4 AND t.generation=$5 AND t.state='completed' WHERE e.room_id=$1 AND e.sequence>$2 AND e.sequence<=$3 AND e.event_type='message.created' AND (e.payload->>'delivery' IS NULL OR e.payload->>'delivery'='room') AND (e.payload->'audienceIds' IS NULL OR e.payload->'audienceIds'='[]'::jsonb)")
        .bind(room).bind(base).bind(revision).bind(conversation).bind(generation).bind(CODEX_AGENT_ID).fetch_one(&mut **tx).await?;
    if totals.get::<bool, _>("oversized_reply") {
        return Err(ConversationError::ContextMismatch);
    }
    if totals.get::<i64, _>("count") > policy.max_records as i64
        || totals.get::<i64, _>("bytes") > policy.max_context_bytes as i64
    {
        return Err(ConversationError::ContextTooLarge);
    }
    let rows=sqlx::query("SELECT e.event_id,e.sequence,e.actor_id,e.actor_role,e.actor_display_name,e.occurred_at,e.payload->>'text' AS text,t.task_id AS source_task_id,t.receipt_acknowledgement,t.context_revision AS source_revision,t.frozen_input->'context'->>'digest' AS source_context_digest FROM room_events e LEFT JOIN conversation_tasks t ON t.reply_event_id=e.event_id AND t.room_id=e.room_id AND t.conversation_id=$4 AND t.generation=$5 AND t.state='completed' WHERE e.room_id=$1 AND e.sequence>$2 AND e.sequence<=$3 AND e.event_type='message.created' AND (e.payload->>'delivery' IS NULL OR e.payload->>'delivery'='room') AND (e.payload->'audienceIds' IS NULL OR e.payload->'audienceIds'='[]'::jsonb) ORDER BY e.sequence")
        .bind(room).bind(base).bind(revision).bind(conversation).bind(generation).fetch_all(&mut **tx).await?;
    let mut entries = vec![];
    let mut bindings = vec![];
    for row in rows {
        let id: Uuid = row.try_get("event_id")?;
        let sequence: i64 = row.try_get("sequence")?;
        let text: Option<String> = row.try_get("text")?;
        let text = text
            .as_deref()
            .filter(|text| !text.is_empty())
            .ok_or(ConversationError::ContextMismatch)?;
        let author: Uuid = row.try_get("actor_id")?;
        let acknowledgement: Option<Value> = row.try_get("receipt_acknowledgement")?;
        if base > 0
            && author == CODEX_AGENT_ID
            && row.get::<&str, _>("actor_role") == "agent"
            && let Some(ack) = acknowledgement
        {
            validate_profile_value("ack", &ack).map_err(|_| ConversationError::ContextMismatch)?;
            let source_task: Uuid = row.try_get("source_task_id")?;
            let source_revision: i64 = row.try_get("source_revision")?;
            let source_digest: String = row.try_get("source_context_digest")?;
            let text_digest = format!("{:x}", Sha256::digest(text.as_bytes()));
            if ack["taskId"] != json!(source_task)
                || ack["conversationId"] != json!(conversation)
                || ack["generation"] != generation
                || ack["replyEventId"] != json!(id)
                || ack["replySequence"] != sequence
                || ack["textDigest"] != text_digest
                || ack["consumedRevision"] != source_revision
                || source_revision > base
                || ack["contextDigest"] != source_digest
            {
                return Err(ConversationError::ContextMismatch);
            }
            bindings.push(json!({"eventId":id,"sequence":sequence,"sourceTaskId":source_task,"generation":generation,"textDigest":text_digest}));
        } else {
            let mut entry = json!({"eventId":id,"sequence":sequence,"authorId":author,"authorRole":row.get::<&str,_>("actor_role"),"occurredAt":row.get::<DateTime<Utc>,_>("occurred_at"),"text":text});
            if let Some(name) = row.try_get::<Option<String>, _>("actor_display_name")? {
                entry["displayName"] = json!(name);
            }
            entries.push(entry);
        }
    }
    // Decisions with missing or targeted sources are excluded, including sources
    // outside this room. Both the size preflight and projection use the same filter.
    const DECISIONS: &str = "SELECT d.* FROM decisions d WHERE d.room_id=$1 AND d.status='active' AND NOT EXISTS (SELECT 1 FROM unnest(d.source_event_ids) AS source(id) WHERE NOT EXISTS (SELECT 1 FROM room_events e WHERE e.event_id=source.id AND e.room_id=d.room_id AND e.sequence<=$2 AND (e.payload->>'delivery' IS NULL OR e.payload->>'delivery'='room') AND (e.payload->'audienceIds' IS NULL OR e.payload->'audienceIds'='[]'::jsonb)))";
    let size_sql = format!(
        "SELECT COALESCE(sum(octet_length(title)+octet_length(summary)+cardinality(source_event_ids)*39+80),0)::bigint FROM ({DECISIONS}) AS visible"
    );
    let decision_bytes: i64 = sqlx::query_scalar(&size_sql)
        .bind(room)
        .bind(revision)
        .fetch_one(&mut **tx)
        .await?;
    if decision_bytes > policy.max_context_bytes as i64 {
        return Err(ConversationError::ContextTooLarge);
    }
    let decision_sql = format!("{DECISIONS} ORDER BY d.decision_id");
    let decisions = sqlx::query(&decision_sql)
        .bind(room)
        .bind(revision)
        .fetch_all(&mut **tx)
        .await?;
    let mut active = vec![];
    for row in decisions {
        active.push(json!({"decisionId":row.get::<Uuid,_>("decision_id"),"title":row.get::<String,_>("title"),"summary":row.get::<String,_>("summary"),"sourceEventIds":row.get::<Vec<Uuid>,_>("source_event_ids")}));
    }
    Ok(
        json!({"kind":if base==0 {"baseline"} else {"delta"},"baseRevision":base,"revision":revision,"policyRevision":policy.policy_revision,"entries":entries,"activeDecisions":active,"nativeReplyBindings":bindings}),
    )
}

fn source_manifest(context: &Value) -> Value {
    let mut ids = HashSet::new();
    let mut sources = vec![];
    for field in ["entries", "nativeReplyBindings"] {
        for entry in context[field].as_array().expect("constructed entries") {
            let id = entry["eventId"].clone();
            if ids.insert(id.as_str().expect("constructed UUID").to_owned()) {
                sources.push(json!({"sourceId":id,"sourceKind":"event"}));
            }
        }
    }
    for decision in context["activeDecisions"]
        .as_array()
        .expect("constructed decisions")
    {
        let id = decision["decisionId"].clone();
        if ids.insert(id.as_str().expect("constructed UUID").to_owned()) {
            sources.push(json!({"sourceId":id,"sourceKind":"decision"}));
        }
        for id in decision["sourceEventIds"]
            .as_array()
            .expect("constructed sources")
        {
            if ids.insert(id.as_str().expect("constructed UUID").to_owned()) {
                sources.push(json!({"sourceId":id,"sourceKind":"event"}));
            }
        }
    }
    Value::Array(sources)
}
