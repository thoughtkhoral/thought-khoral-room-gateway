use std::{
    collections::{HashMap, HashSet},
    sync::LazyLock,
};

use jsonschema::Resource;
use serde_json::Value;

pub const PROFILE_VERSION: &str = "thought-khoral.agent-conversation.v1";
pub const CODEX_AGENT_ID: uuid::Uuid = uuid::Uuid::from_u128(0x74686f756768746b_686f72616c000004);
pub const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

const SCHEMAS: &[(&str, &str)] = &[
    (
        "turn",
        include_str!("../contracts/agent-conversation-v1/schemas/turn.schema.json"),
    ),
    (
        "input",
        include_str!("../contracts/agent-conversation-v1/schemas/input.schema.json"),
    ),
    (
        "task",
        include_str!("../contracts/agent-conversation-v1/schemas/task.schema.json"),
    ),
    (
        "view",
        include_str!("../contracts/agent-conversation-v1/schemas/view.schema.json"),
    ),
    (
        "ack",
        include_str!("../contracts/agent-conversation-v1/schemas/ack.schema.json"),
    ),
    (
        "result",
        include_str!("../contracts/agent-conversation-v1/schemas/result.schema.json"),
    ),
    (
        "catalog",
        include_str!("../contracts/agent-conversation-v1/schemas/catalog.schema.json"),
    ),
    (
        "update",
        include_str!("../contracts/agent-conversation-v1/schemas/update.schema.json"),
    ),
    (
        "error",
        include_str!("../contracts/agent-conversation-v1/schemas/error.schema.json"),
    ),
];

static VALIDATORS: LazyLock<HashMap<&'static str, jsonschema::Validator>> = LazyLock::new(|| {
    let schemas: Vec<(&str, Value)> = SCHEMAS
        .iter()
        .map(|(name, schema)| {
            (
                *name,
                serde_json::from_str(schema).expect("pinned schema JSON"),
            )
        })
        .collect();
    let mut validators: HashMap<_, _> = schemas
        .iter()
        .map(|(name, schema)| {
            let resources = schemas.iter().map(|(_, value)| {
                (
                    value["$id"].as_str().expect("pinned schema ID").to_owned(),
                    Resource::from_contents(value.clone()),
                )
            });
            let validator = jsonschema::draft202012::options()
                .should_validate_formats(true)
                .with_resources(resources)
                .build(schema)
                .expect("pinned profile schemas compile offline");
            (*name, validator)
        })
        .collect();
    let mut reserved_schema = schemas
        .iter()
        .find(|(name, _)| *name == "input")
        .expect("input schema")
        .1
        .clone();
    for key in ["leaseOwner", "leaseExpiresAt"] {
        reserved_schema["properties"]
            .as_object_mut()
            .expect("properties")
            .remove(key);
    }
    reserved_schema["required"]
        .as_array_mut()
        .expect("required")
        .retain(|key| key != "leaseOwner" && key != "leaseExpiresAt");
    let resources = schemas.iter().map(|(_, value)| {
        (
            value["$id"].as_str().expect("schema ID").to_owned(),
            Resource::from_contents(value.clone()),
        )
    });
    validators.insert(
        "reserved-input",
        jsonschema::draft202012::options()
            .should_validate_formats(true)
            .with_resources(resources)
            .build(&reserved_schema)
            .expect("reserved core schema compiles"),
    );
    validators
});

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConversationError {
    InvalidTaskInput,
    Forbidden,
    ConversationBusy,
    ConversationStale,
    ContextMismatch,
    ContextTooLarge,
    RuntimeUnavailable,
    AuthenticationRequired,
    SessionUnavailable,
    Timeout,
    ConversationInterrupted,
    ExecutionFailed,
    DuplicateConflict,
}

impl ConversationError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidTaskInput => "invalid_task_input",
            Self::Forbidden => "forbidden",
            Self::ConversationBusy => "conversation_busy",
            Self::ConversationStale => "conversation_stale",
            Self::ContextMismatch => "context_mismatch",
            Self::ContextTooLarge => "context_too_large",
            Self::RuntimeUnavailable => "runtime_unavailable",
            Self::AuthenticationRequired => "authentication_required",
            Self::SessionUnavailable => "session_unavailable",
            Self::Timeout => "timeout",
            Self::ConversationInterrupted => "conversation_interrupted",
            Self::ExecutionFailed => "execution_failed",
            Self::DuplicateConflict => "duplicate_conflict",
        }
    }
}

impl std::fmt::Display for ConversationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}
impl std::error::Error for ConversationError {}
impl From<sqlx::Error> for ConversationError {
    fn from(_: sqlx::Error) -> Self {
        Self::RuntimeUnavailable
    }
}
impl From<crate::StoreError> for ConversationError {
    fn from(_: crate::StoreError) -> Self {
        Self::RuntimeUnavailable
    }
}

pub(crate) fn validate_profile_value(name: &str, value: &Value) -> Result<(), ConversationError> {
    if safe_numbers(value)
        && VALIDATORS
            .get(name)
            .is_some_and(|validator| validator.is_valid(value))
    {
        Ok(())
    } else {
        Err(ConversationError::InvalidTaskInput)
    }
}

fn safe_numbers(value: &Value) -> bool {
    match value {
        Value::Number(n) => n.as_u64().is_some_and(|n| n <= MAX_SAFE_INTEGER),
        Value::Array(values) => values.iter().all(safe_numbers),
        Value::Object(values) => values.values().all(safe_numbers),
        _ => true,
    }
}

pub fn validate_turn_request(request: &Value) -> Result<Value, ConversationError> {
    validate_profile_value("turn", request)?;
    if request["agentId"] != CODEX_AGENT_ID.to_string() {
        return Err(ConversationError::InvalidTaskInput);
    }
    let mut participants = HashSet::new();
    let mut aliases = HashSet::new();
    let mut direct_codex = false;
    for mention in request["mentions"]
        .as_array()
        .ok_or(ConversationError::InvalidTaskInput)?
    {
        if mention["type"] == "participant" {
            let id = mention["id"]
                .as_str()
                .ok_or(ConversationError::InvalidTaskInput)?;
            if !participants.insert(id) {
                return Err(ConversationError::InvalidTaskInput);
            }
            if mention["id"] == request["agentId"] {
                if mention["token"] != "codex-agent" {
                    return Err(ConversationError::InvalidTaskInput);
                }
                direct_codex = true;
            }
        } else if !aliases.insert(mention["alias"].as_str()) {
            return Err(ConversationError::InvalidTaskInput);
        }
    }
    if !direct_codex {
        return Err(ConversationError::InvalidTaskInput);
    }
    Ok(request.clone())
}

#[cfg(test)]
fn canonical_bytes(value: &Value) -> Result<Vec<u8>, ConversationError> {
    crate::conversation_context::canonical_context_bytes(value)
}

#[cfg(test)]
mod tests {
    use super::canonical_bytes;
    use serde_json::json;
    use sha2::{Digest, Sha256};
    #[test]
    fn canonical_unicode_bytes_match_the_published_independent_vector() {
        let bytes = canonical_bytes(&json!({"z":"é","a":[1,"x\n"]})).unwrap();
        assert_eq!(bytes, "{\"a\":[1,\"x\\n\"],\"z\":\"é\"}".as_bytes());
        assert_eq!(
            format!("{:x}", Sha256::digest(bytes)),
            "089204610aca5bd0285b6c1285dee671b94a7afe21bf9af07f0a9a1b8a704a94"
        );
    }
}
