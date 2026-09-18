use chrono::Utc;
use serde_json::json;
use thought_khoral_room_gateway::{
    RoomEvent,
    facilitator::{
        FACILITATOR_ACTOR_ID, MemoryEngineDraftProposal, validate_memory_engine_proposal,
    },
};
use uuid::Uuid;

fn source_event(room_id: Uuid) -> RoomEvent {
    RoomEvent {
        event_id: Uuid::new_v4(),
        room_id,
        sequence: 1,
        request_id: Uuid::new_v4(),
        event_type: "message.created".to_owned(),
        actor_id: Uuid::new_v4(),
        actor_role: "human".to_owned(),
        actor_display_name: Some("Human".to_owned()),
        payload: json!({"text": "deployment"}),
        occurred_at: Utc::now(),
    }
}

#[test]
fn gateway_adapter_validates_and_converts_memory_draft() {
    let room_id = Uuid::new_v4();
    let source = source_event(room_id);
    let candidate = MemoryEngineDraftProposal {
        room_id,
        title: "Use blue-green rollout".to_owned(),
        summary: "The room discussed a blue-green rollout.".to_owned(),
        source_event_ids: vec![source.event_id],
    };

    let proposal = validate_memory_engine_proposal(candidate, std::slice::from_ref(&source))
        .expect("memory draft should use the existing facilitator port");
    assert_eq!(proposal.source_event_ids, vec![source.event_id]);
    assert_eq!(proposal.actor_id, FACILITATOR_ACTOR_ID);
    assert_eq!(proposal.actor_role, "agent");
}

#[test]
fn gateway_adapter_rejects_cross_room_memory_draft() {
    let room_id = Uuid::new_v4();
    let source = source_event(Uuid::new_v4());
    let candidate = MemoryEngineDraftProposal {
        room_id,
        title: "wrong room".to_owned(),
        summary: "must be rejected".to_owned(),
        source_event_ids: vec![source.event_id],
    };

    assert!(validate_memory_engine_proposal(candidate, &[source]).is_err());
}

#[test]
fn adapter_has_no_client_visible_proposal_or_transition_method() {
    let facilitator_source = include_str!("../src/facilitator.rs");
    assert!(!facilitator_source.contains("decision.propose"));
    assert!(!facilitator_source.contains("decision.transition"));
}
