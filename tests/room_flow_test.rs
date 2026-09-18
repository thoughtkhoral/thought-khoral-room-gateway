mod support;

use serde_json::json;
use sqlx::Row;
use uuid::Uuid;

use support::{TestServer, common_params, join, recv_json, rpc, send_json};

#[tokio::test]
async fn join_returns_named_participants_and_presence_updates() {
    let server = TestServer::start().await;
    let room_id = Uuid::new_v4();
    let first_id = Uuid::new_v4();
    let mut first_options = support::TokenOptions::valid(first_id, "human");
    first_options.name = Some("Maya Chen".to_owned());
    let mut first = server.connect(&server.token_with(first_options)).await;

    let joined = join(&mut first, room_id, None).await;
    assert_eq!(
        joined["result"]["participants"][0]["displayName"],
        "Maya Chen"
    );
    assert_eq!(joined["result"]["participants"][0]["online"], true);

    let mut message = common_params(Uuid::new_v4(), room_id);
    message["text"] = json!("The join response is complete.");
    send_json(&mut first, rpc("message", "chat.send", message)).await;
    assert_eq!(recv_json(&mut first).await["eventType"], "message.created");

    let second_id = Uuid::new_v4();
    let mut second_options = support::TokenOptions::valid(second_id, "agent");
    second_options.preferred_username = Some("Atlas Planner".to_owned());
    let mut second = server.connect(&server.token_with(second_options)).await;
    let second_joined = join(&mut second, room_id, None).await;
    assert_eq!(
        second_joined["result"]["participants"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        recv_json(&mut first).await["method"],
        "room.participants.updated"
    );

    drop(second);
    let offline_update = recv_json(&mut first).await;
    let participants = offline_update["params"]["participants"].as_array().unwrap();
    assert!(participants.iter().any(|participant| {
        participant["displayName"] == "Atlas Planner" && participant["online"] == false
    }));
}

// This fails if a chat event is broadcast before persistence or clients see divergent events.
#[tokio::test]
async fn two_humans_receive_the_same_persisted_message_sequence() {
    let server = TestServer::start().await;
    let room_id = Uuid::new_v4();
    let token = server.token(Uuid::new_v4(), "human");
    let mut first = server.connect(&token).await;
    let mut second = server.connect(&token).await;
    join(&mut first, room_id, None).await;
    join(&mut second, room_id, None).await;

    let request_id = Uuid::new_v4();
    let mut params = common_params(request_id, room_id);
    params["text"] = json!("Persist this before publishing it.");
    send_json(&mut first, rpc("chat", "chat.send", params)).await;

    let first_event = recv_json(&mut first).await;
    let second_event = recv_json(&mut second).await;
    assert_eq!(first_event, second_event);
    assert_eq!(first_event["eventType"], "message.created");

    let row = sqlx::query(
        "SELECT event_id, sequence FROM room_events WHERE room_id = $1 AND request_id = $2",
    )
    .bind(room_id)
    .bind(request_id)
    .fetch_one(&server.pool)
    .await
    .expect("the event must already be committed when it is observed");
    assert_eq!(
        first_event["eventId"],
        row.try_get::<Uuid, _>("event_id").unwrap().to_string()
    );
    assert_eq!(
        first_event["sequence"],
        row.try_get::<i64, _>("sequence").unwrap()
    );
}

// This fails if an identical retry creates or broadcasts another persisted event.
#[tokio::test]
async fn identical_request_retry_returns_the_original_without_rebroadcasting() {
    let server = TestServer::start().await;
    let room_id = Uuid::new_v4();
    let token = server.token(Uuid::new_v4(), "human");
    let mut sender = server.connect(&token).await;
    let mut observer = server.connect(&token).await;
    join(&mut sender, room_id, None).await;
    join(&mut observer, room_id, None).await;
    let request_id = Uuid::new_v4();
    let mut params = common_params(request_id, room_id);
    params["text"] = json!("Send this exactly once.");

    send_json(&mut sender, rpc("first", "chat.send", params.clone())).await;
    let original = recv_json(&mut sender).await;
    assert_eq!(recv_json(&mut observer).await, original);
    send_json(&mut sender, rpc("retry", "chat.send", params)).await;
    assert_eq!(recv_json(&mut sender).await, original);

    let observer_result = tokio::time::timeout(
        std::time::Duration::from_millis(150),
        recv_json(&mut observer),
    )
    .await;
    assert!(
        observer_result.is_err(),
        "an identical retry must not rebroadcast"
    );
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM room_events WHERE room_id = $1 AND request_id = $2",
    )
    .bind(room_id)
    .bind(request_id)
    .fetch_one(&server.pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
}

// This fails if requestId reuse with changed canonical parameters is treated as a successful retry.
#[tokio::test]
async fn changed_request_with_reused_id_returns_conflicting_duplicate() {
    let server = TestServer::start().await;
    let room_id = Uuid::new_v4();
    let token = server.token(Uuid::new_v4(), "human");
    let mut socket = server.connect(&token).await;
    join(&mut socket, room_id, None).await;
    let request_id = Uuid::new_v4();
    let mut original = common_params(request_id, room_id);
    original["text"] = json!("Original body");
    send_json(&mut socket, rpc("first", "chat.send", original.clone())).await;
    recv_json(&mut socket).await;

    original["text"] = json!("Different body");
    send_json(&mut socket, rpc("conflict", "chat.send", original)).await;
    let error = recv_json(&mut socket).await;
    assert_eq!(error["error"]["code"], -32012);
}

// This fails if deletion is not atomic, does not preserve the audit event, or is not idempotent.
#[tokio::test]
async fn human_delete_physically_removes_draft_and_replays_audit_event() {
    let server = TestServer::start().await;
    let room_id = Uuid::new_v4();
    let token = server.token(Uuid::new_v4(), "human");
    let mut socket = server.connect(&token).await;
    join(&mut socket, room_id, None).await;

    let source_id = Uuid::new_v4();
    let mut proposal = common_params(Uuid::new_v4(), room_id);
    proposal["title"] = json!("Delete this draft");
    proposal["summary"] = json!("The audit snapshot must remain.");
    proposal["sourceEventIds"] = json!([source_id]);
    send_json(&mut socket, rpc("proposal", "decision.propose", proposal)).await;
    let proposed = recv_json(&mut socket).await;
    let decision_id = Uuid::parse_str(proposed["payload"]["decisionId"].as_str().unwrap()).unwrap();

    let delete_request_id = Uuid::new_v4();
    let mut delete = common_params(delete_request_id, room_id);
    delete["decisionId"] = json!(decision_id);
    send_json(
        &mut socket,
        rpc("delete", "decision.delete", delete.clone()),
    )
    .await;
    let deleted = recv_json(&mut socket).await;
    assert_eq!(deleted["eventType"], "decision.deleted");
    assert_eq!(deleted["payload"]["decisionId"], decision_id.to_string());
    assert_eq!(deleted["payload"]["priorStatus"], "draft");
    assert_eq!(deleted["payload"]["title"], "Delete this draft");
    assert_eq!(
        deleted["payload"]["summary"],
        "The audit snapshot must remain."
    );
    assert_eq!(deleted["payload"]["sourceEventIds"], json!([source_id]));

    let decision_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM decisions WHERE room_id = $1 AND decision_id = $2",
    )
    .bind(room_id)
    .bind(decision_id)
    .fetch_one(&server.pool)
    .await
    .unwrap();
    assert_eq!(decision_count, 0);
    let audit_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM room_events WHERE room_id = $1 AND event_type = 'decision.deleted' AND request_id = $2",
    )
    .bind(room_id)
    .bind(delete_request_id)
    .fetch_one(&server.pool)
    .await
    .unwrap();
    assert_eq!(audit_count, 1);

    send_json(&mut socket, rpc("retry", "decision.delete", delete)).await;
    assert_eq!(recv_json(&mut socket).await, deleted);
    let event_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM room_events WHERE room_id = $1 AND request_id = $2",
    )
    .bind(room_id)
    .bind(delete_request_id)
    .fetch_one(&server.pool)
    .await
    .unwrap();
    assert_eq!(event_count, 1);

    let mut missing = common_params(Uuid::new_v4(), room_id);
    missing["decisionId"] = json!(decision_id);
    send_json(&mut socket, rpc("missing", "decision.delete", missing)).await;
    assert_eq!(recv_json(&mut socket).await["error"]["code"], -32004);
}
