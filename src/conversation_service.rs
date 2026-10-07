use crate::{
    Actor, ActorRole, GatewayState,
    auth::WorkloadAuthenticationError,
    conversation_protocol::{
        CODEX_AGENT_ID, ConversationError, PROFILE_VERSION, validate_profile_value,
    },
    conversation_store::{ConversationPolicy, ConversationStore},
};
use axum::{
    Extension, Json, Router,
    body::to_bytes,
    extract::{Path, Request, State},
    http::{
        HeaderValue, Method, StatusCode,
        header::{AUTHORIZATION, ORIGIN},
    },
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::Utc;
use serde::{
    Deserialize,
    de::{self, MapAccess, SeqAccess, Visitor},
};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::{
    collections::{HashMap, HashSet},
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
};
use uuid::Uuid;
pub trait CatalogQuery: Send + Sync {
    fn page<'a>(
        &'a self,
        agent: Uuid,
        cursor: Option<String>,
        limit: usize,
    ) -> Pin<Box<dyn Future<Output = Result<Value, ConversationError>> + Send + 'a>>;
}

pub(crate) struct ConversationIntegration {
    pub store: ConversationStore,
    catalog: Option<Arc<dyn CatalogQuery>>,
    cursors: Mutex<HashMap<String, (String, String)>>,
}
impl ConversationIntegration {
    pub(crate) fn disabled(pool: PgPool) -> Self {
        Self {
            store: ConversationStore::new(pool),
            catalog: None,
            cursors: Mutex::new(HashMap::new()),
        }
    }
    pub(crate) fn new(
        pool: PgPool,
        policy: ConversationPolicy,
        catalog: Option<Arc<dyn CatalogQuery>>,
    ) -> Result<Self, ConversationError> {
        Ok(Self {
            store: ConversationStore::new(pool).with_policy(policy)?,
            catalog,
            cursors: Mutex::new(HashMap::new()),
        })
    }
}
struct UnavailableCatalog;
impl CatalogQuery for UnavailableCatalog {
    fn page<'a>(
        &'a self,
        _agent: Uuid,
        _cursor: Option<String>,
        _limit: usize,
    ) -> Pin<Box<dyn Future<Output = Result<Value, ConversationError>> + Send + 'a>> {
        Box::pin(async { Err(ConversationError::RuntimeUnavailable) })
    }
}
static UNAVAILABLE_CATALOG: UnavailableCatalog = UnavailableCatalog;

pub(crate) fn routes(state: GatewayState) -> Router<GatewayState> {
    let browser = Router::new()
        .route("/turns", post(turn).options(preflight))
        .route(
            "/rooms/{room}/agents/{agent}",
            get(conversation).options(preflight),
        )
        .route(
            "/rooms/{room}/agents/{agent}/models",
            get(models).options(preflight),
        )
        .route(
            "/rooms/{room}/agents/{agent}/defaults",
            get(defaults).options(preflight),
        )
        .route(
            "/rooms/{room}/tasks/{task}",
            get(task_view).options(preflight),
        )
        .fallback(not_found)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            authenticate_browser,
        ));
    let internal = Router::new()
        .route("/claim", post(claim))
        .route("/tasks/{task}/context", get(context))
        .route("/tasks/{task}/updates", post(update))
        .route("/tasks/{task}/authority", get(authority))
        .route("/tasks/{task}/receipt", get(receipt))
        .fallback(not_found)
        .layer(middleware::from_fn_with_state(state, authenticate_workload));
    Router::new()
        .nest("/api/agent-conversations/v1", browser)
        .nest("/internal/agent-conversations/v1", internal)
}
async fn preflight() -> StatusCode {
    StatusCode::NO_CONTENT
}
async fn not_found() -> Response {
    profile_error(ConversationError::SessionUnavailable)
}

