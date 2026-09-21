use std::{collections::HashSet, sync::LazyLock};

use chrono::{DateTime, Utc};
use jsonschema::Resource;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

pub const MAX_CHAT_MENTIONS: usize = 50;

const ENVELOPE_SCHEMA: &str = include_str!("../contracts/n2n.room.v1/schemas/envelope.schema.json");
const RPC_SCHEMA: &str = include_str!("../contracts/n2n.room.v1/schemas/rpc.schema.json");
const ROOM_EVENT_SCHEMA: &str =
    include_str!("../contracts/n2n.room.v1/schemas/room-event.schema.json");
const ENVELOPE_SCHEMA_ID: &str = "https://n2n.redhat.com/schemas/n2n.room.v1/envelope.schema.json";
const ROOM_EVENT_SCHEMA_ID: &str =
    "https://n2n.redhat.com/schemas/n2n.room.v1/room-event.schema.json";

static RPC_VALIDATOR: LazyLock<jsonschema::Validator> = LazyLock::new(|| {
    let envelope = parse_pinned_schema(ENVELOPE_SCHEMA);
    let room_event = parse_pinned_schema(ROOM_EVENT_SCHEMA);
    let rpc = parse_pinned_schema(RPC_SCHEMA);

    jsonschema::draft202012::options()
        .should_validate_formats(true)
        .with_resources(
            [
                (ENVELOPE_SCHEMA_ID, Resource::from_contents(envelope)),
                (ROOM_EVENT_SCHEMA_ID, Resource::from_contents(room_event)),
            ]
            .into_iter(),
        )
        .build(&rpc)
        .expect("the pinned n2n.room.v1 schemas must compile")
});

static ROOM_EVENT_VALIDATOR: LazyLock<jsonschema::Validator> = LazyLock::new(|| {
    let envelope = parse_pinned_schema(ENVELOPE_SCHEMA);
    let room_event = parse_pinned_schema(ROOM_EVENT_SCHEMA);

    jsonschema::draft202012::options()
        .should_validate_formats(true)
        .with_resource(ENVELOPE_SCHEMA_ID, Resource::from_contents(envelope))
        .build(&room_event)
        .expect("the pinned n2n.room.v1 room-event schema must compile")
});

fn parse_pinned_schema(schema: &str) -> Value {
    serde_json::from_str(schema).expect("a checked-in contract schema must be valid JSON")
}

pub(crate) fn is_valid_room_event(event_id: Uuid, event: &crate::store::NewEvent) -> bool {
    let event = json!({
        "contractVersion": "n2n.room.v1",
        "requestId": event.request_id,
        "roomId": event.room_id,
        "occurredAt": event.occurred_at,
        "sequence": 1,
        "eventId": event_id,
        "eventType": event.event_type,
        "actor": { "id": event.actor_id, "role": event.actor_role },
        "payload": event.payload,
    });
    ROOM_EVENT_VALIDATOR.is_valid(&event) && message_mentions_are_unique(&event)
}

