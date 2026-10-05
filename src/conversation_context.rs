use crate::{
    conversation_protocol::{
        CODEX_AGENT_ID, ConversationError, MAX_SAFE_INTEGER, validate_profile_value,
    },
    conversation_store::ConversationPolicy,
    store::lock_room,
};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Row, Transaction};
use std::collections::HashSet;
use uuid::Uuid;

/// Canonical UTF-8 preimage bytes, with sorted ASCII keys and safe integer values.
pub fn canonical_context_bytes(value: &Value) -> Result<Vec<u8>, ConversationError> {
    fn sorted(value: &Value) -> Result<Value, ConversationError> {
        match value {
            Value::Number(n) if n.as_u64().is_none_or(|n| n > MAX_SAFE_INTEGER) => {
                Err(ConversationError::ContextMismatch)
            }
            Value::Object(map) => {
                if map.keys().any(|key| !key.is_ascii()) {
                    return Err(ConversationError::ContextMismatch);
                }
                let mut keys: Vec<_> = map.keys().collect();
                keys.sort_unstable();
                Ok(Value::Object(
                    keys.into_iter()
                        .map(|key| Ok((key.clone(), sorted(&map[key])?)))
                        .collect::<Result<_, ConversationError>>()?,
                ))
            }
            Value::Array(values) => Ok(Value::Array(
                values.iter().map(sorted).collect::<Result<_, _>>()?,
            )),
            _ => Ok(value.clone()),
        }
    }
    serde_json::to_vec(&sorted(value)?).map_err(|_| ConversationError::ContextMismatch)
}

/// Build only while holding the room transaction. Never use this to reconstruct
/// a claimed task: claims deliver the already persisted frozen core.
pub async fn build_context(
    tx: &mut Transaction<'_, Postgres>,
    room: Uuid,
    conversation: Uuid,
    generation: i64,
    base: i64,
    trigger: Uuid,
    policy: &ConversationPolicy,
) -> Result<Value, ConversationError> {
    lock_room(tx, room).await?;
    let revision:Option<i64>=sqlx::query_scalar("SELECT sequence FROM room_events WHERE event_id=$1 AND room_id=$2 AND event_type='message.created' AND actor_role='human' AND payload->>'delivery'='room' AND payload->'audienceIds'='[]'::jsonb")
        .bind(trigger).bind(room).fetch_optional(&mut **tx).await?;
    let revision = revision.ok_or(ConversationError::ContextMismatch)?;
    if base < 0
        || base >= revision
        || generation <= 0
        || generation as u64 > MAX_SAFE_INTEGER
        || revision as u64 > MAX_SAFE_INTEGER
    {
        return Err(ConversationError::ContextMismatch);
    }
    let mut context =
        freeze_context(tx, room, conversation, generation, base, revision, policy).await?;
    let preimage = json!({"roomId":room,"agentId":CODEX_AGENT_ID,"conversationId":conversation,"generation":generation,"triggerEventId":trigger,"guidanceRevision":policy.guidance_revision,"context":context});
    let bytes = canonical_context_bytes(&preimage)?;
    if bytes.len() > policy.max_context_bytes {
        return Err(ConversationError::ContextTooLarge);
    }
    context["digest"] = json!(format!("{:x}", Sha256::digest(bytes)));
    Ok(context)
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

pub(crate) fn source_manifest(context: &Value) -> Value {
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
