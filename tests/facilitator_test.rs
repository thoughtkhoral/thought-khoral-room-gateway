mod support;

use chrono::Utc;
use serde_json::json;
use sqlx::Row;
use thought_khoral_room_gateway::{
    RoomEvent,
    facilitator::{FACILITATOR_ACTOR_ID, propose_from_message},
};
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
        payload: json!({ "text": text }),
        occurred_at: Utc::now(),
    }
}

// This fails if a decision-prefixed persisted message does not retain its provenance,
// is not attributed to the facilitator agent, or creates a non-draft proposal.
#[test]
fn decision_prefixed_message_creates_an_agent_draft_with_source_event() {
    let event = message_event("  Decision: adopt JSON-RPC for room events.  ");

    let proposal = propose_from_message(&event).expect("a decision message must propose a draft");

    assert_eq!(proposal.title, "adopt JSON-RPC for room events.");
    assert_eq!(proposal.summary, "adopt JSON-RPC for room events.");
    assert_eq!(proposal.source_event_ids, vec![event.event_id]);
    assert_eq!(proposal.actor_role, "agent");
}

// This fails if ordinary conversation is accidentally treated as a decision proposal.
#[test]
fn ordinary_message_creates_no_proposal() {
    assert!(propose_from_message(&message_event("We should discuss the room protocol.")).is_none());
}

// This fails if the facilitator uses an unpinned identity, emits an extra authority event,
// or bypasses the human approval boundary by activating or superseding a decision.
#[tokio::test]
async fn facilitator_broadcasts_only_a_draft_proposal_after_the_source_message() {
    let server = TestServer::start().await;
    let room_id = Uuid::new_v4();
    let token = server.token(Uuid::new_v4(), "human");
    let mut socket = server.connect(&token).await;
    join(&mut socket, room_id, None).await;

    let mut params = common_params(Uuid::new_v4(), room_id);
    params["text"] = json!("Decision: adopt JSON-RPC for room events.");
    send_json(&mut socket, rpc("chat", "chat.send", params)).await;

    let source = recv_json(&mut socket).await;
    let proposal = recv_json(&mut socket).await;
    assert_eq!(source["eventType"], "message.created");
    assert_eq!(proposal["eventType"], "decision.proposed");
    assert_eq!(
        proposal["actor"]["id"],
        "6e326e00-0000-0000-0000-000000000001"
    );
    assert_eq!(proposal["actor"]["role"], "agent");
    assert_eq!(proposal["payload"]["status"], "draft");
    assert_eq!(
        proposal["payload"]["sourceEventIds"],
        json!([source["eventId"]])
    );

    let event_types =
        sqlx::query("SELECT event_type FROM room_events WHERE room_id = $1 ORDER BY sequence")
            .bind(room_id)
            .fetch_all(&server.pool)
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.try_get::<String, _>("event_type").unwrap())
            .collect::<Vec<_>>();
    assert_eq!(event_types, vec!["message.created", "decision.proposed"]);

    let statuses = sqlx::query("SELECT status FROM decisions WHERE room_id = $1")
        .bind(room_id)
        .fetch_all(&server.pool)
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.try_get::<String, _>("status").unwrap())
        .collect::<Vec<_>>();
    assert_eq!(statuses, vec!["draft"]);
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