fn message_mentions_are_unique(event: &Value) -> bool {
    if event.get("eventType").and_then(Value::as_str) != Some("message.created") {
        return true;
    }
    let Some(mentions) = event.pointer("/payload/mentions").and_then(Value::as_array) else {
        return true;
    };

    let mut participant_ids = HashSet::new();
    let mut aliases = HashSet::new();
    mentions.iter().all(
        |mention| match mention.get("type").and_then(Value::as_str) {
            Some("participant") => mention
                .get("id")
                .and_then(Value::as_str)
                .is_some_and(|id| participant_ids.insert(id)),
            Some("alias") => mention
                .get("alias")
                .and_then(Value::as_str)
                .is_some_and(|alias| aliases.insert(alias)),
            _ => false,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::is_valid_room_event;
    use chrono::Utc;
    use serde_json::json;
    use uuid::Uuid;

    // This fails if structural schema validation accepts duplicate direct identities with different tokens.
    #[test]
    fn rejects_persisted_message_with_duplicate_participant_identity() {
        let target_id = Uuid::new_v4();
        let event = crate::store::NewEvent {
            room_id: Uuid::new_v4(),
            request_id: Uuid::new_v4(),
            event_type: "message.created".to_owned(),
            actor_id: Uuid::new_v4(),
            actor_role: "human".to_owned(),
            actor_display_name: None,
            payload: json!({
                "text": "targeted message",
                "delivery": "mentioned",
                "mentions": [
                    { "type": "participant", "id": target_id, "token": "maya-chen" },
                    { "type": "participant", "id": target_id, "token": "maya" }
                ],
                "audienceIds": [target_id],
            }),
            occurred_at: Utc::now(),
        };

        assert!(!is_valid_room_event(Uuid::new_v4(), &event));
    }

    #[test]
    fn accepts_a_persisted_action_items_task_event() {
        let agent_id = Uuid::from_u128(0x74686f756768746b_686f72616c000002);
        let event = crate::store::NewEvent {
            room_id: Uuid::new_v4(),
            request_id: Uuid::new_v4(),
            event_type: "agent.task.succeeded".to_owned(),
            actor_id: agent_id,
            actor_role: "agent".to_owned(),
            actor_display_name: None,
            payload: json!({
                "taskId": Uuid::new_v4(),
                "kind": "action-items.v1",
                "sourceEventId": Uuid::new_v4(),
                "requesterId": Uuid::new_v4(),
                "agentId": agent_id,
                "result": { "actionItems": [] },
            }),
            occurred_at: Utc::now(),
        };

        assert!(is_valid_room_event(Uuid::new_v4(), &event));
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RpcError {
    pub code: i32,
    pub message: &'static str,
}

impl RpcError {
    pub(crate) const fn invalid_request() -> Self {
        Self {
            code: -32600,
            message: "Invalid request",
        }
    }

    pub(crate) const fn unknown_method() -> Self {
        Self {
            code: -32601,
            message: "Method not found",
        }
    }

    pub(crate) const fn unsupported_contract_version() -> Self {
        Self {
            code: -32009,
            message: "Unsupported contract version",
        }
    }

    pub(crate) const fn unauthenticated() -> Self {
        Self {
            code: -32001,
            message: "Unauthenticated",
        }
    }

    pub(crate) const fn forbidden() -> Self {
        Self {
            code: -32003,
            message: "Forbidden",
        }
    }

    pub(crate) const fn not_found() -> Self {
        Self {
            code: -32004,
            message: "Room or decision not found",
        }
    }

    pub(crate) const fn invalid_transition() -> Self {
        Self {
            code: -32010,
            message: "Invalid state transition",
        }
    }

    pub(crate) const fn conflicting_duplicate() -> Self {
        Self {
            code: -32012,
            message: "Duplicate request with a different payload",
        }
    }

    pub(crate) const fn unknown_mention_target() -> Self {
        Self {
            code: -32013,
            message: "Unknown message mention target",
        }
    }

    pub(crate) const fn internal_error() -> Self {
        Self {
            code: -32603,
            message: "Internal error",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ValidatedRequest {
    SessionAuthenticate(SessionAuthenticate),
    Join(Join),
    ChatSend(ChatSend),
    DecisionPropose(DecisionPropose),
    DecisionTransition(DecisionTransition),
    DecisionDelete(DecisionDelete),
    AgentTaskStart(crate::store::AgentTaskStart),
}

impl ValidatedRequest {
    pub fn id(&self) -> &str {
        match self {
            Self::SessionAuthenticate(request) => &request.id,
            Self::Join(request) => &request.id,
            Self::ChatSend(request) => &request.id,
            Self::DecisionPropose(request) => &request.id,
            Self::DecisionTransition(request) => &request.id,
            Self::DecisionDelete(request) => &request.id,
            Self::AgentTaskStart(request) => &request.id,
        }
    }

    pub fn room_id(&self) -> Option<Uuid> {
        match self {
            Self::SessionAuthenticate(_) => None,
            Self::Join(request) => Some(request.room_id),
            Self::ChatSend(request) => Some(request.room_id),
            Self::DecisionPropose(request) => Some(request.room_id),
            Self::DecisionTransition(request) => Some(request.room_id),
            Self::DecisionDelete(request) => Some(request.room_id),
            Self::AgentTaskStart(request) => Some(request.room_id),
        }
    }

    pub fn request_id(&self) -> Option<Uuid> {
        match self {
            Self::SessionAuthenticate(_) => None,
            Self::Join(request) => Some(request.request_id),
            Self::ChatSend(request) => Some(request.request_id),
            Self::DecisionPropose(request) => Some(request.request_id),
            Self::DecisionTransition(request) => Some(request.request_id),
            Self::DecisionDelete(request) => Some(request.request_id),
            Self::AgentTaskStart(request) => Some(request.request_id),
        }
    }

    pub(crate) fn fingerprint(&self) -> Value {
        match self {
            Self::SessionAuthenticate(_) => {
                unreachable!("session authentication is not an idempotent room mutation")
            }
            Self::ChatSend(request) => json!({
                "method": "chat.send",
                "contractVersion": request.contract_version,
                "requestId": request.request_id,
                "roomId": request.room_id,
                "occurredAt": request.occurred_at,
                "text": request.text,
                "mentions": request.mentions,
                "delivery": request.delivery,
            }),
            Self::DecisionPropose(request) => json!({
                "method": "decision.propose",
                "contractVersion": request.contract_version,
                "requestId": request.request_id,
                "roomId": request.room_id,
                "occurredAt": request.occurred_at,
                "title": request.title,
                "summary": request.summary,
                "sourceEventIds": request.source_event_ids,
            }),
            Self::DecisionTransition(request) => json!({
                "method": "decision.transition",
                "contractVersion": request.contract_version,
                "requestId": request.request_id,
                "roomId": request.room_id,
                "occurredAt": request.occurred_at,
                "decisionId": request.decision_id,
                "action": request.action,
                "editedTitle": request.edited_title,
                "editedSummary": request.edited_summary,
            }),
            Self::DecisionDelete(request) => json!({
                "method": "decision.delete",
                "contractVersion": request.contract_version,
                "requestId": request.request_id,
                "roomId": request.room_id,
                "occurredAt": request.occurred_at,
                "decisionId": request.decision_id,
            }),
            Self::AgentTaskStart(request) => json!({
                "method": "agent.task.start",
                "contractVersion": request.contract_version,
                "requestId": request.request_id,
                "roomId": request.room_id,
                "occurredAt": request.occurred_at,
                "agentId": request.agent_id,
                "skillId": request.skill_id,
                "input": request.input,
            }),
            Self::Join(request) => json!({
                "method": "room.join",
                "contractVersion": request.contract_version,
                "requestId": request.request_id,
                "roomId": request.room_id,
                "occurredAt": request.occurred_at,
                "afterSequence": request.after_sequence,
            }),
        }
    }
}

#[derive(Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionAuthenticate {
    pub id: String,
    pub access_token: String,
}

impl std::fmt::Debug for SessionAuthenticate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SessionAuthenticate")
            .field("id", &self.id)
            .field("access_token", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Join {
    pub id: String,
    pub contract_version: String,
    pub request_id: Uuid,
    pub room_id: Uuid,
    pub occurred_at: DateTime<Utc>,
    pub after_sequence: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub enum ChatMention {
    Participant { id: Uuid, token: String },
    Alias { alias: ChatMentionAlias },
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ChatMentionAlias {
    AllHumans,
    AllAgents,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ChatDelivery {
    #[default]
    Room,
    Mentioned,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChatSend {
    pub id: String,
    pub contract_version: String,
    pub request_id: Uuid,
    pub room_id: Uuid,
    pub occurred_at: DateTime<Utc>,
    pub text: String,
    #[serde(default)]
    pub mentions: Vec<ChatMention>,
    #[serde(default)]
    pub delivery: ChatDelivery,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DecisionPropose {
    pub id: String,
    pub contract_version: String,
    pub request_id: Uuid,
    pub room_id: Uuid,
    pub occurred_at: DateTime<Utc>,
    pub title: String,
    pub summary: String,
    pub source_event_ids: Vec<Uuid>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DecisionTransition {
    pub id: String,
    pub contract_version: String,
    pub request_id: Uuid,
    pub room_id: Uuid,
    pub occurred_at: DateTime<Utc>,
    pub decision_id: Uuid,
    pub action: DecisionAction,
    pub edited_title: Option<String>,
    pub edited_summary: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DecisionDelete {
    pub id: String,
    pub contract_version: String,
    pub request_id: Uuid,
    pub room_id: Uuid,
    pub occurred_at: DateTime<Utc>,
    pub decision_id: Uuid,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DecisionAction {
    Confirm,
    Edit,
    Dismiss,
}

/// Parses a JSON-RPC request from an untrusted wire payload into a typed v1 request.
///
/// JSON-RPC envelope fields and the application contract version are checked before
/// JSON Schema validation and typed parameter deserialization so callers receive the
/// protocol-specific error codes promised by the contract.
pub fn validate_request(request: &str) -> Result<ValidatedRequest, RpcError> {
    let value: Value = serde_json::from_str(request).map_err(|_| RpcError::invalid_request())?;
    let object = value.as_object().ok_or_else(RpcError::invalid_request)?;

    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
        || object
            .get("id")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        || object.get("params").and_then(Value::as_object).is_none()
    {
        return Err(RpcError::invalid_request());
    }

    let id = object
        .get("id")
        .and_then(Value::as_str)
        .expect("id was checked above");
    let method = object
        .get("method")
        .and_then(Value::as_str)
        .ok_or_else(RpcError::invalid_request)?;
    if !matches!(
        method,
        "session.authenticate"
            | "room.join"
            | "chat.send"
            | "decision.propose"
            | "decision.transition"
            | "decision.delete"
            | "agent.task.start"
    ) {
        return Err(RpcError::unknown_method());
    }

    let params = object
        .get("params")
        .and_then(Value::as_object)
        .expect("params was checked above");
    if method != "session.authenticate" {
        match params.get("contractVersion") {
            Some(Value::String(version)) if version != "n2n.room.v1" => {
                return Err(RpcError::unsupported_contract_version());
            }
            Some(Value::String(_)) => {}
            _ => return Err(RpcError::invalid_request()),
        }
    }

    if !RPC_VALIDATOR.is_valid(&value) {
        return Err(RpcError::invalid_request());
    }

    match method {
        "session.authenticate" => deserialize_request::<SessionAuthenticate>(params, id)
            .map(ValidatedRequest::SessionAuthenticate),
        "room.join" => deserialize_request::<Join>(params, id).map(ValidatedRequest::Join),
        "chat.send" => deserialize_request::<ChatSend>(params, id).map(ValidatedRequest::ChatSend),
        "decision.propose" => deserialize_request::<DecisionPropose>(params, id)
            .map(ValidatedRequest::DecisionPropose),
        "decision.transition" => deserialize_request::<DecisionTransition>(params, id)
            .map(ValidatedRequest::DecisionTransition),
        "decision.delete" => {
            deserialize_request::<DecisionDelete>(params, id).map(ValidatedRequest::DecisionDelete)
        }
        "agent.task.start" => deserialize_request::<crate::store::AgentTaskStart>(params, id)
            .map(ValidatedRequest::AgentTaskStart),
        _ => Err(RpcError::unknown_method()),
    }
}

fn deserialize_request<T>(params: &serde_json::Map<String, Value>, id: &str) -> Result<T, RpcError>
where
    T: for<'de> Deserialize<'de>,
{
    let mut request = params.clone();
    // Typed payloads carry the JSON-RPC id while the contract keeps it at the outer envelope.
    request.insert("id".to_owned(), Value::String(id.to_owned()));
    serde_json::from_value(Value::Object(request)).map_err(|_| RpcError::invalid_request())
}
