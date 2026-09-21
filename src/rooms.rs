use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::http::Uri;
use serde_json::{Value, json};
use sqlx::{PgPool, Row};
use tokio::sync::broadcast;
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;

use crate::{
    action_items::{ACTION_ITEMS_AGENT_DISPLAY_NAME, ACTION_ITEMS_AGENT_ID, extract_action_items},
    auth::{Actor, ActorRole, AuthValidator},
    memory_engine_client::MemoryEngineClient,
    protocol::{
        ChatDelivery, ChatMention, ChatMentionAlias, ChatSend, DecisionAction, DecisionDelete,
        DecisionTransition, RpcError, ValidatedRequest,
    },
    store::{
        NewEvent, RoomEvent, append_event_in_transaction, lock_room, prior_request, record_request,
        start_agent_task_in_transaction,
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
    rooms: Mutex<HashMap<Uuid, broadcast::Sender<RoomBroadcast>>>,
    presence: Mutex<HashMap<Uuid, HashMap<Uuid, ParticipantPresence>>>,
    memory_engine: Option<MemoryEngineClient>,
}

#[derive(Clone, Debug)]
pub(crate) struct RoomParticipant {
    pub id: Uuid,
    pub role: String,
    pub display_name: String,
    pub online: bool,
}

impl RoomParticipant {
    pub fn to_wire_value(&self) -> Value {
        json!({
            "id": self.id,
            "role": self.role,
            "displayName": self.display_name,
            "online": self.online,
        })
    }
}

const MENTION_ALIASES: [&str; 2] = ["allhumans", "allagents"];

/// Governs both WebSocket replay and agent context packets. A packet must never contain an event
/// that its invoking human could not have received in the room.
pub(crate) fn event_visible_to(event: &RoomEvent, actor: Uuid) -> bool {
    match event.payload.get("delivery").and_then(Value::as_str) {
        Some("mentioned") => event
            .payload
            .get("audienceIds")
            .and_then(Value::as_array)
            .is_some_and(|audience_ids| {
                audience_ids.iter().any(|audience_id| {
                    audience_id.as_str().and_then(|id| id.parse::<Uuid>().ok()) == Some(actor)
                })
            }),
        Some("room") | None => true,
        Some(_) => true,
    }
}

fn normalized_participant_name(display_name: &str) -> String {
    let slug = display_name
        .nfkd()
        .filter(|character| !matches!(*character, '\u{0300}'..='\u{036f}'))
        .flat_map(char::to_lowercase)
        .fold(String::new(), |mut slug, character| {
            if character.is_ascii_alphanumeric() {
                slug.push(character);
            } else if !slug.ends_with('-') {
                slug.push('-');
            }
            slug
        })
        .trim_matches('-')
        .to_owned();
    if slug.is_empty() {
        "participant".to_owned()
    } else {
        slug
    }
}

fn participant_id_token(id: Uuid) -> String {
    let token = id
        .to_string()
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    if token.is_empty() {
        "participant".to_owned()
    } else {
        token
    }
}

fn participant_mention_tokens(participants: &[RoomParticipant]) -> Vec<String> {
    let names = participants
        .iter()
        .map(|participant| normalized_participant_name(&participant.display_name))
        .collect::<Vec<_>>();
    let mut name_counts = HashMap::<&str, usize>::new();
    for name in &names {
        *name_counts.entry(name).or_default() += 1;
    }

    let mut tokens = vec![None; participants.len()];
    let mut occupied = MENTION_ALIASES
        .into_iter()
        .map(str::to_owned)
        .collect::<HashSet<_>>();
    let needs_suffix = |index: usize| {
        MENTION_ALIASES.contains(&names[index].as_str()) || name_counts[names[index].as_str()] > 1
    };
    for (index, name) in names.iter().enumerate() {
        if !needs_suffix(index) {
            tokens[index] = Some(name.clone());
            occupied.insert(name.clone());
        }
    }

    let mut disambiguated = participants
        .iter()
        .enumerate()
        .filter(|(index, _)| needs_suffix(*index))
        .map(|(index, participant)| {
            (
                index,
                names[index].clone(),
                participant_id_token(participant.id),
            )
        })
        .collect::<Vec<_>>();
    disambiguated.sort_by(|left, right| {
        left.1
            .cmp(&right.1)
            .then_with(|| left.2.cmp(&right.2))
            .then_with(|| left.0.cmp(&right.0))
    });

    let mut suffix_lengths = vec![None; participants.len()];
    for (index, name, _) in &disambiguated {
        if suffix_lengths[*index].is_some() {
            continue;
        }
        let group = disambiguated
            .iter()
            .filter(|(_, candidate_name, _)| candidate_name == name)
            .collect::<Vec<_>>();
        let max_id_length = group.iter().map(|(_, _, id)| id.len()).max().unwrap_or(0);
        let mut id_length = 8.min(max_id_length);
        while id_length < max_id_length
            && (group
                .iter()
                .map(|(_, _, id)| &id[..id_length])
                .collect::<HashSet<_>>()
                .len()
                != group.len()
                || group
                    .iter()
                    .any(|(_, _, id)| occupied.contains(&format!("{name}-{}", &id[..id_length]))))
        {
            id_length += 1;
        }
        for (group_index, _, _) in group {
            suffix_lengths[*group_index] = Some(id_length);
        }
    }

    for (index, name, id) in disambiguated {
        let mut id_length = suffix_lengths[index].unwrap_or(8).min(id.len());
        let mut token = format!("{name}-{}", &id[..id_length]);
        while occupied.contains(&token) && id_length < id.len() {
            id_length += 1;
            token = format!("{name}-{}", &id[..id_length]);
        }
        let mut duplicate = 2;
        while occupied.contains(&token) {
            token = format!("{name}-{id}-{duplicate}");
            duplicate += 1;
        }
        tokens[index] = Some(token.clone());
        occupied.insert(token);
    }

    tokens
        .into_iter()
        .map(|token| token.unwrap_or_else(|| "participant".to_owned()))
        .collect()
}

fn resolve_chat_audience(
    actor: &Actor,
    participants: &[RoomParticipant],
    request: &ChatSend,
) -> Result<Vec<Uuid>, RpcError> {
    let participant_tokens = participant_mention_tokens(participants);
    let known_participants = participants
        .iter()
        .enumerate()
        .map(|(index, participant)| (participant.id, &participant_tokens[index]))
        .collect::<HashMap<_, _>>();
    let mut mentioned_participant_ids = HashSet::new();
    let mut mentioned_aliases = HashSet::new();

    for mention in &request.mentions {
        match mention {
            ChatMention::Participant { id, token } => {
                let Some(canonical_token) = known_participants.get(id) else {
                    return Err(RpcError::unknown_mention_target());
                };
                if !mentioned_participant_ids.insert(*id) || token != *canonical_token {
                    return Err(RpcError::unknown_mention_target());
                }
            }
            ChatMention::Alias { alias } if !mentioned_aliases.insert(*alias as u8) => {
                return Err(RpcError::unknown_mention_target());
            }
            ChatMention::Alias { .. } => {}
        }
    }

    if request.delivery == ChatDelivery::Room {
        return Ok(Vec::new());
    }

    let mut audience_ids = HashSet::from([actor.id]);

    for mention in &request.mentions {
        match mention {
            ChatMention::Participant { id, .. } => {
                audience_ids.insert(*id);
            }
            ChatMention::Alias {
                alias: ChatMentionAlias::AllHumans,
            } => {
                audience_ids.extend(
                    participants
                        .iter()
                        .filter(|participant| participant.role == "human")
                        .map(|participant| participant.id),
                );
            }
            ChatMention::Alias {
                alias: ChatMentionAlias::AllAgents,
            } => {
                audience_ids.extend(
                    participants
                        .iter()
                        .filter(|participant| {
                            matches!(participant.role.as_str(), "agent" | "human")
                        })
                        .map(|participant| participant.id),
                );
            }
        }
    }

    let mut audience_ids = audience_ids.into_iter().collect::<Vec<_>>();
    audience_ids.sort_unstable();
    Ok(audience_ids)
}

fn normalized_chat_payload(request: &ChatSend, audience_ids: &[Uuid]) -> serde_json::Value {
    json!({
        "text": request.text,
        "mentions": request.mentions,
        "delivery": request.delivery,
        "audienceIds": audience_ids,
    })
}

fn action_items_task_payload(
    task_id: Uuid,
    source_event_id: Uuid,
    requester_id: Uuid,
) -> serde_json::Value {
    json!({
        "taskId": task_id,
        "kind": "action-items.v1",
        "sourceEventId": source_event_id,
        "requesterId": requester_id,
        "agentId": ACTION_ITEMS_AGENT_ID,
    })
}

fn invokes_action_items(actor: &Actor, request: &ChatSend) -> bool {
    actor.role == ActorRole::Human
        && request.delivery == ChatDelivery::Room
        && request.mentions.iter().any(|mention| {
            matches!(
                mention,
                ChatMention::Participant { id, .. } if *id == ACTION_ITEMS_AGENT_ID
            )
        })
}

#[derive(Clone, Debug)]
pub(crate) enum RoomBroadcast {
    Event(RoomEvent),
    Participants {
        participants: Vec<RoomParticipant>,
        exclude_actor_id: Option<Uuid>,
    },
}

#[derive(Clone, Debug)]
struct ParticipantPresence {
    role: String,
    display_name: String,
    connections: usize,
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
                presence: Mutex::new(HashMap::new()),
                memory_engine: None,
            }),
        }
    }

    pub fn with_memory_engine_client(
        pool: PgPool,
        auth: AuthValidator,
        websocket_policy: WebSocketPolicy,
        memory_engine: MemoryEngineClient,
    ) -> Self {
        Self {
            inner: Arc::new(GatewayStateInner {
                pool,
                auth,
                websocket_policy,
                rooms: Mutex::new(HashMap::new()),
                presence: Mutex::new(HashMap::new()),
                memory_engine: Some(memory_engine),
            }),
        }
    }

    pub(crate) fn auth(&self) -> &AuthValidator {
        &self.inner.auth
    }

    pub(crate) fn websocket_policy(&self) -> &WebSocketPolicy {
        &self.inner.websocket_policy
    }

    pub(crate) fn pool(&self) -> &PgPool {
        &self.inner.pool
    }

    pub(crate) fn room_channel(
        &self,
        room_id: Uuid,
    ) -> (
        broadcast::Sender<RoomBroadcast>,
        broadcast::Receiver<RoomBroadcast>,
    ) {
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
        let _ = sender.send(RoomBroadcast::Event(event));
    }

    pub(crate) fn register_participant(&self, room_id: Uuid, actor: &Actor) {
        let mut presence = self
            .inner
            .presence
            .lock()
            .expect("room presence map is not poisoned");
        let room = presence.entry(room_id).or_default();
        let participant = room.entry(actor.id).or_insert_with(|| ParticipantPresence {
            role: actor.role.as_str().to_owned(),
            display_name: actor.display_name.clone(),
            connections: 0,
        });
        participant.role = actor.role.as_str().to_owned();
        participant.display_name = actor.display_name.clone();
        participant.connections += 1;
    }

    pub(crate) fn unregister_participant(&self, room_id: Uuid, actor_id: Uuid) {
        let mut presence = self
            .inner
            .presence
            .lock()
            .expect("room presence map is not poisoned");
        let Some(room) = presence.get_mut(&room_id) else {
            return;
        };
        let Some(participant) = room.get_mut(&actor_id) else {
            return;
        };
        if participant.connections > 1 {
            participant.connections -= 1;
        } else {
            participant.connections = 0;
        }
    }

    pub(crate) async fn participant_snapshot(
        &self,
        room_id: Uuid,
    ) -> Result<Vec<RoomParticipant>, RpcError> {
        let events = self.replay(room_id, 0).await?;
        let mut participants = HashMap::<Uuid, RoomParticipant>::new();
        participants.insert(
            ACTION_ITEMS_AGENT_ID,
            RoomParticipant {
                id: ACTION_ITEMS_AGENT_ID,
                role: "agent".to_owned(),
                display_name: ACTION_ITEMS_AGENT_DISPLAY_NAME.to_owned(),
                online: true,
            },
        );
        for event in events {
            let role = event.actor_role;
            let display_name = event.actor_display_name.unwrap_or_else(|| {
                format!(
                    "{} {}",
                    if role == "human" { "Human" } else { "Agent" },
                    &event.actor_id.to_string()[..8]
                )
            });
            participants.insert(
                event.actor_id,
                RoomParticipant {
                    id: event.actor_id,
                    role,
                    display_name,
                    online: false,
                },
            );
        }
        if let Some(room_presence) = self
            .inner
            .presence
            .lock()
            .expect("room presence map is not poisoned")
            .get(&room_id)
            .cloned()
        {
            for (id, presence) in room_presence {
                participants.insert(
                    id,
                    RoomParticipant {
                        id,
                        role: presence.role,
                        display_name: presence.display_name,
                        online: presence.connections > 0,
                    },
                );
            }
        }
        let mut participants = participants.into_values().collect::<Vec<_>>();
        participants.sort_by(|left, right| {
            left.role
                .cmp(&right.role)
                .then_with(|| left.display_name.cmp(&right.display_name))
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(participants)
    }

    pub(crate) async fn publish_participant_snapshot(
        &self,
        room_id: Uuid,
        exclude_actor_id: Option<Uuid>,
    ) -> Result<(), RpcError> {
        let participants = self.participant_snapshot(room_id).await?;
        let sender = {
            let rooms = self
                .inner
                .rooms
                .lock()
                .expect("room channel map is not poisoned");
            rooms.get(&room_id).cloned()
        };
        if let Some(sender) = sender {
            let _ = sender.send(RoomBroadcast::Participants {
                participants,
                exclude_actor_id,
            });
        }
        Ok(())
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
        if matches!(
            request,
            ValidatedRequest::DecisionTransition(_)
                | ValidatedRequest::DecisionDelete(_)
                | ValidatedRequest::AgentTaskStart(_)
        ) && actor.role != ActorRole::Human
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
                let participants = self.participant_snapshot(room_id).await?;
                let audience_ids = resolve_chat_audience(&actor, &participants, &request)?;
                let message = append_event_in_transaction(
                    &mut transaction,
                    NewEvent {
                        room_id,
                        request_id,
                        event_type: "message.created".to_owned(),
                        actor_id: actor.id,
                        actor_role: actor.role.as_str().to_owned(),
                        actor_display_name: Some(actor.display_name.clone()),
                        payload: normalized_chat_payload(&request, &audience_ids),
                        occurred_at: request.occurred_at,
                    },
                )
                .await
                .map_err(|_| RpcError::internal_error())?;
                let mut events = vec![message.clone()];
                if invokes_action_items(&actor, &request) {
                    let task_id = Uuid::new_v4();
                    let core_payload =
                        action_items_task_payload(task_id, message.event_id, actor.id);
                    for event_type in ["agent.task.queued", "agent.task.running"] {
                        events.push(
                            append_event_in_transaction(
                                &mut transaction,
                                NewEvent {
                                    room_id,
                                    request_id,
                                    event_type: event_type.to_owned(),
                                    actor_id: ACTION_ITEMS_AGENT_ID,
                                    actor_role: "agent".to_owned(),
                                    actor_display_name: Some(
                                        ACTION_ITEMS_AGENT_DISPLAY_NAME.to_owned(),
                                    ),
                                    payload: core_payload.clone(),
                                    occurred_at: request.occurred_at,
                                },
                            )
                            .await
                            .map_err(|_| RpcError::internal_error())?,
                        );
                    }
                    let (event_type, payload) = match extract_action_items(&request.text) {
                        Ok(action_items) => {
                            let mut payload = core_payload;
                            payload["result"] = json!({
                                "actionItems": action_items.into_iter().map(|item| json!({
                                    "text": item.text,
                                    "owner": item.owner,
                                    "due": item.due,
                                })).collect::<Vec<_>>(),
                            });
                            ("agent.task.succeeded", payload)
                        }
                        Err(_) => {
                            let mut payload = core_payload;
                            payload["failure"] = json!({ "code": "invalid_task_input" });
                            ("agent.task.failed", payload)
                        }
                    };
                    events.push(
                        append_event_in_transaction(
                            &mut transaction,
                            NewEvent {
                                room_id,
                                request_id,
                                event_type: event_type.to_owned(),
                                actor_id: ACTION_ITEMS_AGENT_ID,
                                actor_role: "agent".to_owned(),
                                actor_display_name: Some(
                                    ACTION_ITEMS_AGENT_DISPLAY_NAME.to_owned(),
                                ),
                                payload,
                                occurred_at: request.occurred_at,
                            },
                        )
                        .await
                        .map_err(|_| RpcError::internal_error())?,
                    );
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
                            actor_display_name: Some(actor.display_name.clone()),
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
            ValidatedRequest::DecisionDelete(request) => {
                delete_decision_in_transaction(&mut transaction, actor, request).await?
            }
            ValidatedRequest::AgentTaskStart(request) => {
                start_agent_task_in_transaction(
                    &mut transaction,
                    actor.id,
                    &actor.display_name,
                    request,
                    false,
                )
                .await
                .map_err(|_| RpcError::internal_error())?
                .events
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
        if !events.is_empty()
            && let Some(memory_engine) = self.inner.memory_engine.clone()
        {
            for event in &events {
                let _ = memory_engine.enqueue_committed_event(event.clone()).await;
            }
        }
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

async fn delete_decision_in_transaction(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    actor: Actor,
    request: DecisionDelete,
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
    let prior_status: String = row
        .try_get("status")
        .map_err(|_| RpcError::internal_error())?;
    let title: String = row
        .try_get("title")
        .map_err(|_| RpcError::internal_error())?;
    let summary: String = row
        .try_get("summary")
        .map_err(|_| RpcError::internal_error())?;
    let source_event_ids: Vec<Uuid> = row
        .try_get("source_event_ids")
        .map_err(|_| RpcError::internal_error())?;

    let event = append_event_in_transaction(
        transaction,
        NewEvent {
            room_id: request.room_id,
            request_id: request.request_id,
            event_type: "decision.deleted".to_owned(),
            actor_id: actor.id,
            actor_role: actor.role.as_str().to_owned(),
            actor_display_name: Some(actor.display_name),
            payload: json!({
                "decisionId": request.decision_id,
                "priorStatus": prior_status,
                "title": title,
                "summary": summary,
                "sourceEventIds": source_event_ids,
            }),
            occurred_at: request.occurred_at,
        },
    )
    .await
    .map_err(|_| RpcError::internal_error())?;
    sqlx::query("DELETE FROM decisions WHERE decision_id = $1 AND room_id = $2")
        .bind(request.decision_id)
        .bind(request.room_id)
        .execute(&mut **transaction)
        .await
        .map_err(|_| RpcError::internal_error())?;
    Ok(vec![event])
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
                    actor_display_name: Some(actor.display_name.clone()),
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
                    actor_display_name: Some(actor.display_name.clone()),
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
                    actor_display_name: Some(actor.display_name.clone()),
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
