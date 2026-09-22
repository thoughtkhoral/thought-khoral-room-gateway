use std::collections::{BTreeMap, HashSet};

use axum::{
    Json, Router,
    extract::{Path, Request, State},
    http::{HeaderMap, StatusCode, header::AUTHORIZATION},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    auth::WorkloadAuthenticationError,
    rooms::{GatewayState, event_visible_to},
    store::{
        AGENT_TASK_LEASE_DURATION, AgentSkillId, AgentTaskContext, AgentTaskLease, AgentTaskUpdate,
        RoomEvent, StoreError, agent_task, claim_oldest_queued_agent_task, context_for_lease,
        record_agent_task_update_with_outcome,
    },
};

const REFERENCE_AGENT_ID: Uuid = Uuid::from_u128(0x74686f756768746b_686f72616c000003);
const LEASE_TOKEN_HEADER: &str = "x-thought-khoral-lease-token";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClaimRequest {
    pub lease_owner: Uuid,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NormalizedAgentTaskUpdate {
    pub event_type: String,
    pub payload: Value,
    pub occurred_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskUpdateRequest {
    pub update_id: Uuid,
    pub context_revision: i64,
    pub update: NormalizedAgentTaskUpdate,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveDecision {
    pub decision_id: Uuid,
    pub title: String,
    pub summary: String,
    pub source_event_ids: Vec<Uuid>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomContextPacket {
    pub task_id: Uuid,
    pub room_id: Uuid,
    pub requester_id: Uuid,
    pub agent_id: Uuid,
    pub skill_id: AgentSkillId,
    pub context_revision: i64,
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub input: String,
    pub events: Vec<Value>,
    pub active_decisions: Vec<ActiveDecision>,
    pub canonical_sha256: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextResponse {
    pub packet: RoomContextPacket,
    pub lease_token: Uuid,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CanonicalRoomContextPacket<'a> {
    task_id: Uuid,
    room_id: Uuid,
    requester_id: Uuid,
    agent_id: Uuid,
    skill_id: AgentSkillId,
    context_revision: i64,
    issued_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    input: &'a str,
    events: &'a [Value],
    active_decisions: &'a [ActiveDecision],
}

pub(crate) fn routes(state: GatewayState) -> Router<GatewayState> {
    Router::new()
        .route("/internal/v1/agent-tasks/claim", post(claim))
        .route("/internal/v1/agent-tasks/{task_id}/context", get(context))
        .route("/internal/v1/agent-tasks/{task_id}/updates", post(update))
        .route_layer(middleware::from_fn_with_state(
            state,
            authenticate_agent_gateway,
        ))
}

async fn claim(State(state): State<GatewayState>, Json(_request): Json<ClaimRequest>) -> Response {
    // The client's stable worker ID is not a lease capability. Mint a fresh
    // token even when that same worker reclaims an expired task, so an old
    // in-flight request cannot acquire the renewed lease's authority.
    let lease = match claim_oldest_queued_agent_task(
        state.pool(),
        REFERENCE_AGENT_ID,
        Uuid::new_v4(),
        Utc::now(),
    )
    .await
    {
        Ok(Some(lease)) => lease,
        Ok(None) => return StatusCode::NO_CONTENT.into_response(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    context_response(&state, lease).await
}

async fn context(
    State(state): State<GatewayState>,
    Path(task_id): Path<Uuid>,
    headers: HeaderMap,
) -> Response {
    let Some(lease) = matching_live_lease(&state, task_id, &headers).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    context_response(&state, lease).await
}

async fn update(
    State(state): State<GatewayState>,
    Path(task_id): Path<Uuid>,
    headers: HeaderMap,
    Json(request): Json<TaskUpdateRequest>,
) -> Response {
    let Some(lease) = matching_live_lease(&state, task_id, &headers).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let context = match context_for_lease(state.pool(), &lease).await {
        Ok(Some(context)) => context,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    if request.context_revision != context.task.context_revision {
        return StatusCode::CONFLICT.into_response();
    }
    let packet = match context_packet(context, &lease) {
        Ok(packet) => packet,
        Err(()) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    if !terminal_citations_are_visible(&packet, &request.update) {
        return StatusCode::UNPROCESSABLE_ENTITY.into_response();
    }
    let update = AgentTaskUpdate {
        update_id: request.update_id,
        event_type: request.update.event_type,
        payload: request.update.payload,
        occurred_at: request.update.occurred_at,
    };
    let result = match record_agent_task_update_with_outcome(state.pool(), &lease, update).await {
        Ok(result) => result,
        Err(StoreError::ConflictingDuplicate) => return StatusCode::CONFLICT.into_response(),
        Err(StoreError::InvalidAgentTask | StoreError::InvalidEvent) => {
            return StatusCode::UNPROCESSABLE_ENTITY.into_response();
        }
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    if !result.duplicate {
        for event in &result.events {
            state.publish(event.clone());
        }
    }
    Json(serde_json::json!({
        "events": result.events.iter().map(RoomEvent::to_wire_value).collect::<Vec<_>>(),
    }))
    .into_response()
}

async fn authenticate_agent_gateway(
    State(state): State<GatewayState>,
    request: Request,
    next: Next,
) -> Response {
    let authorization = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    match state
        .auth()
        .authenticate_agent_gateway_bearer(authorization)
    {
        Ok(()) => next.run(request).await,
        Err(WorkloadAuthenticationError::Unauthenticated) => {
            StatusCode::UNAUTHORIZED.into_response()
        }
        Err(WorkloadAuthenticationError::Forbidden) => StatusCode::FORBIDDEN.into_response(),
    }
}

async fn matching_live_lease(
    state: &GatewayState,
    task_id: Uuid,
    headers: &HeaderMap,
) -> Option<AgentTaskLease> {
    let lease_token = headers
        .get(LEASE_TOKEN_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Uuid::parse_str(value).ok())?;
    let task = agent_task(state.pool(), task_id).await.ok()?;
    let expires_at = task.lease_expires_at?;
    (task.agent_id == REFERENCE_AGENT_ID
        && task.lease_owner == Some(lease_token)
        && expires_at > Utc::now())
    .then_some(AgentTaskLease {
        task_id,
        owner_id: lease_token,
        expires_at,
    })
}

async fn context_response(state: &GatewayState, lease: AgentTaskLease) -> Response {
    let context = match context_for_lease(state.pool(), &lease).await {
        Ok(Some(context)) => context,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    match context_packet(context, &lease) {
        Ok(packet) => Json(ContextResponse {
            packet,
            lease_token: lease.owner_id,
        })
        .into_response(),
        Err(()) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

fn context_packet(
    context: AgentTaskContext,
    lease: &AgentTaskLease,
) -> Result<RoomContextPacket, ()> {
    let task = context.task;
    let visible_events = context
        .events
        .into_iter()
        .filter(|event| event_visible_to(event, task.requester_id))
        .collect::<Vec<_>>();
    let active_decisions = active_decisions(&visible_events);
    let events = visible_events
        .iter()
        .map(RoomEvent::to_wire_value)
        .collect::<Vec<_>>();
    let issued_at = lease.expires_at - AGENT_TASK_LEASE_DURATION;
    let canonical = CanonicalRoomContextPacket {
        task_id: task.task_id,
        room_id: task.room_id,
        requester_id: task.requester_id,
        agent_id: task.agent_id,
        skill_id: task.skill_id,
        context_revision: task.context_revision,
        issued_at,
        expires_at: lease.expires_at,
        input: &task.input,
        events: &events,
        active_decisions: &active_decisions,
    };
    let canonical_bytes = serde_json::to_vec(&canonical).map_err(|_| ())?;
    let canonical_sha256 = format!("{:x}", Sha256::digest(canonical_bytes));
    Ok(RoomContextPacket {
        task_id: task.task_id,
        room_id: task.room_id,
        requester_id: task.requester_id,
        agent_id: task.agent_id,
        skill_id: task.skill_id,
        context_revision: task.context_revision,
        issued_at,
        expires_at: lease.expires_at,
        input: task.input,
        events,
        active_decisions,
        canonical_sha256,
    })
}

fn terminal_citations_are_visible(
    packet: &RoomContextPacket,
    update: &NormalizedAgentTaskUpdate,
) -> bool {
    if update.event_type != "agent.task.succeeded" {
        return true;
    }
    let Some(citations) = update.payload.pointer("/result/citations") else {
        return true;
    };
    let Some(citations) = citations.as_array() else {
        return false;
    };
    let event_ids = packet
        .events
        .iter()
        .filter_map(|event| event.get("eventId"))
        .filter_map(Value::as_str)
        .filter_map(|value| Uuid::parse_str(value).ok());
    let decision_ids = packet
        .active_decisions
        .iter()
        .map(|decision| decision.decision_id);
    let visible_ids = event_ids.chain(decision_ids).collect::<HashSet<_>>();
    citations.iter().all(|citation| {
        citation
            .as_str()
            .and_then(|value| Uuid::parse_str(value).ok())
            .is_some_and(|citation_id| visible_ids.contains(&citation_id))
    })
}

fn active_decisions(events: &[RoomEvent]) -> Vec<ActiveDecision> {
    let mut decisions = BTreeMap::new();
    for event in events {
        let payload = &event.payload;
        let Some(decision_id) = payload
            .get("decisionId")
            .and_then(Value::as_str)
            .and_then(|value| Uuid::parse_str(value).ok())
        else {
            continue;
        };
        match event.event_type.as_str() {
            "decision.confirmed"
                if payload.get("status").and_then(Value::as_str) == Some("active") =>
            {
                let Some(title) = payload.get("title").and_then(Value::as_str) else {
                    continue;
                };
                let Some(summary) = payload.get("summary").and_then(Value::as_str) else {
                    continue;
                };
                let source_event_ids = payload
                    .get("sourceEventIds")
                    .and_then(Value::as_array)
                    .map(|ids| {
                        ids.iter()
                            .filter_map(Value::as_str)
                            .filter_map(|value| Uuid::parse_str(value).ok())
                            .collect()
                    })
                    .unwrap_or_default();
                decisions.insert(
                    decision_id,
                    ActiveDecision {
                        decision_id,
                        title: title.to_owned(),
                        summary: summary.to_owned(),
                        source_event_ids,
                    },
                );
            }
            "decision.dismissed" | "decision.deleted" | "decision.edited" => {
                decisions.remove(&decision_id);
            }
            _ => {}
        }
    }
    decisions.into_values().collect()
}
