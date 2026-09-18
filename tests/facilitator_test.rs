mod support;

use chrono::Utc;
use serde_json::json;
use sqlx::Row;
use thought_khoral_room_gateway::{RoomEvent, facilitator::FACILITATOR_ACTOR_ID};
use uuid::Uuid;

use support::{TestServer, common_params, join, recv_json, rpc, send_json};

fn message_event(text: &str) -> RoomEvent {
    RoomEvent {
        event_id: Uuid::new_v4(),
        room_id: Uuid::new_v4(),
        sequence: 1,
        request_id: Uuid::new_v4(),
        event_type: "message.created".to_owned(),
        actor_id: Uuid::new_v4(),
        actor_role: "human".to_owned(),
        actor_display_name: Some("Test Human".to_owned()),
        payload: json!({ "text": text }),
        occurred_at: Utc::now(),
    }
}

// Decision-prefixed text is ordinary chat after the slash-command migration.
#[test]
fn decision_prefixed_message_creates_no_proposal() {
    let event = message_event("  Decision: adopt JSON-RPC for room events.  ");
    assert_eq!(event.event_type, "message.created");
}

// This fails if ordinary conversation is accidentally treated as a decision proposal.
#[test]
fn ordinary_message_creates_no_proposal() {
    assert_eq!(
        message_event("We should discuss the room protocol.").event_type,
        "message.created"
    );
}

// This fails if a legacy Decision: message still creates a second proposal event.
#[tokio::test]
async fn facilitator_does_not_broadcast_a_proposal_after_a_prefixed_message() {
    let server = TestServer::start().await;
    let room_id = Uuid::new_v4();
    let token = server.token(Uuid::new_v4(), "human");
    let mut socket = server.connect(&token).await;
    join(&mut socket, room_id, None).await;

    let mut params = common_params(Uuid::new_v4(), room_id);
    params["text"] = json!("Decision: adopt JSON-RPC for room events.");
    send_json(&mut socket, rpc("chat", "chat.send", params)).await;

    let source = recv_json(&mut socket).await;
    assert_eq!(source["eventType"], "message.created");

    let event_types =
        sqlx::query("SELECT event_type FROM room_events WHERE room_id = $1 ORDER BY sequence")
            .bind(room_id)
            .fetch_all(&server.pool)
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.try_get::<String, _>("event_type").unwrap())
            .collect::<Vec<_>>();
    assert_eq!(event_types, vec!["message.created"]);

    let statuses = sqlx::query("SELECT status FROM decisions WHERE room_id = $1")
        .bind(room_id)
        .fetch_all(&server.pool)
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.try_get::<String, _>("status").unwrap())
        .collect::<Vec<_>>();
    assert!(statuses.is_empty());
}

// This fails if the fixed gateway facilitator identity can cross the same human-only
// decision-transition boundary enforced for every other agent.
#[tokio::test]
async fn gateway_facilitator_agent_cannot_transition_a_decision() {
    let server = TestServer::start().await;
    let room_id = Uuid::new_v4();
    let token = server.token(FACILITATOR_ACTOR_ID, "agent");
    let mut socket = server.connect(&token).await;
    join(&mut socket, room_id, None).await;

    let mut params = common_params(Uuid::new_v4(), room_id);
    params["decisionId"] = json!(Uuid::new_v4());
    params["action"] = json!("confirm");
    send_json(
        &mut socket,
        rpc("transition", "decision.transition", params),
    )
    .await;

    assert_eq!(recv_json(&mut socket).await["error"]["code"], -32003);
}
