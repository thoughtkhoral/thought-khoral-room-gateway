mod support;

use chrono::Utc;
use serde_json::json;
use thought_khoral_room_gateway::{NewEvent, append_event};
use uuid::Uuid;

use support::{TestServer, common_params, join, recv_json, rpc, send_json};

// This fails if reconnect replay ignores the cursor or returns events out of order.
#[tokio::test]
async fn reconnect_replays_only_events_after_the_sequence_cursor() {
    let server = TestServer::start().await;
    let room_id = Uuid::new_v4();
    let token = server.token(Uuid::new_v4(), "human");
    let mut socket = server.connect(&token).await;
    join(&mut socket, room_id, None).await;

    for number in 1..=3 {
        let mut params = common_params(Uuid::new_v4(), room_id);
        params["text"] = json!(format!("message {number}"));
        send_json(
            &mut socket,
            rpc(&format!("chat-{number}"), "chat.send", params),
        )
        .await;
        let event = recv_json(&mut socket).await;
        assert_eq!(event["sequence"], number);
    }
    drop(socket);

    let mut reconnected = server.connect(&token).await;
    let joined = join(&mut reconnected, room_id, Some(1)).await;
    let events = joined["result"]["events"].as_array().unwrap();

    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["sequence"], 2);
    assert_eq!(events[1]["sequence"], 3);
}

// This fails if reverse-scheduled publication can advance past and discard an earlier committed sequence.
#[tokio::test]
async fn concurrent_commits_published_in_reverse_are_delivered_in_sequence_order() {
    let server = TestServer::start().await;
    let room_id = Uuid::new_v4();
    let actor_id = Uuid::new_v4();
    let token = server.token(actor_id, "human");
    let mut observer = server.connect(&token).await;
    join(&mut observer, room_id, None).await;

    let new_event = || NewEvent {
        room_id,
        request_id: Uuid::new_v4(),
        event_type: "message.created".to_owned(),
        actor_id,
        actor_role: "human".to_owned(),
        payload: json!({ "text": "concurrent publication" }),
        occurred_at: Utc::now(),
    };
    let (first, second) = tokio::join!(
        append_event(&server.pool, new_event()),
        append_event(&server.pool, new_event()),
    );
    let mut committed = [first.unwrap(), second.unwrap()];
    committed.sort_by_key(|event| event.sequence);

    server.state.publish(committed[1].clone());
    server.state.publish(committed[0].clone());

    let delivered_first = recv_json(&mut observer).await;
    let delivered_second = recv_json(&mut observer).await;
    assert_eq!(delivered_first["sequence"], committed[0].sequence);
    assert_eq!(delivered_second["sequence"], committed[1].sequence);
}
