use serde_json::{Value, json};
use thought_khoral_room_gateway::conversation_protocol::{
    ConversationError, validate_turn_request,
};

fn first_turn() -> Value {
    serde_json::from_str(include_str!(
        "../contracts/agent-conversation-v1/fixtures/valid/first-turn.json"
    ))
    .unwrap()
}

// Removing strict schema/identity checks would admit client authority or ambiguous invocation.
#[test]
fn accepts_the_published_first_turn() {
    let request = first_turn();
    assert_eq!(validate_turn_request(&request).unwrap(), request);
}

#[test]
fn rejects_client_authority_and_unknown_fields() {
    for field in [
        "delivery",
        "requesterId",
        "threadId",
        "endpoint",
        "directory",
    ] {
        let mut request = first_turn();
        request[field] = json!("client supplied");
        assert_eq!(
            validate_turn_request(&request),
            Err(ConversationError::InvalidTaskInput),
            "{field}"
        );
    }
}

#[test]
fn requires_one_canonical_direct_codex_identity() {
    let mut request = first_turn();
    request["agentId"] = json!("11111111-1111-4111-8111-111111111111");
    assert!(validate_turn_request(&request).is_err());
    request = first_turn();
    request["mentions"][0]["token"] = json!("codex");
    assert!(validate_turn_request(&request).is_err());
    request = first_turn();
    let agent_id = request["agentId"].clone();
    request["mentions"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type":"participant", "id":agent_id, "token":"codex"}));
    assert!(validate_turn_request(&request).is_err());
    request = first_turn();
    request["mentions"] = json!([{"type":"alias", "alias":"allagents"}]);
    assert!(validate_turn_request(&request).is_err());
}

#[test]
fn counts_unicode_scalars_and_rejects_float_generations() {
    let mut request = first_turn();
    request["text"] = json!("🦀".repeat(8000));
    assert!(validate_turn_request(&request).is_ok());
    request["text"] = json!("🦀".repeat(8001));
    assert!(validate_turn_request(&request).is_err());
    request = first_turn();
    request["conversation"] = json!({"mode":"continue","id":request["requestId"],"generation":1.0});
    assert!(validate_turn_request(&request).is_err());
}

#[test]
fn duplicate_aliases_and_partial_settings_fail() {
    let mut request = first_turn();
    request["mentions"].as_array_mut().unwrap().extend([
        json!({"type":"alias","alias":"allhumans"}),
        json!({"type":"alias","alias":"allhumans"}),
    ]);
    assert!(validate_turn_request(&request).is_err());
    request = first_turn();
    request["settings"] = json!({"model":"model-a"});
    assert!(validate_turn_request(&request).is_err());
}

#[test]
fn pinned_stateless_turn_fixtures_match_the_rust_validator() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("contracts/agent-conversation-v1/fixtures");
    let manifest: Vec<Value> =
        serde_json::from_slice(&std::fs::read(root.join("manifest.json")).unwrap()).unwrap();
    for case in manifest {
        // Mutable broker state belongs to store tests. Raw duplicate-key checking
        // belongs to Task 3's HTTP decoder, because Value has already lost keys.
        if case["schema"] != "turn"
            || case.get("context").is_some()
            || case["reason"] == "json-duplicate"
        {
            continue;
        }
        let bytes = std::fs::read(root.join(case["path"].as_str().unwrap())).unwrap();
        let valid = serde_json::from_slice::<Value>(&bytes)
            .is_ok_and(|value| validate_turn_request(&value).is_ok());
        assert_eq!(valid, case["valid"].as_bool().unwrap(), "{}", case["path"]);
    }
}

#[test]
fn vendored_files_match_the_published_artifact_lock() {
    use sha2::{Digest, Sha256};
    let root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("contracts/agent-conversation-v1");
    let lock: Value =
        serde_json::from_slice(&std::fs::read(root.join("lock.json")).unwrap()).unwrap();
    assert_eq!(lock["commit"], "85baf86e574276fcd036e53e23641af6aad602f9");
    assert_eq!(
        lock["archiveSha256"],
        "0038fdbf858db013c4a269aeedbca51a5db9128a2f1d6e39754f92ba60fd8f36"
    );
    for (path, digest) in lock["files"].as_object().unwrap() {
        assert_eq!(
            format!(
                "{:x}",
                Sha256::digest(std::fs::read(root.join(path)).unwrap())
            ),
            digest.as_str().unwrap(),
            "{path}"
        );
    }
}
