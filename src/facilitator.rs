use uuid::Uuid;

use crate::store::RoomEvent;

/// The gateway-owned identity recorded on deterministic facilitator proposals.
pub const FACILITATOR_ACTOR_ID: Uuid = Uuid::from_u128(0x6e326e00_0000_0000_0000_000000000001);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewDecisionProposal {
    pub title: String,
    pub summary: String,
    pub source_event_ids: Vec<Uuid>,
    pub actor_id: Uuid,
    pub actor_role: String,
}

/// Derives one draft-only proposal from an already-persisted decision message.
///
/// This boundary is intentionally pure: it has no transition capability and makes
/// no model, tool, operating-system, or database call.
pub fn propose_from_message(event: &RoomEvent) -> Option<NewDecisionProposal> {
    if event.event_type != "message.created" {
        return None;
    }
    let text = event.payload.get("text")?.as_str()?.trim();
    let title = text.strip_prefix("Decision:")?.trim();
    if title.is_empty() {
        return None;
    }

    Some(NewDecisionProposal {
        title: title.to_owned(),
        summary: title.to_owned(),
        source_event_ids: vec![event.event_id],
        actor_id: FACILITATOR_ACTOR_ID,
        actor_role: "agent".to_owned(),
    })
}
