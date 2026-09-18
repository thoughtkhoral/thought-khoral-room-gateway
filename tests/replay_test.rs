mod support;

use chrono::Utc;
use serde_json::json;
use thought_khoral_room_gateway::{NewEvent, append_event};
use uuid::Uuid;

use support::{TestServer, common_params, join, recv_json, rpc, send_json};

async fn drain_participant_updates(socket: &mut support::TestSocket) {
    while let Ok(update) =
        tokio::time::timeout(std::time::Duration::from_millis(150), recv_json(socket)).await
    {
        assert_eq!(update["method"], "room.participants.updated");
    }
}

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

// This fails if replay exposes mentioned events outside their audience or renumbers later events.
#[tokio::test]
async fn replay_skips_hidden_targeted_events_without_creating_sequence_gaps() {
    let server = TestServer::start().await;
    let room_id = Uuid::new_v4();
    let sender_id = Uuid::new_v4();
    let observer_id = Uuid::new_v4();
    let target_id = Uuid::new_v4();
    let sender_token = server.token(sender_id, "human");
    let observer_token = server.token(observer_id, "human");
    let mut target_options = support::TokenOptions::valid(target_id, "agent");
    target_options.name = Some("Target".to_owned());
    let target_token = server.token_with(target_options);
    let mut sender = server.connect(&sender_token).await;
    let mut observer = server.connect(&observer_token).await;
    let mut target = server.connect(&target_token).await;
    join(&mut sender, room_id, None).await;
    join(&mut observer, room_id, None).await;
    join(&mut target, room_id, None).await;
    drain_participant_updates(&mut sender).await;
    drain_participant_updates(&mut observer).await;
    drain_participant_updates(&mut target).await;

    let mut targeted = common_params(Uuid::new_v4(), room_id);
    targeted["text"] = json!("Only the target agent may read this.");
    targeted["mentions"] = json!([{ "type": "participant", "id": target_id, "token": "target" }]);
    targeted["delivery"] = json!("mentioned");
    send_json(&mut sender, rpc("targeted", "chat.send", targeted)).await;
    let targeted_event = recv_json(&mut sender).await;
    assert_eq!(targeted_event["sequence"], 1);
    assert_eq!(recv_json(&mut target).await, targeted_event);

    let mut public = common_params(Uuid::new_v4(), room_id);
    public["text"] = json!("Everyone may read this.");
    send_json(&mut sender, rpc("public", "chat.send", public)).await;
    let public_event = recv_json(&mut sender).await;
    assert_eq!(public_event["sequence"], 2);
    assert_eq!(recv_json(&mut observer).await, public_event);
    assert_eq!(recv_json(&mut target).await, public_event);
    drop(observer);
    drop(target);

    let mut reconnected_observer = server.connect(&observer_token).await;
    let observer_events = join(&mut reconnected_observer, room_id, Some(0)).await;
    assert_eq!(observer_events["result"]["events"], json!([public_event]));

    let mut reconnected_target = server.connect(&target_token).await;
    let target_events = join(&mut reconnected_target, room_id, Some(0)).await;
    assert_eq!(
        target_events["result"]["events"],
        json!([targeted_event, public_event])
    );
}

// This fails if reverse-scheduled recovery leaks a hidden event or fails to advance the cursor past it.
#[tokio::test]
async fn reverse_live_recovery_skips_hidden_event_and_advances_cursor() {
    let server = TestServer::start().await;
    let room_id = Uuid::new_v4();
    let actor_id = Uuid::new_v4();
    let target_id = Uuid::new_v4();
    let token = server.token(actor_id, "human");
    let mut observer = server.connect(&token).await;
    join(&mut observer, room_id, None).await;

    let new_event = |payload| NewEvent {
        room_id,
        request_id: Uuid::new_v4(),
        event_type: "message.created".to_owned(),
        actor_id,
        actor_role: "human".to_owned(),
        actor_display_name: Some("Test Human".to_owned()),
        payload,
        occurred_at: Utc::now(),
    };
    let hidden = append_event(
        &server.pool,
        new_event(json!({
            "text": "hidden",
            "delivery": "mentioned",
            "mentions": [{ "type": "participant", "id": target_id, "token": "target" }],
            "audienceIds": [target_id],
        })),
    )
    .await
    .unwrap();
    let public = append_event(&server.pool, new_event(json!({ "text": "public" })))
        .await
        .unwrap();

    server.state.publish(public.clone());
    server.state.publish(hidden);
    assert_eq!(recv_json(&mut observer).await, public.to_wire_value());
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(150),
            recv_json(&mut observer)
        )
        .await
        .is_err()
    );

    let later = append_event(&server.pool, new_event(json!({ "text": "later" })))
        .await
        .unwrap();
    server.state.publish(later.clone());
    assert_eq!(recv_json(&mut observer).await, later.to_wire_value());
}

// This fails if replay drops the deletion audit event or returns it out of sequence.
#[tokio::test]
async fn reconnect_replays_decision_proposal_and_deletion_audit() {
    let server = TestServer::start().await;
    let room_id = Uuid::new_v4();
    let token = server.token(Uuid::new_v4(), "human");
    let mut socket = server.connect(&token).await;
    join(&mut socket, room_id, None).await;

    let mut proposal = common_params(Uuid::new_v4(), room_id);
    proposal["title"] = json!("Replay this draft");
    proposal["summary"] = json!("Replay the deletion too.");
    proposal["sourceEventIds"] = json!([]);
    send_json(&mut socket, rpc("proposal", "decision.propose", proposal)).await;
    let proposed = recv_json(&mut socket).await;
    let decision_id = proposed["payload"]["decisionId"].clone();
    let mut delete = common_params(Uuid::new_v4(), room_id);
    delete["decisionId"] = decision_id;
    send_json(&mut socket, rpc("delete", "decision.delete", delete)).await;
    assert_eq!(
        recv_json(&mut socket).await["eventType"],
        "decision.deleted"
    );
    drop(socket);

    let mut reconnected = server.connect(&token).await;
    let joined = join(&mut reconnected, room_id, Some(0)).await;
    let events = joined["result"]["events"].as_array().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["eventType"], "decision.proposed");
    assert_eq!(events[1]["eventType"], "decision.deleted");
    assert!(events[0]["sequence"].as_i64().unwrap() < events[1]["sequence"].as_i64().unwrap());
}
