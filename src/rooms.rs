use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::http::Uri;
use serde_json::json;
use sqlx::{PgPool, Row};
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::{
    auth::{Actor, ActorRole, AuthValidator},
    facilitator::{NewDecisionProposal, propose_from_message},
    protocol::{DecisionAction, DecisionTransition, RpcError, ValidatedRequest},
    store::{
        NewEvent, RoomEvent, append_event_in_transaction, lock_room, prior_request, record_request,
    },
};

#[derive(Clone)]
pub struct GatewayState {
    inner: Arc<GatewayStateInner>,
}

#[derive(Clone, Debug)]
pub struct WebSocketPolicy {
    allowed_origins: HashSet<String>,
    authentication_timeout: Duration,
}

impl WebSocketPolicy {
    pub fn new<I, S>(
        allowed_origins: I,
        authentication_timeout: Duration,
    ) -> Result<Self, WebSocketPolicyError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        if authentication_timeout < Duration::from_millis(1)
            || authentication_timeout > Duration::from_secs(30)
        {
            return Err(WebSocketPolicyError);
        }

        let mut normalized_origins = HashSet::new();
        for origin in allowed_origins {
            let origin = origin.as_ref();
            let uri = origin.parse::<Uri>().map_err(|_| WebSocketPolicyError)?;
            let scheme = uri.scheme_str().ok_or(WebSocketPolicyError)?;
            let authority = uri.authority().ok_or(WebSocketPolicyError)?;
            if !matches!(scheme, "http" | "https") || origin != format!("{scheme}://{authority}") {
                return Err(WebSocketPolicyError);
            }
            normalized_origins.insert(origin.to_owned());
        }
        if normalized_origins.is_empty() {
            return Err(WebSocketPolicyError);
        }

        Ok(Self {
            allowed_origins: normalized_origins,
            authentication_timeout,
        })
    }

    fn deny_all() -> Self {
        Self {
            allowed_origins: HashSet::new(),
            authentication_timeout: Duration::from_secs(5),
        }
    }

    pub(crate) fn allows_origin(&self, origin: &str) -> bool {
        self.allowed_origins.contains(origin)
    }

    pub(crate) fn authentication_timeout(&self) -> Duration {
        self.authentication_timeout
    }
}

#[derive(Clone, Copy, Debug)]
pub struct WebSocketPolicyError;

impl std::fmt::Display for WebSocketPolicyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("invalid WebSocket origin or authentication-timeout policy")
    }
}

impl std::error::Error for WebSocketPolicyError {}

struct GatewayStateInner {
    pool: PgPool,
    auth: AuthValidator,
    websocket_policy: WebSocketPolicy,
    rooms: Mutex<HashMap<Uuid, broadcast::Sender<RoomEvent>>>,
}

pub(crate) struct ProcessedRequest {
    pub events: Vec<RoomEvent>,
    pub duplicate: bool,
}

impl GatewayState {
    pub fn new(pool: PgPool, auth: AuthValidator) -> Self {
        Self::with_websocket_policy(pool, auth, WebSocketPolicy::deny_all())
    }

    pub fn with_websocket_policy(
        pool: PgPool,
        auth: AuthValidator,
        websocket_policy: WebSocketPolicy,
    ) -> Self {
        Self {
            inner: Arc::new(GatewayStateInner {
                pool,
                auth,
                websocket_policy,
                rooms: Mutex::new(HashMap::new()),
            }),
        }
    }

    pub(crate) fn auth(&self) -> &AuthValidator {
        &self.inner.auth
    }

    pub(crate) fn websocket_policy(&self) -> &WebSocketPolicy {
        &self.inner.websocket_policy
    }

    pub(crate) fn room_channel(
        &self,
        room_id: Uuid,
    ) -> (broadcast::Sender<RoomEvent>, broadcast::Receiver<RoomEvent>) {
        let mut rooms = self
            .inner
            .rooms
            .lock()
            .expect("room channel map is not poisoned");
        let sender = rooms
            .entry(room_id)
            .or_insert_with(|| broadcast::channel(256).0)
            .clone();
        let receiver = sender.subscribe();
        (sender, receiver)
    }

    pub fn publish(&self, event: RoomEvent) {
        let sender = {
            let mut rooms = self
                .inner
                .rooms
                .lock()
                .expect("room channel map is not poisoned");
            rooms
                .entry(event.room_id)
                .or_insert_with(|| broadcast::channel(256).0)
                .clone()
        };
        let _ = sender.send(event);
    }

    pub(crate) async fn replay(
        &self,
        room_id: Uuid,
        after_sequence: i64,
    ) -> Result<Vec<RoomEvent>, RpcError> {
        crate::store::events_after(&self.inner.pool, room_id, after_sequence)
            .await
            .map_err(|_| RpcError::internal_error())
    }