fn profile_error(error: ConversationError) -> Response {
    let status = match error {
        ConversationError::InvalidTaskInput => StatusCode::BAD_REQUEST,
        ConversationError::AuthenticationRequired => StatusCode::UNAUTHORIZED,
        ConversationError::Forbidden => StatusCode::FORBIDDEN,
        ConversationError::SessionUnavailable => StatusCode::NOT_FOUND,
        ConversationError::ContextTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        ConversationError::RuntimeUnavailable => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::CONFLICT,
    };
    (status,Json(json!({"profileVersion":PROFILE_VERSION,"code":error.code(),"message":error.code(),"requestId":null}))).into_response()
}
fn response(result: Result<Value, ConversationError>) -> Response {
    match result {
        Ok(value) => Json(value).into_response(),
        Err(error) => profile_error(error),
    }
}

async fn authenticate_browser(
    State(state): State<GatewayState>,
    mut request: Request,
    next: Next,
) -> Response {
    let origin = request
        .headers()
        .get(ORIGIN)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    if request.headers().contains_key(ORIGIN)
        && origin
            .as_ref()
            .is_none_or(|origin| !state.websocket_policy().allows_origin(origin))
    {
        let mut result = profile_error(ConversationError::Forbidden);
        result
            .headers_mut()
            .insert("cache-control", HeaderValue::from_static("no-store"));
        return result;
    }
    let mut result = if request.method() == Method::OPTIONS {
        let method = request
            .headers()
            .get("access-control-request-method")
            .and_then(|value| value.to_str().ok());
        let headers = request
            .headers()
            .get("access-control-request-headers")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");
        if origin.is_none()
            || !matches!(method, Some("GET" | "POST"))
            || !headers
                .split(',')
                .map(str::trim)
                .filter(|header| !header.is_empty())
                .all(|header| {
                    matches!(
                        header.to_ascii_lowercase().as_str(),
                        "authorization" | "content-type"
                    )
                })
        {
            profile_error(ConversationError::Forbidden)
        } else {
            let mut response = StatusCode::NO_CONTENT.into_response();
            response.headers_mut().insert(
                "access-control-allow-methods",
                HeaderValue::from_static("GET, POST, OPTIONS"),
            );
            response.headers_mut().insert(
                "access-control-allow-headers",
                HeaderValue::from_static("authorization, content-type"),
            );
            response
        }
    } else {
        match state.auth().authenticate_bearer(
            request
                .headers()
                .get(AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
        ) {
            None => profile_error(ConversationError::AuthenticationRequired),
            Some(actor) if actor.role != ActorRole::Human => {
                profile_error(ConversationError::Forbidden)
            }
            Some(actor) => {
                request.extensions_mut().insert(actor);
                next.run(request).await
            }
        }
    };
    result
        .headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    if let Some(origin) = origin
        && let Ok(origin) = HeaderValue::from_str(&origin)
    {
        result
            .headers_mut()
            .insert("access-control-allow-origin", origin);
        result
            .headers_mut()
            .insert("vary", HeaderValue::from_static("Origin"));
    }
    result
}
async fn authenticate_workload(
    State(state): State<GatewayState>,
    request: Request,
    next: Next,
) -> Response {
    if request.headers().contains_key(ORIGIN) {
        return profile_error(ConversationError::Forbidden);
    }
    let mut response = match state.auth().authenticate_agent_gateway_bearer(
        request
            .headers()
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
    ) {
        Ok(()) => next.run(request).await,
        Err(WorkloadAuthenticationError::Unauthenticated) => {
            profile_error(ConversationError::AuthenticationRequired)
        }
        Err(WorkloadAuthenticationError::Forbidden) => profile_error(ConversationError::Forbidden),
    };
    response
        .headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    response
}

fn canonical_uuid(value: &str) -> Result<Uuid, ConversationError> {
    let uuid = Uuid::parse_str(value).map_err(|_| ConversationError::InvalidTaskInput)?;
    if uuid.to_string() != value {
        return Err(ConversationError::InvalidTaskInput);
    }
    Ok(uuid)
}
fn lease(request: &Request) -> Result<Uuid, ConversationError> {
    request
        .headers()
        .get("x-thought-khoral-lease-token")
        .and_then(|value| value.to_str().ok())
        .ok_or(ConversationError::Forbidden)
        .and_then(|value| canonical_uuid(value).map_err(|_| ConversationError::Forbidden))
}
fn no_query(request: &Request) -> Result<(), ConversationError> {
    if request.uri().query().is_some() {
        Err(ConversationError::InvalidTaskInput)
    } else {
        Ok(())
    }
}

async fn body(request: Request) -> Result<Value, ConversationError> {
    no_query(&request)?;
    if request
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .is_none_or(|value| value.split(';').next().map(str::trim) != Some("application/json"))
    {
        return Err(ConversationError::InvalidTaskInput);
    }
    let bytes = to_bytes(request.into_body(), 1_048_576)
        .await
        .map_err(|_| ConversationError::ContextTooLarge)?;
    parse_strict_json(&bytes)
}

pub(crate) fn parse_strict_json(bytes: &[u8]) -> Result<Value, ConversationError> {
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    let value = StrictValue::deserialize(&mut decoder)
        .map_err(|_| ConversationError::InvalidTaskInput)?
        .0;
    decoder
        .end()
        .map_err(|_| ConversationError::InvalidTaskInput)?;
    Ok(value)
}
struct StrictValue(Value);
impl<'de> Deserialize<'de> for StrictValue {
    fn deserialize<D: serde::Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        struct StrictVisitor;
        impl<'de> Visitor<'de> for StrictVisitor {
            type Value = StrictValue;
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("JSON with unique object keys")
            }
            fn visit_bool<E: de::Error>(self, value: bool) -> Result<Self::Value, E> {
                Ok(StrictValue(json!(value)))
            }
            fn visit_i64<E: de::Error>(self, value: i64) -> Result<Self::Value, E> {
                Ok(StrictValue(json!(value)))
            }
            fn visit_u64<E: de::Error>(self, value: u64) -> Result<Self::Value, E> {
                Ok(StrictValue(json!(value)))
            }
            fn visit_f64<E: de::Error>(self, _value: f64) -> Result<Self::Value, E> {
                Err(E::custom("non-integer JSON number"))
            }
            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(StrictValue(json!(value)))
            }
            fn visit_string<E: de::Error>(self, value: String) -> Result<Self::Value, E> {
                Ok(StrictValue(json!(value)))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(StrictValue(Value::Null))
            }
            fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(StrictValue(Value::Null))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut values = vec![];
                while let Some(value) = seq.next_element::<StrictValue>()? {
                    values.push(value.0);
                }
                Ok(StrictValue(Value::Array(values)))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut values = serde_json::Map::new();
                while let Some(key) = map.next_key::<String>()? {
                    if values.contains_key(&key) {
                        return Err(de::Error::custom("duplicate JSON key"));
                    }
                    values.insert(key, map.next_value::<StrictValue>()?.0);
                }
                Ok(StrictValue(Value::Object(values)))
            }
        }
        decoder.deserialize_any(StrictVisitor)
    }
}

