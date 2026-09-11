use n2n_room_gateway::{ValidatedRequest, validate_request};
use uuid::Uuid;

const CHAT_SEND: &str = include_str!("../contracts/n2n.room.v1/fixtures/valid/chat-send.json");
const BAD_VERSION: &str =
    include_str!("../contracts/n2n.room.v1/fixtures/invalid/bad-version.json");
const MISSING_REQUEST_ID: &str =
    include_str!("../contracts/n2n.room.v1/fixtures/invalid/missing-request-id.json");

// This fails if the validated boundary stops returning the typed chat payload.
#[test]
fn validates_chat_send_as_a_typed_request() {
    let request = validate_request(CHAT_SEND).expect("the released chat fixture must validate");

    match request {
        ValidatedRequest::ChatSend(chat) => {
            assert_eq!(chat.id, "chat-1");
            assert_eq!(
                chat.request_id,
                Uuid::parse_str("22222222-2222-4222-8222-222222222222").unwrap()
            );
            assert_eq!(chat.text, "Use JSON-RPC for room events.");
        }
        other => panic!("expected ChatSend, got {other:?}"),
    }
}

// This fails if an unsupported contract version is treated as generic malformed input.
#[test]
fn rejects_an_unsupported_contract_version() {
    let error =
        validate_request(BAD_VERSION).expect_err("v2 must not be accepted by the v1 gateway");

    assert_eq!(error.code, -32009);
}

// This fails if a required application-envelope identifier is accepted or misclassified.
#[test]
fn rejects_a_request_without_request_id() {
    let error = validate_request(MISSING_REQUEST_ID)
        .expect_err("requestId is required by the contract envelope");

    assert_eq!(error.code, -32600);
}

// This fails if unsupported methods are handled as a generic invalid request.
#[test]
fn rejects_an_unknown_method() {
    let error = validate_request(
        r#"{
          "jsonrpc":"2.0",
          "id":"unknown-1",
          "method":"chat.delete",
          "params":{}
        }"#,
    )
    .expect_err("unknown JSON-RPC methods must be rejected");

    assert_eq!(error.code, -32601);
}