    pub(crate) async fn process(
        &self,
        actor: Actor,
        request: ValidatedRequest,
    ) -> Result<ProcessedRequest, RpcError> {
        if matches!(request, ValidatedRequest::DecisionTransition(_))
            && actor.role != ActorRole::Human
        {
            return Err(RpcError::forbidden());
        }
        if matches!(
            request,
            ValidatedRequest::Join(_) | ValidatedRequest::SessionAuthenticate(_)
        ) {
            return Err(RpcError::invalid_request());
        }

        let room_id = request
            .room_id()
            .expect("non-room requests returned before persistence");
        let request_id = request
            .request_id()
            .expect("non-room requests returned before persistence");
        let fingerprint = request.fingerprint();
        let mut transaction = self
            .inner
            .pool
            .begin()
            .await
            .map_err(|_| RpcError::internal_error())?;
        lock_room(&mut transaction, room_id)
            .await
            .map_err(|_| RpcError::internal_error())?;

        if let Some((prior_fingerprint, events)) =
            prior_request(&mut transaction, room_id, request_id)
                .await
                .map_err(|_| RpcError::internal_error())?
        {
            transaction
                .rollback()
                .await
                .map_err(|_| RpcError::internal_error())?;
            if prior_fingerprint == fingerprint {
                return Ok(ProcessedRequest {
                    events,
                    duplicate: true,
                });
            }
            return Err(RpcError::conflicting_duplicate());
        }

        let events = match request {
            ValidatedRequest::ChatSend(request) => {
                let message = append_event_in_transaction(
                    &mut transaction,
                    NewEvent {
                        room_id,
                        request_id,
                        event_type: "message.created".to_owned(),
                        actor_id: actor.id,
                        actor_role: actor.role.as_str().to_owned(),
                        payload: json!({ "text": request.text }),
                        occurred_at: request.occurred_at,
                    },
                )
                .await
                .map_err(|_| RpcError::internal_error())?;
                let mut events = vec![message.clone()];
                if let Some(proposal) = propose_from_message(&message) {
                    events
                        .push(persist_draft_proposal(&mut transaction, &message, proposal).await?);
                }
                events
            }
            ValidatedRequest::DecisionPropose(request) => {
                let decision_id = Uuid::new_v4();
                sqlx::query(
                    r#"
                    INSERT INTO decisions (
                        decision_id, room_id, status, title, summary,
                        source_event_ids, created_at, updated_at
                    ) VALUES ($1, $2, 'draft', $3, $4, $5, $6, $6)
                    "#,
                )
                .bind(decision_id)
                .bind(room_id)
                .bind(&request.title)
                .bind(&request.summary)
                .bind(&request.source_event_ids)
                .bind(request.occurred_at)
                .execute(&mut *transaction)
                .await
                .map_err(|_| RpcError::internal_error())?;
                vec![
                    append_event_in_transaction(
                        &mut transaction,
                        NewEvent {
                            room_id,
                            request_id,
                            event_type: "decision.proposed".to_owned(),
                            actor_id: actor.id,
                            actor_role: actor.role.as_str().to_owned(),
                            payload: json!({
                                "decisionId": decision_id,
                                "status": "draft",
                                "title": request.title,
                                "summary": request.summary,
                                "sourceEventIds": request.source_event_ids,
                            }),
                            occurred_at: request.occurred_at,
                        },
                    )
                    .await
                    .map_err(|_| RpcError::internal_error())?,
                ]
            }
            ValidatedRequest::DecisionTransition(request) => {
                transition_decision_in_transaction(&mut transaction, actor, request).await?
            }
            ValidatedRequest::Join(_) => unreachable!("join requests returned above"),
            ValidatedRequest::SessionAuthenticate(_) => {
                unreachable!("session authentication requests returned above")
            }
        };

        record_request(&mut transaction, room_id, request_id, fingerprint, &events)
            .await
            .map_err(|_| RpcError::internal_error())?;
        transaction
            .commit()
            .await
            .map_err(|_| RpcError::internal_error())?;
        Ok(ProcessedRequest {
            events,
            duplicate: false,
        })
    }

    pub async fn transition_decision(
        &self,
        actor: Actor,
        request: DecisionTransition,
    ) -> Result<Vec<RoomEvent>, RpcError> {
        self.process(actor, ValidatedRequest::DecisionTransition(request))
            .await
            .map(|processed| processed.events)
    }
}

async fn persist_draft_proposal(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    source: &RoomEvent,
    proposal: NewDecisionProposal,
) -> Result<RoomEvent, RpcError> {
    let decision_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO decisions (
            decision_id, room_id, status, title, summary,
            source_event_ids, created_at, updated_at
        ) VALUES ($1, $2, 'draft', $3, $4, $5, $6, $6)
        "#,
    )
    .bind(decision_id)
    .bind(source.room_id)
    .bind(&proposal.title)
    .bind(&proposal.summary)
    .bind(&proposal.source_event_ids)
    .bind(source.occurred_at)
    .execute(&mut **transaction)
    .await
    .map_err(|_| RpcError::internal_error())?;

    append_event_in_transaction(
        transaction,
        NewEvent {
            room_id: source.room_id,
            request_id: source.request_id,
            event_type: "decision.proposed".to_owned(),
            actor_id: proposal.actor_id,
            actor_role: proposal.actor_role,
            payload: json!({
                "decisionId": decision_id,
                "status": "draft",
                "title": proposal.title,
                "summary": proposal.summary,
                "sourceEventIds": proposal.source_event_ids,
            }),
            occurred_at: source.occurred_at,
        },
    )
    .await
    .map_err(|_| RpcError::internal_error())
}

