use std::sync::LazyLock;

use chrono::{DateTime, Utc};
use jsonschema::Resource;
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

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

fn parse_pinned_schema(schema: &str) -> Value {
    serde_json::from_str(schema).expect("a checked-in contract schema must be valid JSON")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RpcError {
    pub code: i32,
    pub message: &'static str,
}

impl RpcError {
    const fn invalid_request() -> Self {
        Self {
            code: -32600,
            message: "Invalid request",
        }
    }

    const fn unknown_method() -> Self {
        Self {
            code: -32601,
            message: "Method not found",
        }
    }

    const fn unsupported_contract_version() -> Self {
        Self {
            code: -32009,
            message: "Unsupported contract version",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ValidatedRequest {
    Join(Join),
    ChatSend(ChatSend),
    DecisionPropose(DecisionPropose),
    DecisionTransition(DecisionTransition),
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Join {
    pub id: String,
    pub contract_version: String,
    pub request_id: Uuid,
    pub room_id: Uuid,
    pub occurred_at: DateTime<Utc>,
    pub after_sequence: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChatSend {
    pub id: String,
    pub contract_version: String,
    pub request_id: Uuid,
    pub room_id: Uuid,
    pub occurred_at: DateTime<Utc>,
    pub text: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
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

#[derive(Clone, Debug, Deserialize, PartialEq)]
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

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
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
        "room.join" | "chat.send" | "decision.propose" | "decision.transition"
    ) {
        return Err(RpcError::unknown_method());
    }

    let params = object
        .get("params")
        .and_then(Value::as_object)
        .expect("params was checked above");
    match params.get("contractVersion") {
        Some(Value::String(version)) if version != "n2n.room.v1" => {
            return Err(RpcError::unsupported_contract_version());
        }
        Some(Value::String(_)) => {}
        _ => return Err(RpcError::invalid_request()),
    }

    if !RPC_VALIDATOR.is_valid(&value) {
        return Err(RpcError::invalid_request());
    }

    match method {
        "room.join" => deserialize_request::<Join>(params, id).map(ValidatedRequest::Join),
        "chat.send" => deserialize_request::<ChatSend>(params, id).map(ValidatedRequest::ChatSend),
        "decision.propose" => deserialize_request::<DecisionPropose>(params, id)
            .map(ValidatedRequest::DecisionPropose),
        "decision.transition" => deserialize_request::<DecisionTransition>(params, id)
            .map(ValidatedRequest::DecisionTransition),
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
