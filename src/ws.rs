use std::time::Duration;

use axum::{
    Json, Router,
    extract::{
        State,
        ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade, close_code},
    },
    http::{HeaderMap, StatusCode, header::AUTHORIZATION},
    response::{IntoResponse, Response},
    routing::get,
};
use serde_json::{Value, json};
use tokio::sync::broadcast;
use tracing::warn;
use uuid::Uuid;

use crate::{
    auth::Actor,
    protocol::{RpcError, ValidatedRequest, validate_request},
    rooms::GatewayState,
    store::RoomEvent,
};

pub fn app(state: GatewayState) -> Router {
    Router::new()
        .route("/ws", get(websocket_upgrade))
        .with_state(state)
}

async fn websocket_upgrade(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    let authorization = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    let Some(actor) = state.auth().authenticate_bearer(authorization) else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(error_response(Value::Null, RpcError::unauthenticated())),
        )
            .into_response();
    };
    upgrade.on_upgrade(move |socket| websocket_session(socket, state, actor))
}

async fn websocket_session(mut socket: WebSocket, state: GatewayState, actor: Actor) {
    let now = chrono::Utc::now().timestamp();
    let expires_in = actor.expires_at.saturating_sub(now).max(0) as u64;
    let expiry = tokio::time::sleep(Duration::from_secs(expires_in));
    tokio::pin!(expiry);

    let mut joined_room = None;
    let mut sender = None;
    let mut receiver: Option<broadcast::Receiver<RoomEvent>> = None;
    let mut last_sequence = 0_i64;

    loop {
        if let Some(room_receiver) = receiver.as_mut() {
            tokio::select! {
                _ = &mut expiry => {
                    let _ = send_error(&mut socket, Value::Null, RpcError::unauthenticated()).await;
                    let _ = socket.send(Message::Close(Some(CloseFrame {
                        code: close_code::POLICY,
                        reason: "authentication expired".into(),
                    }))).await;
                    return;
                }
                message = socket.recv() => {
                    let Some(Ok(message)) = message else { return; };
                    if !handle_message(
                        &mut socket,
                        &state,
                        actor,
                        message,
                        &mut joined_room,
                        &mut sender,
                        &mut receiver,
                        &mut last_sequence,
                    ).await {
                        return;
                    }
                }
                event = room_receiver.recv() => {
                    match event {
                        Ok(event) if event.sequence > last_sequence => {
                            last_sequence = event.sequence;
                            if send_value(&mut socket, event.to_wire_value()).await.is_err() {
                                return;
                            }
                        }
                        Ok(_) => {}
                        Err(broadcast::error::RecvError::Lagged(_)) => {
                            let Some(room_id) = joined_room else { continue; };
                            let Ok(events) = state.replay(room_id, last_sequence).await else { return; };
                            for event in events {
                                last_sequence = event.sequence;
                                if send_value(&mut socket, event.to_wire_value()).await.is_err() {
                                    return;
                                }
                            }
                        }
                        Err(broadcast::error::RecvError::Closed) => return,
                    }
                }
            }
        } else {
            tokio::select! {
                _ = &mut expiry => {
                    let _ = send_error(&mut socket, Value::Null, RpcError::unauthenticated()).await;
                    let _ = socket.send(Message::Close(None)).await;
                    return;
                }
                message = socket.recv() => {
                    let Some(Ok(message)) = message else { return; };
                    if !handle_message(
                        &mut socket,
                        &state,
                        actor,
                        message,
                        &mut joined_room,
                        &mut sender,
                        &mut receiver,
                        &mut last_sequence,
                    ).await {
                        return;
                    }
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_message(
    socket: &mut WebSocket,
    state: &GatewayState,
    actor: Actor,
    message: Message,
    joined_room: &mut Option<Uuid>,
    sender: &mut Option<broadcast::Sender<RoomEvent>>,
    receiver: &mut Option<broadcast::Receiver<RoomEvent>>,
    last_sequence: &mut i64,
) -> bool {
    let Message::Text(text) = message else {
        if matches!(message, Message::Close(_)) {
            return false;
        }
        return true;
    };
    let raw = text.as_str();
    let fallback_id = request_id_value(raw);
    let request = match validate_request(raw) {
        Ok(request) => request,
        Err(error) => {
            warn!(actor_id = %actor.id, error_code = error.code, "room request rejected");
            return send_error(socket, fallback_id, error).await.is_ok();
        }
    };
    let response_id = Value::String(request.id().to_owned());
    let room_id = request.room_id();
    let application_request_id = request.request_id();

    if let ValidatedRequest::Join(join) = request {
        if joined_room.is_some_and(|joined| joined != join.room_id) {
            let error = RpcError::forbidden();
            log_rejection(actor, join.room_id, join.request_id, &error);
            return send_error(socket, response_id, error).await.is_ok();
        }
        let (room_sender, room_receiver) = state.room_channel(join.room_id);
        let after_sequence = join.after_sequence.unwrap_or(0);
        let events = match state.replay(join.room_id, after_sequence).await {
            Ok(events) => events,
            Err(error) => {
                log_rejection(actor, join.room_id, join.request_id, &error);
                return send_error(socket, response_id, error).await.is_ok();
            }
        };
        *last_sequence = events.last().map_or(after_sequence, |event| event.sequence);
        *joined_room = Some(join.room_id);
        *sender = Some(room_sender);
        *receiver = Some(room_receiver);
        let events = events
            .into_iter()
            .map(|event| event.to_wire_value())
            .collect::<Vec<_>>();
        return send_value(
            socket,
            json!({ "jsonrpc": "2.0", "id": response_id, "result": { "events": events } }),
        )
        .await
        .is_ok();
    }

    if *joined_room != Some(room_id) {
        let error = RpcError::forbidden();
        log_rejection(actor, room_id, application_request_id, &error);
        return send_error(socket, response_id, error).await.is_ok();
    }

    match state.process(actor, request).await {
        Ok(processed) if processed.duplicate => {
            for event in processed.events {
                if send_value(socket, event.to_wire_value()).await.is_err() {
                    return false;
                }
            }
            true
        }
        Ok(processed) => {
            let room_sender = sender.as_ref().expect("joined room has a sender");
            for event in processed.events {
                let _ = room_sender.send(event);
            }
            true
        }
        Err(error) => {
            log_rejection(actor, room_id, application_request_id, &error);
            send_error(socket, response_id, error).await.is_ok()
        }
    }
}

fn log_rejection(actor: Actor, room_id: Uuid, request_id: Uuid, error: &RpcError) {
    warn!(
        actor_id = %actor.id,
        room_id = %room_id,
        request_id = %request_id,
        error_code = error.code,
        "room request rejected"
    );
}

fn request_id_value(raw: &str) -> Value {
    serde_json::from_str::<Value>(raw)
        .ok()
        .and_then(|value| value.get("id").cloned())
        .filter(|id| id.is_string())
        .unwrap_or(Value::Null)
}

fn error_response(id: Value, error: RpcError) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": error.code, "message": error.message },
    })
}

async fn send_error(socket: &mut WebSocket, id: Value, error: RpcError) -> Result<(), axum::Error> {
    send_value(socket, error_response(id, error)).await
}

async fn send_value(socket: &mut WebSocket, value: Value) -> Result<(), axum::Error> {
    socket.send(Message::Text(value.to_string().into())).await
}
