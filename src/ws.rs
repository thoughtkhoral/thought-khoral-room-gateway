use std::time::Duration;

use axum::{
    Json, Router,
    extract::{
        State,
        ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade, close_code},
    },
    http::{
        HeaderMap, StatusCode,
        header::{AUTHORIZATION, ORIGIN},
    },
    response::{IntoResponse, Response},
    routing::get,
};
use serde_json::{Value, json};
use tokio::sync::broadcast;
use tracing::warn;
use uuid::Uuid;

use crate::{
    PRODUCT_NAME, SERVICE_NAME,
    auth::Actor,
    protocol::{RpcError, ValidatedRequest, validate_request},
    rooms::{GatewayState, RoomBroadcast, RoomParticipant},
};

pub fn app(state: GatewayState) -> Router {
    Router::new()
        .route("/health", get(gateway_status))
        .route("/ws", get(websocket_upgrade))
        .with_state(state)
}

pub async fn gateway_status() -> Json<Value> {
    Json(json!({
        "product": PRODUCT_NAME,
        "service": SERVICE_NAME,
        "status": "ok",
    }))
}

async fn websocket_upgrade(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    if let Some(origin) = headers.get(ORIGIN) {
        let Some(origin) = origin.to_str().ok() else {
            return rejected_upgrade(StatusCode::FORBIDDEN, RpcError::forbidden());
        };
        if !state.websocket_policy().allows_origin(origin) {
            return rejected_upgrade(StatusCode::FORBIDDEN, RpcError::forbidden());
        }
        return upgrade.on_upgrade(move |socket| browser_authentication_session(socket, state));
    }

    let authorization = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    let Some(actor) = state.auth().authenticate_bearer(authorization) else {
        return rejected_upgrade(StatusCode::UNAUTHORIZED, RpcError::unauthenticated());
    };
    upgrade.on_upgrade(move |socket| websocket_session(socket, state, actor))
}

fn rejected_upgrade(status: StatusCode, error: RpcError) -> Response {
    (status, Json(error_response(Value::Null, error))).into_response()
}

async fn browser_authentication_session(mut socket: WebSocket, state: GatewayState) {
    let timeout = tokio::time::sleep(state.websocket_policy().authentication_timeout());
    tokio::pin!(timeout);

    loop {
        tokio::select! {
            _ = &mut timeout => {
                warn!(error_code = -32001, "browser authentication timed out");
                close_with_error(
                    &mut socket,
                    Value::Null,
                    RpcError::unauthenticated(),
                    "authentication timeout",
                ).await;
                return;
            }
            message = socket.recv() => {
                let Some(Ok(message)) = message else { return; };
                match message {
                    Message::Text(text) => {
                        let raw = text.as_str();
                        let fallback_id = request_id_value(raw);
                        match validate_request(raw) {
                            Ok(ValidatedRequest::SessionAuthenticate(request)) => {
                                let Some(actor) = state
                                    .auth()
                                    .authenticate_access_token(&request.access_token)
                                else {
                                    warn!(error_code = -32001, "browser authentication rejected");
                                    close_with_error(
                                        &mut socket,
                                        Value::String(request.id),
                                        RpcError::unauthenticated(),
                                        "authentication failed",
                                    ).await;
                                    return;
                                };
                                let response = json!({
                                    "jsonrpc": "2.0",
                                    "id": request.id,
                                "result": {
                                    "actor": {
                                        "id": actor.id,
                                        "role": actor.role.as_str(),
                                        "displayName": actor.display_name,
                                    },
                                        "expiresAt": actor.expires_at,
                                    },
                                });
                                if send_value(&mut socket, response).await.is_err() {
                                    return;
                                }
                                websocket_session(socket, state, actor).await;
                                return;
                            }
                            Ok(_) => {
                                warn!(error_code = -32001, "room request rejected before authentication");
                                close_with_error(
                                    &mut socket,
                                    fallback_id,
                                    RpcError::unauthenticated(),
                                    "authentication required",
                                ).await;
                                return;
                            }
                            Err(error) => {
                                warn!(error_code = error.code, "initial authentication request rejected");
                                close_with_error(
                                    &mut socket,
                                    fallback_id,
                                    error,
                                    "invalid authentication request",
                                ).await;
                                return;
                            }
                        }
                    }
                    Message::Close(_) => return,
                    Message::Ping(_) | Message::Pong(_) => {}
                    Message::Binary(_) => {
                        let error = RpcError::invalid_request();
                        warn!(error_code = error.code, "initial authentication request rejected");
                        close_with_error(
                            &mut socket,
                            Value::Null,
                            error,
                            "invalid authentication request",
                        ).await;
                        return;
                    }
                }
            }
        }
    }
}