async fn transition_decision_in_transaction(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    actor: Actor,
    request: DecisionTransition,
) -> Result<Vec<RoomEvent>, RpcError> {
    let row = sqlx::query(
        r#"
        SELECT status, title, summary, source_event_ids
        FROM decisions
        WHERE decision_id = $1 AND room_id = $2
        FOR UPDATE
        "#,
    )
    .bind(request.decision_id)
    .bind(request.room_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|_| RpcError::internal_error())?
    .ok_or_else(RpcError::not_found)?;
    let status: String = row
        .try_get("status")
        .map_err(|_| RpcError::internal_error())?;
    if status != "draft" {
        return Err(RpcError::invalid_transition());
    }
    let title: String = row
        .try_get("title")
        .map_err(|_| RpcError::internal_error())?;
    let summary: String = row
        .try_get("summary")
        .map_err(|_| RpcError::internal_error())?;
    let source_event_ids: Vec<Uuid> = row
        .try_get("source_event_ids")
        .map_err(|_| RpcError::internal_error())?;

    match request.action {
        DecisionAction::Confirm | DecisionAction::Dismiss => {
            let (status, event_type) = match request.action {
                DecisionAction::Confirm => ("active", "decision.confirmed"),
                DecisionAction::Dismiss => ("dismissed", "decision.dismissed"),
                DecisionAction::Edit => unreachable!(),
            };
            sqlx::query("UPDATE decisions SET status = $1, updated_at = $2 WHERE decision_id = $3")
                .bind(status)
                .bind(request.occurred_at)
                .bind(request.decision_id)
                .execute(&mut **transaction)
                .await
                .map_err(|_| RpcError::internal_error())?;
            let event = append_event_in_transaction(
                transaction,
                NewEvent {
                    room_id: request.room_id,
                    request_id: request.request_id,
                    event_type: event_type.to_owned(),
                    actor_id: actor.id,
                    actor_role: actor.role.as_str().to_owned(),
                    payload: json!({
                        "decisionId": request.decision_id,
                        "status": status,
                        "title": title,
                        "summary": summary,
                        "sourceEventIds": source_event_ids,
                    }),
                    occurred_at: request.occurred_at,
                },
            )
            .await
            .map_err(|_| RpcError::internal_error())?;
            Ok(vec![event])
        }
        DecisionAction::Edit => {
            let edited_title = request.edited_title.ok_or_else(RpcError::invalid_request)?;
            let edited_summary = request
                .edited_summary
                .ok_or_else(RpcError::invalid_request)?;
            let replacement_id = Uuid::new_v4();
            sqlx::query("UPDATE decisions SET status = 'superseded', updated_at = $1 WHERE decision_id = $2")
                .bind(request.occurred_at)
                .bind(request.decision_id)
                .execute(&mut **transaction)
                .await
                .map_err(|_| RpcError::internal_error())?;
            sqlx::query(
                r#"
                INSERT INTO decisions (
                    decision_id, room_id, status, title, summary,
                    derived_from_decision_id, source_event_ids, created_at, updated_at
                ) VALUES ($1, $2, 'active', $3, $4, $5, $6, $7, $7)
                "#,
            )
            .bind(replacement_id)
            .bind(request.room_id)
            .bind(&edited_title)
            .bind(&edited_summary)
            .bind(request.decision_id)
            .bind(&source_event_ids)
            .bind(request.occurred_at)
            .execute(&mut **transaction)
            .await
            .map_err(|_| RpcError::internal_error())?;

            let edited = append_event_in_transaction(
                transaction,
                NewEvent {
                    room_id: request.room_id,
                    request_id: request.request_id,
                    event_type: "decision.edited".to_owned(),
                    actor_id: actor.id,
                    actor_role: actor.role.as_str().to_owned(),
                    payload: json!({
                        "decisionId": request.decision_id,
                        "status": "superseded",
                        "replacementDecisionId": replacement_id,
                    }),
                    occurred_at: request.occurred_at,
                },
            )
            .await
            .map_err(|_| RpcError::internal_error())?;
            let activated = append_event_in_transaction(
                transaction,
                NewEvent {
                    room_id: request.room_id,
                    request_id: request.request_id,
                    event_type: "decision.confirmed".to_owned(),
                    actor_id: actor.id,
                    actor_role: actor.role.as_str().to_owned(),
                    payload: json!({
                        "decisionId": replacement_id,
                        "status": "active",
                        "title": edited_title,
                        "summary": edited_summary,
                        "sourceEventIds": source_event_ids,
                        "derivedFromDecisionId": request.decision_id,
                    }),
                    occurred_at: request.occurred_at,
                },
            )
            .await
            .map_err(|_| RpcError::internal_error())?;
            Ok(vec![edited, activated])
        }
    }
}
