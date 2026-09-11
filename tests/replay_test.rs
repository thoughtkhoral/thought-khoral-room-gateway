mod support;

use serde_json::json;
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
