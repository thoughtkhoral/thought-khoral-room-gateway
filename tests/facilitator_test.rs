mod support;

use chrono::Utc;
use n2n_room_gateway::{RoomEvent, facilitator::propose_from_message};
use serde_json::json;
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

// This fails if the facilitator emits something other than a draft proposal or bypasses
// the human approval boundary by activating a decision.
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
    assert_eq!(proposal["actor"]["role"], "agent");
    assert_eq!(proposal["payload"]["status"], "draft");
    assert_eq!(
        proposal["payload"]["sourceEventIds"],
        json!([source["eventId"]])
    );
}