async fn turn(
    State(state): State<GatewayState>,
    Extension(actor): Extension<Actor>,
    request: Request,
) -> Response {
    let request = match body(request).await {
        Ok(request) => request,
        Err(error) => return profile_error(error),
    };
    let request = match crate::conversation_protocol::validate_turn_request(&request) {
        Ok(request) => request,
        Err(error) => return profile_error(error),
    };
    let integration = state.conversations().await;
    let catalog = integration
        .catalog
        .as_deref()
        .unwrap_or(&UNAVAILABLE_CATALOG);
    let expiry = chrono::DateTime::from_timestamp(actor.expires_at, 0).unwrap_or_else(Utc::now);
    match integration
        .store
        .reserve_mediated(&actor, &request, expiry, catalog, &state)
        .await
    {
        Err(error) => profile_error(error),
        Ok(outcome) => {
            if let Some(event) = outcome.prompt {
                state.publish(event);
            }
            (StatusCode::ACCEPTED, Json(outcome.accepted)).into_response()
        }
    }
}
async fn defaults(
    State(state): State<GatewayState>,
    Extension(actor): Extension<Actor>,
    Path((room, agent)): Path<(String, String)>,
    request: Request,
) -> Response {
    response(
        async move {
            no_query(&request)?;
            let room_id = canonical_uuid(&room)?;
            let agent_id = canonical_uuid(&agent)?;
            if agent_id != CODEX_AGENT_ID {
                return Err(ConversationError::SessionUnavailable);
            }
            let token_expiry = chrono::DateTime::from_timestamp(actor.expires_at, 0)
                .ok_or(ConversationError::AuthenticationRequired)?;
            crate::rooms::authorize_conversation_turn(
                &actor,
                token_expiry,
                Utc::now(),
                chrono::Duration::zero(),
            )?;
            // Keep one integration snapshot throughout the read; no room row or
            // membership is required by the existing authenticated-human authority.
            let integration = state.conversations().await;
            let policy = integration.store.policy();
            if !policy.enabled {
                return Err(ConversationError::RuntimeUnavailable);
            }
            let catalog = integration
                .catalog
                .as_deref()
                .ok_or(ConversationError::RuntimeUnavailable)?;
            let selected = policy
                .resolve_settings(None)
                .map_err(|_| ConversationError::RuntimeUnavailable)?;
            tokio::time::timeout(
                std::time::Duration::from_secs(5),
                validate_catalog_selection(catalog, policy, &selected),
            )
            .await
            .map_err(|_| ConversationError::RuntimeUnavailable)?
            .map_err(|_| ConversationError::RuntimeUnavailable)?;
            crate::rooms::authorize_conversation_turn(
                &actor,
                token_expiry,
                Utc::now(),
                chrono::Duration::zero(),
            )?;
            let view = json!({"profileVersion":PROFILE_VERSION,"roomId":room_id,
            "agentId":agent_id,"selectedSettings":selected});
            validate_profile_value("resolved-settings", &view)?;
            Ok(view)
        }
        .await,
    )
}
async fn conversation(
    State(state): State<GatewayState>,
    Path((room, agent)): Path<(String, String)>,
    request: Request,
) -> Response {
    let result = async move {
        no_query(&request)?;
        state
            .conversations()
            .await
            .store
            .conversation_view(canonical_uuid(&room)?, canonical_uuid(&agent)?)
            .await
    }
    .await;
    response(result)
}
async fn task_view(
    State(state): State<GatewayState>,
    Path((room, task)): Path<(String, String)>,
    request: Request,
) -> Response {
    let result = async move {
        no_query(&request)?;
        state
            .conversations()
            .await
            .store
            .task_view(canonical_uuid(&room)?, canonical_uuid(&task)?)
            .await
    }
    .await;
    response(result)
}
async fn claim(State(state): State<GatewayState>, request: Request) -> Response {
    let result = async {
        let request = body(request).await?;
        if request.as_object().is_none_or(|object| object.len() != 2)
            || request["agentId"] != json!(CODEX_AGENT_ID)
        {
            return Err(ConversationError::InvalidTaskInput);
        }
        let owner = request["leaseOwner"]
            .as_str()
            .ok_or(ConversationError::InvalidTaskInput)?;
        state.conversations().await.store.claim(owner).await
    }
    .await;
    match result {
        Ok(Some(value)) => Json(value).into_response(),
        Ok(None) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => profile_error(error),
    }
}
async fn context(
    State(state): State<GatewayState>,
    Path(task): Path<String>,
    request: Request,
) -> Response {
    response(
        async move {
            no_query(&request)?;
            state
                .conversations()
                .await
                .store
                .packet(canonical_uuid(&task)?, lease(&request)?)
                .await
        }
        .await,
    )
}
async fn authority(
    State(state): State<GatewayState>,
    Path(task): Path<String>,
    request: Request,
) -> Response {
    response(async move {no_query(&request)?;let packet=state.conversations().await.store.packet(canonical_uuid(&task)?,lease(&request)?).await?;Ok(json!({"profileVersion":PROFILE_VERSION,"taskId":packet["taskId"],"generation":packet["conversation"]["generation"],"contextDigest":packet["context"]["digest"],"expiresAt":packet["expiresAt"],"authorizationExpiresAt":packet["authorizationExpiresAt"]}))}.await)
}
async fn receipt(
    State(state): State<GatewayState>,
    Path(task): Path<String>,
    request: Request,
) -> Response {
    response(
        async move {
            no_query(&request)?;
            state
                .conversations()
                .await
                .store
                .receipt(canonical_uuid(&task)?)
                .await
        }
        .await,
    )
}
async fn update(
    State(state): State<GatewayState>,
    Path(task): Path<String>,
    request: Request,
) -> Response {
    let result = async {
        let lease = lease(&request)?;
        let request = body(request).await?;
        state
            .conversations()
            .await
            .store
            .record_update(canonical_uuid(&task)?, lease, &request)
            .await
    }
    .await;
    match result {
        Err(error) => profile_error(error),
        Ok(outcome) => {
            if let Some(event) = outcome.event {
                state.publish(event);
            }
            Json(outcome.response).into_response()
        }
    }
}

