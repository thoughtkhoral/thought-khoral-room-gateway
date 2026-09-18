use std::path::Path;
use thought_khoral_room_gateway::{
    ChatDelivery, ChatMention, ChatMentionAlias, MAX_CHAT_MENTIONS, ValidatedRequest,
    gateway_status, validate_request,
};
use uuid::Uuid;

const CHAT_SEND: &str = include_str!("../contracts/n2n.room.v1/fixtures/valid/chat-send.json");
const SESSION_AUTHENTICATE: &str =
    include_str!("../contracts/n2n.room.v1/fixtures/valid/session-authenticate.json");
const BAD_VERSION: &str =
    include_str!("../contracts/n2n.room.v1/fixtures/invalid/bad-version.json");
const MISSING_REQUEST_ID: &str =
    include_str!("../contracts/n2n.room.v1/fixtures/invalid/missing-request-id.json");
const DECISION_DELETE: &str =
    include_str!("../contracts/n2n.room.v1/fixtures/valid/decision-delete.json");
const DECISION_PROPOSE_EMPTY_SOURCES: &str =
    include_str!("../contracts/n2n.room.v1/fixtures/valid/decision-propose-empty-sources.json");
const CHAT_SEND_MENTIONS: &str =
    include_str!("../contracts/n2n.room.v1/fixtures/valid/chat-send-mentions.json");
const CHAT_SEND_ALIASES: &str =
    include_str!("../contracts/n2n.room.v1/fixtures/valid/chat-send-aliases.json");
const CHAT_SEND_TOO_MANY_MENTIONS: &str =
    include_str!("../contracts/n2n.room.v1/fixtures/invalid/chat-send-too-many-mentions.json");
const CHAT_SEND_INVALID_DELIVERY: &str =
    include_str!("../contracts/n2n.room.v1/fixtures/invalid/chat-send-invalid-delivery.json");

// This fails if active package, binary, service, or display metadata regresses to a legacy name.
#[tokio::test]
async fn health_status_identifies_the_thought_khoral_gateway() {
    assert_eq!(env!("CARGO_PKG_NAME"), "thought-khoral-room-gateway");

    let binary = env!("CARGO_BIN_EXE_thought-khoral-room-gateway");
    assert_eq!(
        Path::new(binary).file_name().and_then(|name| name.to_str()),
        Some("thought-khoral-room-gateway")
    );

    let axum::Json(status) = gateway_status().await;
    assert_eq!(status["product"], "ThoughtKhoral");
    assert_eq!(status["service"], "thought-khoral-room-gateway");
}

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
            assert!(chat.mentions.is_empty());
            assert_eq!(chat.delivery, ChatDelivery::Room);
        }
        other => panic!("expected ChatSend, got {other:?}"),
    }
}

// This fails if direct mentions are not retained as typed participant targets.
#[test]
fn validates_chat_send_with_participant_mentions() {
    let request = validate_request(CHAT_SEND_MENTIONS).expect("participant mentions must validate");
    let ValidatedRequest::ChatSend(chat) = request else {
        panic!("expected ChatSend");
    };

    assert_eq!(chat.delivery, ChatDelivery::Mentioned);
    assert_eq!(chat.mentions.len(), 2);
    assert_eq!(
        chat.mentions[0],
        ChatMention::Participant {
            id: Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap(),
            token: "maya-chen".to_owned(),
        }
    );
    assert_eq!(
        chat.mentions[1],
        ChatMention::Participant {
            id: Uuid::parse_str("22222222-2222-4222-8222-222222222222").unwrap(),
            token: "atlas-planner".to_owned(),
        }
    );
}

// This fails if either supported broadcast alias is parsed as an untyped or wrong target.
#[test]
fn validates_chat_send_with_both_alias_mentions() {
    let request = validate_request(CHAT_SEND_ALIASES).expect("alias mentions must validate");
    let ValidatedRequest::ChatSend(chat) = request else {
        panic!("expected ChatSend");
    };

    assert_eq!(chat.delivery, ChatDelivery::Mentioned);
    assert_eq!(
        chat.mentions,
        vec![
            ChatMention::Alias {
                alias: ChatMentionAlias::AllHumans,
            },
            ChatMention::Alias {
                alias: ChatMentionAlias::AllAgents,
            },
        ]
    );
}

// This fails if the mention limit changes or schema validation allows more than 50 targets.
#[test]
fn rejects_chat_send_with_more_than_maximum_mentions() {
    assert_eq!(MAX_CHAT_MENTIONS, 50);
    let error = validate_request(CHAT_SEND_TOO_MANY_MENTIONS)
        .expect_err("a chat request may contain at most 50 mention targets");

    assert_eq!(error.code, -32600);
}

// This fails if an unsupported delivery mode reaches the typed request boundary.
#[test]
fn rejects_chat_send_with_an_invalid_delivery() {
    let error = validate_request(CHAT_SEND_INVALID_DELIVERY)
        .expect_err("only room and mentioned delivery modes are supported");

    assert_eq!(error.code, -32600);
}

#[test]
fn validates_decision_delete_as_a_typed_request() {
    let request = validate_request(DECISION_DELETE).expect("delete fixture must validate");
    let ValidatedRequest::DecisionDelete(delete) = request else {
        panic!("expected DecisionDelete");
    };
    assert_eq!(
        delete.decision_id,
        Uuid::parse_str("cccccccc-cccc-4ccc-8ccc-cccccccccccc").unwrap()
    );
}

#[test]
fn validates_decision_propose_without_source_evidence() {
    let request = validate_request(DECISION_PROPOSE_EMPTY_SOURCES)
        .expect("empty-source proposal fixture must validate");
    let ValidatedRequest::DecisionPropose(propose) = request else {
        panic!("expected DecisionPropose");
    };
    assert!(propose.source_event_ids.is_empty());
}

// This fails if the pinned v1.0.2 authentication request is not consumed as a typed request.
#[test]
fn validates_session_authenticate_as_a_typed_request() {
    let request = validate_request(SESSION_AUTHENTICATE).expect("the tagged fixture must validate");
    assert_eq!(request.room_id(), None);
    assert_eq!(request.request_id(), None);
    let ValidatedRequest::SessionAuthenticate(request) = request else {
        panic!("expected session.authenticate");
    };

    assert_eq!(request.id, "authenticate-1");
    assert_eq!(request.access_token, "header.payload.signature");
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
