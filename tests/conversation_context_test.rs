use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use thought_khoral_room_gateway::conversation_context::canonical_context_bytes;

// These independent published literals detect a changed key order, escaping, or Unicode encoding.
#[test]
fn unicode_vector_has_the_published_digest() {
    let bytes = canonical_context_bytes(&json!({"z":"é","a":[1,"x\n"]})).unwrap();
    assert_eq!(bytes, "{\"a\":[1,\"x\\n\"],\"z\":\"é\"}".as_bytes());
    assert_eq!(
        format!("{:x}", Sha256::digest(bytes)),
        "089204610aca5bd0285b6c1285dee671b94a7afe21bf9af07f0a9a1b8a704a94"
    );
}

#[test]
fn bound_baseline_has_the_published_digest() {
    let packet: Value = serde_json::from_str(include_str!(
        "../contracts/agent-conversation-v1/fixtures/valid/baseline-hidden-sequence-gap.json"
    ))
    .unwrap();
    let mut context = packet["context"].clone();
    context.as_object_mut().unwrap().remove("digest");
    let preimage = json!({"roomId":packet["roomId"],"agentId":packet["agentId"],"conversationId":packet["conversation"]["id"],"generation":packet["conversation"]["generation"],"triggerEventId":packet["triggerEventId"],"guidanceRevision":packet["guidanceRevision"],"context":context});
    assert_eq!(
        format!(
            "{:x}",
            Sha256::digest(canonical_context_bytes(&preimage).unwrap())
        ),
        "d72f81e3da10a0b3f0c9f1bafc32bc1f6afea94875ab0e7ee2b0d6f31c157e62"
    );
}

#[test]
fn canonicalization_rejects_unsafe_numbers_and_non_schema_keys() {
    for value in [
        json!({"number":1.0}),
        json!({"number":-1}),
        json!({"number":9007199254740992u64}),
        json!({"é":"non-ASCII key"}),
    ] {
        assert!(canonical_context_bytes(&value).is_err());
    }
}