async fn websocket_session(mut socket: WebSocket, state: GatewayState, actor: Actor) {
    let expires_at_millis = actor.expires_at.saturating_mul(1_000);
    let expires_in_millis = expires_at_millis
        .saturating_sub(chrono::Utc::now().timestamp_millis())
        .max(0) as u64;
    let expiry = tokio::time::sleep(Duration::from_millis(expires_in_millis));
    tokio::pin!(expiry);

    let mut joined_room = None;
    let mut receiver: Option<broadcast::Receiver<RoomBroadcast>> = None;
    let mut last_sequence = 0_i64;

    loop {
        if let Some(room_receiver) = receiver.as_mut() {
            tokio::select! {
                _ = &mut expiry => {
                    close_with_error(
                        &mut socket,
                        Value::Null,
                        RpcError::unauthenticated(),
                        "authentication expired",
                    ).await;
                    break;
                }
                message = socket.recv() => {
                    let Some(Ok(message)) = message else { break; };
                    if !handle_message(
                        &mut socket,
                        &state,
                        actor.clone(),
                        message,
                        &mut joined_room,
                        &mut receiver,
                        &mut last_sequence,
                    ).await {
                        break;
                    }
                }
                event = room_receiver.recv() => {
                    match event {
                        Ok(RoomBroadcast::Event(event)) if event.sequence == last_sequence + 1 => {
                            if send_value(&mut socket, event.to_wire_value()).await.is_err() {
                                break;
                            }
                            last_sequence = event.sequence;
                        }
                        Ok(RoomBroadcast::Event(event)) if event.sequence > last_sequence + 1 => {
                            let Some(room_id) = joined_room else { break; };
                            if replay_in_order(
                                &mut socket,
                                &state,
                                room_id,
                                &mut last_sequence,
                            ).await.is_err() {
                                break;
                            }
                        }
                        Ok(RoomBroadcast::Event(_)) => {}
                        Ok(RoomBroadcast::Participants {
                            participants,
                            exclude_actor_id,
                        }) => {
                            if exclude_actor_id == Some(actor.id) {
                                continue;
                            }
                            if send_value(
                                &mut socket,
                                participant_update_value(
                                    joined_room.expect("participant updates require a room"),
                                    participants,
                                ),
                            )
                            .await
                            .is_err()
                            {
                                break;
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => {
                            let Some(room_id) = joined_room else { continue; };
                            if replay_in_order(
                                &mut socket,
                                &state,
                                room_id,
                                &mut last_sequence,
                            ).await.is_err() {
                                break;
                            }
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        } else {
            tokio::select! {
                _ = &mut expiry => {
                    close_with_error(
                        &mut socket,
                        Value::Null,
                        RpcError::unauthenticated(),
                        "authentication expired",
                    ).await;
                    break;
                }
                message = socket.recv() => {
                    let Some(Ok(message)) = message else { break; };
                    if !handle_message(
                        &mut socket,
                        &state,
                        actor.clone(),
                        message,
                        &mut joined_room,
                        &mut receiver,
                        &mut last_sequence,
                    ).await {
                        break;
                    }
                }
            }
        }
    }

    cleanup_presence(&state, &actor, &mut joined_room).await;
}

async fn cleanup_presence(state: &GatewayState, actor: &Actor, joined_room: &mut Option<Uuid>) {
    let Some(room_id) = joined_room.take() else {
        return;
    };
    state.unregister_participant(room_id, actor.id);
    let _ = state.publish_participant_snapshot(room_id, None).await;
}

async fn handle_message(
    socket: &mut WebSocket,
    state: &GatewayState,
    actor: Actor,
    message: Message,
    joined_room: &mut Option<Uuid>,
    receiver: &mut Option<broadcast::Receiver<RoomBroadcast>>,
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
    if actor.expires_at <= chrono::Utc::now().timestamp() {
        warn!(actor_id = %actor.id, error_code = -32001, "expired session request rejected");
        close_with_error(
            socket,
            fallback_id,
            RpcError::unauthenticated(),
            "authentication expired",
        )
        .await;
        return false;
    }
    let request = match validate_request(raw) {
        Ok(request) => request,
        Err(error) => {
            warn!(actor_id = %actor.id, error_code = error.code, "room request rejected");
            return send_error(socket, fallback_id, error).await.is_ok();
        }
    };
    let response_id = Value::String(request.id().to_owned());
    if matches!(request, ValidatedRequest::SessionAuthenticate(_)) {
        let error = RpcError::forbidden();
        warn!(actor_id = %actor.id, error_code = error.code, "session reauthentication rejected");
        return send_error(socket, response_id, error).await.is_ok();
    }
    let room_id = request
        .room_id()
        .expect("session authentication requests returned above");
    let application_request_id = request
        .request_id()
        .expect("session authentication requests returned above");

    if let ValidatedRequest::Join(join) = request {
        if joined_room.is_some_and(|joined| joined != join.room_id) {
            let error = RpcError::forbidden();
            log_rejection(&actor, join.room_id, join.request_id, &error);
            return send_error(socket, response_id, error).await.is_ok();
        }
        let is_new_join = joined_room.is_none();
        let (_, room_receiver) = state.room_channel(join.room_id);
        let after_sequence = join.after_sequence.unwrap_or(0);
        let events = match state.replay(join.room_id, after_sequence).await {
            Ok(events) => events,
            Err(error) => {
                log_rejection(&actor, join.room_id, join.request_id, &error);
                return send_error(socket, response_id, error).await.is_ok();
            }
        };
        if is_new_join {
            state.register_participant(join.room_id, &actor);
        }
        let participants = match state.participant_snapshot(join.room_id).await {
            Ok(participants) => participants,
            Err(error) => {
                if is_new_join {
                    state.unregister_participant(join.room_id, actor.id);
                }
                log_rejection(&actor, join.room_id, join.request_id, &error);
                return send_error(socket, response_id, error).await.is_ok();
            }
        };
        *last_sequence = events.last().map_or(after_sequence, |event| event.sequence);
        *joined_room = Some(join.room_id);
        *receiver = Some(room_receiver);
        let events = events
            .into_iter()
            .map(|event| event.to_wire_value())
            .collect::<Vec<_>>();
        let sent = send_value(
            socket,
            json!({
                "jsonrpc": "2.0",
                "id": response_id,
                "result": {
                    "events": events,
                    "participants": participants.iter().map(RoomParticipant::to_wire_value).collect::<Vec<_>>(),
                }
            }),
        )
        .await
        .is_ok();
        if sent && is_new_join {
            let _ = state
                .publish_participant_snapshot(join.room_id, Some(actor.id))
                .await;
        }
        return sent;
    }

    if *joined_room != Some(room_id) {
        let error = RpcError::forbidden();
        log_rejection(&actor, room_id, application_request_id, &error);
        return send_error(socket, response_id, error).await.is_ok();
    }

    match state.process(actor.clone(), request).await {
        Ok(processed) if processed.duplicate => {
            for event in processed.events {
                if send_value(socket, event.to_wire_value()).await.is_err() {
                    return false;
                }
            }
            true
        }
        Ok(processed) => {
            for event in processed.events {
                state.publish(event);
            }
            true
        }
        Err(error) => {
            log_rejection(&actor, room_id, application_request_id, &error);
            send_error(socket, response_id, error).await.is_ok()
        }
    }
}

async fn replay_in_order(
    socket: &mut WebSocket,
    state: &GatewayState,
    room_id: Uuid,
    last_sequence: &mut i64,
) -> Result<(), ()> {
    let events = state
        .replay(room_id, *last_sequence)
        .await
        .map_err(|_| ())?;
    for event in events {
        if event.sequence <= *last_sequence {
            continue;
        }
        if event.sequence != *last_sequence + 1 {
            return Err(());
        }
        send_value(socket, event.to_wire_value())
            .await
            .map_err(|_| ())?;
        *last_sequence = event.sequence;
    }
    Ok(())
}

fn log_rejection(actor: &Actor, room_id: Uuid, request_id: Uuid, error: &RpcError) {
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

fn participant_update_value(room_id: Uuid, participants: Vec<RoomParticipant>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "room.participants.updated",
        "params": {
            "contractVersion": "n2n.room.v1",
            "roomId": room_id,
            "participants": participants.iter().map(RoomParticipant::to_wire_value).collect::<Vec<_>>(),
        }
    })
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

async fn close_with_error(
    socket: &mut WebSocket,
    id: Value,
    error: RpcError,
    reason: &'static str,
) {
    let _ = send_error(socket, id, error).await;
    let _ = socket
        .send(Message::Close(Some(CloseFrame {
            code: close_code::POLICY,
            reason: reason.into(),
        })))
        .await;
}

async fn send_value(socket: &mut WebSocket, value: Value) -> Result<(), axum::Error> {
    socket.send(Message::Text(value.to_string().into())).await
}