fn validate_catalog(page: &Value, policy: &ConversationPolicy) -> Result<(), ConversationError> {
    validate_profile_value("catalog", page).map_err(|_| ConversationError::RuntimeUnavailable)?;
    if page.to_string().len() > 1_048_576 {
        return Err(ConversationError::RuntimeUnavailable);
    }
    if page["catalogRevision"] != policy.catalog_revision {
        return Err(ConversationError::ConversationStale);
    }
    let mut models = HashSet::new();
    for model in page["data"]
        .as_array()
        .ok_or(ConversationError::RuntimeUnavailable)?
    {
        if !models.insert(model["id"].as_str()) {
            return Err(ConversationError::RuntimeUnavailable);
        }
        let mut efforts = HashSet::new();
        for effort in model["supportedReasoningEfforts"]
            .as_array()
            .ok_or(ConversationError::RuntimeUnavailable)?
        {
            if !efforts.insert(effort["id"].as_str()) {
                return Err(ConversationError::RuntimeUnavailable);
            }
        }
        if !model["defaultReasoningEffort"].is_null()
            && !efforts.contains(&model["defaultReasoningEffort"].as_str())
        {
            return Err(ConversationError::RuntimeUnavailable);
        }
    }
    Ok(())
}
pub(crate) async fn validate_catalog_selection(
    catalog: &dyn CatalogQuery,
    policy: &ConversationPolicy,
    selected: &Value,
) -> Result<(), ConversationError> {
    let mut cursor = None;
    let mut cursors = HashSet::new();
    let mut models = HashSet::new();
    let mut found = false;
    for _ in 0..100 {
        let page = catalog.page(CODEX_AGENT_ID, cursor, 100).await?;
        validate_catalog(&page, policy)?;
        for model in page["data"]
            .as_array()
            .ok_or(ConversationError::RuntimeUnavailable)?
        {
            if !models.insert(
                model["id"]
                    .as_str()
                    .ok_or(ConversationError::RuntimeUnavailable)?
                    .to_owned(),
            ) {
                return Err(ConversationError::RuntimeUnavailable);
            }
            if model["id"] == selected["model"] {
                found = model["supportedReasoningEfforts"]
                    .as_array()
                    .ok_or(ConversationError::RuntimeUnavailable)?
                    .iter()
                    .any(|effort| effort["id"] == selected["reasoningEffort"]);
            }
        }
        cursor = page["nextCursor"].as_str().map(str::to_owned);
        let Some(next) = cursor.as_ref() else {
            return if found {
                Ok(())
            } else {
                Err(ConversationError::InvalidTaskInput)
            };
        };
        if !cursors.insert(next.clone()) {
            return Err(ConversationError::RuntimeUnavailable);
        }
    }
    Err(ConversationError::RuntimeUnavailable)
}
async fn models(
    State(state): State<GatewayState>,
    Path((_room, agent)): Path<(String, String)>,
    request: Request,
) -> Response {
    response(
        async move {
            canonical_uuid(&_room)?;
            if canonical_uuid(&agent)? != CODEX_AGENT_ID {
                return Err(ConversationError::Forbidden);
            }
            let mut limit = 100;
            let mut cursor = None;
            let mut keys = HashSet::new();
            for (key, value) in
                url::form_urlencoded::parse(request.uri().query().unwrap_or("").as_bytes())
            {
                if !keys.insert(key.to_string()) {
                    return Err(ConversationError::InvalidTaskInput);
                }
                match key.as_ref() {
                    "limit" => {
                        limit = value
                            .parse::<usize>()
                            .map_err(|_| ConversationError::InvalidTaskInput)?;
                        if !(1..=100).contains(&limit) {
                            return Err(ConversationError::InvalidTaskInput);
                        }
                    }
                    "cursor" => {
                        if value.is_empty() || value.chars().count() > 256 {
                            return Err(ConversationError::InvalidTaskInput);
                        }
                        cursor = Some(value.to_string());
                    }
                    _ => return Err(ConversationError::InvalidTaskInput),
                }
            }
            let integration = state.conversations().await;
            let policy = integration.store.policy();
            if !policy.enabled {
                return Err(ConversationError::RuntimeUnavailable);
            }
            let provider_cursor = if let Some(cursor) = cursor {
                let cursors = integration
                    .cursors
                    .lock()
                    .map_err(|_| ConversationError::RuntimeUnavailable)?;
                let (revision, provider) = cursors
                    .get(&cursor)
                    .ok_or(ConversationError::ConversationStale)?;
                if *revision != policy.catalog_revision {
                    return Err(ConversationError::ConversationStale);
                }
                Some(provider.clone())
            } else {
                None
            };
            let mut page = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                integration
                    .catalog
                    .as_deref()
                    .ok_or(ConversationError::RuntimeUnavailable)?
                    .page(CODEX_AGENT_ID, provider_cursor, limit),
            )
            .await
            .map_err(|_| ConversationError::RuntimeUnavailable)??;
            validate_catalog(&page, policy)?;
            let data = page["data"]
                .as_array_mut()
                .ok_or(ConversationError::RuntimeUnavailable)?;
            if data.len() > limit {
                return Err(ConversationError::RuntimeUnavailable);
            }
            data.retain_mut(|model| {
                let Some(allowed) = policy
                    .models
                    .iter()
                    .find(|allowed| model["id"] == allowed.id)
                else {
                    return false;
                };
                if let Some(efforts) = model["supportedReasoningEfforts"].as_array_mut() {
                    efforts.retain(|effort| {
                        allowed
                            .reasoning_efforts
                            .iter()
                            .any(|id| effort["id"] == *id)
                    });
                }
                if !allowed
                    .reasoning_efforts
                    .iter()
                    .any(|id| model["defaultReasoningEffort"] == *id)
                {
                    model["defaultReasoningEffort"] = Value::Null;
                }
                model["supportedReasoningEfforts"]
                    .as_array()
                    .is_some_and(|efforts| !efforts.is_empty())
            });
            if let Some(next) = page["nextCursor"].as_str().map(str::to_owned) {
                let cursor = Uuid::new_v4().to_string();
                let mut cursors = integration
                    .cursors
                    .lock()
                    .map_err(|_| ConversationError::RuntimeUnavailable)?;
                if cursors.len() >= 1024
                    && let Some(old) = cursors.keys().next().cloned()
                {
                    cursors.remove(&old);
                }
                cursors.insert(cursor.clone(), (policy.catalog_revision.clone(), next));
                page["nextCursor"] = json!(cursor);
            }
            validate_profile_value("catalog", &page)?;
            Ok(page)
        }
        .await,
    )
}
