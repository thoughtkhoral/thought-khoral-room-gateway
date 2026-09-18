mod support;

use std::time::Duration;

use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
};
use serde_json::json;
use thought_khoral_room_gateway::memory_engine_client::{
    MemoryEngineClient, MemoryEngineClientConfig,
};
use tokio::{net::TcpListener, sync::oneshot, time::timeout};
use uuid::Uuid;

use support::TestServer;
use support::{common_params, join, recv_json, rpc, send_json};

type RecordedRequest = (HeaderMap, Bytes);
type RecordingSender = std::sync::Arc<tokio::sync::Mutex<Option<oneshot::Sender<RecordedRequest>>>>;

async fn recording_endpoint() -> (String, oneshot::Receiver<RecordedRequest>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (sender, receiver) = oneshot::channel();
    let app = Router::new()
        .route(
            "/internal/v1/ingest",
            post(
                |State(sender): State<RecordingSender>,
                 headers: HeaderMap,
                 body: Bytes| async move {
                    if let Some(sender) = sender.lock().await.take() {
                        let _ = sender.send((headers, body));
                    }
                    StatusCode::OK
                },
            ),
        )
        .with_state(std::sync::Arc::new(tokio::sync::Mutex::new(Some(sender))));
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{address}/internal/v1/ingest"), receiver)
}

#[tokio::test]
async fn committed_event_is_forwarded_after_persistence_without_mutating_decisions() {
    let room_id = Uuid::new_v4();
    let (endpoint, received) = recording_endpoint().await;
    let client = MemoryEngineClient::new(MemoryEngineClientConfig {
        endpoint,
        shared_secret: "test-secret".to_owned(),
        timeout: Duration::from_millis(100),
        queue_capacity: 4,
    });
    let server = TestServer::start_with_memory_engine_client(client).await;
    let token = server.token(Uuid::new_v4(), "human");
    let mut socket = server.connect(&token).await;
    join(&mut socket, room_id, None).await;
    let mut params = common_params(Uuid::new_v4(), room_id);
    params["text"] = json!("hello");
    send_json(&mut socket, rpc("chat", "chat.send", params)).await;
    let source = recv_json(&mut socket).await;
    assert_eq!(source["eventType"], "message.created");
    let event_id = source["eventId"].as_str().unwrap().to_owned();
    let (headers, body) = timeout(Duration::from_secs(1), received)
        .await
        .expect("the memory endpoint should receive the committed event")
        .expect("the recording endpoint should stay alive");
    assert_eq!(headers.get("authorization").unwrap(), "Bearer test-secret");
    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["roomId"], room_id.to_string());
    assert_eq!(body["eventId"], event_id);
    let persisted: i64 =
        sqlx::query_scalar("SELECT count(*) FROM room_events WHERE room_id = $1 AND event_id = $2")
            .bind(room_id)
            .bind(Uuid::parse_str(&event_id).unwrap())
            .fetch_one(&server.pool)
            .await
            .unwrap();
    assert_eq!(persisted, 1);
    let active_decisions: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM decisions WHERE room_id = $1 AND status = 'active'",
    )
    .bind(room_id)
    .fetch_one(&server.pool)
    .await
    .unwrap();
    assert_eq!(active_decisions, 0);
}

#[tokio::test]
async fn memory_engine_failure_does_not_reject_a_previously_committed_event() {
    let client = MemoryEngineClient::new(MemoryEngineClientConfig {
        endpoint: "http://127.0.0.1:9/internal/v1/ingest".to_owned(),
        shared_secret: "test-secret".to_owned(),
        timeout: Duration::from_millis(20),
        queue_capacity: 1,
    });
    let server = TestServer::start_with_memory_engine_client(client).await;
    let room_id = Uuid::new_v4();
    let token = server.token(Uuid::new_v4(), "human");
    let mut socket = server.connect(&token).await;
    join(&mut socket, room_id, None).await;
    let mut params = common_params(Uuid::new_v4(), room_id);
    params["text"] = json!("hello");
    send_json(&mut socket, rpc("chat", "chat.send", params)).await;
    assert_eq!(recv_json(&mut socket).await["eventType"], "message.created");
}
