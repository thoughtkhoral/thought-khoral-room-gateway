use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

use crate::store::RoomEvent;

/// The gateway-owned identity recorded on deterministic facilitator proposals.
pub const FACILITATOR_ACTOR_ID: Uuid = Uuid::from_u128(0x6e326e00_0000_0000_0000_000000000001);
pub const FACILITATOR_DISPLAY_NAME: &str = "ThoughtKhoral Facilitator";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewDecisionProposal {
    pub title: String,
    pub summary: String,
    pub source_event_ids: Vec<Uuid>,
    pub actor_id: Uuid,
    pub actor_role: String,
    pub actor_display_name: String,
}

/// Wire-compatible draft returned by the inactive memory-engine adapter.
/// This remains a draft shape and carries no transition/status field.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct MemoryEngineDraftProposal {
    pub room_id: Uuid,
    pub title: String,
    pub summary: String,
    pub source_event_ids: Vec<Uuid>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MemoryProposalValidationError {
    EmptyRoom,
    EmptyTitle,
    EmptySummary,
    MissingSourceEvent(Uuid),
    SourceEventFromAnotherRoom,
    DuplicateSourceEvent(Uuid),
}

/// Validates a memory-engine draft and converts it into the existing gateway
/// facilitator proposal shape. It does not persist, transition, or broadcast.
pub fn validate_memory_engine_proposal(
    candidate: MemoryEngineDraftProposal,
    source_events: &[RoomEvent],
) -> Result<NewDecisionProposal, MemoryProposalValidationError> {
    if candidate.room_id.is_nil() {
        return Err(MemoryProposalValidationError::EmptyRoom);
    }
    if candidate.title.trim().is_empty() {
        return Err(MemoryProposalValidationError::EmptyTitle);
    }
    if candidate.summary.trim().is_empty() {
        return Err(MemoryProposalValidationError::EmptySummary);
    }
    if candidate.source_event_ids.is_empty() {
        return Err(MemoryProposalValidationError::MissingSourceEvent(
            Uuid::nil(),
        ));
    }

    let events = source_events
        .iter()
        .map(|event| (event.event_id, event))
        .collect::<HashMap<_, _>>();
    let mut seen = std::collections::HashSet::new();
    for event_id in &candidate.source_event_ids {
        if !seen.insert(*event_id) {
            return Err(MemoryProposalValidationError::DuplicateSourceEvent(
                *event_id,
            ));
        }
        let event = events
            .get(event_id)
            .ok_or(MemoryProposalValidationError::MissingSourceEvent(*event_id))?;
        if event.room_id != candidate.room_id {
            return Err(MemoryProposalValidationError::SourceEventFromAnotherRoom);
        }
    }

    Ok(NewDecisionProposal {
        title: candidate.title,
        summary: candidate.summary,
        source_event_ids: candidate.source_event_ids,
        actor_id: FACILITATOR_ACTOR_ID,
        actor_role: "agent".to_owned(),
        actor_display_name: FACILITATOR_DISPLAY_NAME.to_owned(),
    })
}
