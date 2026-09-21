mod support;

use serde_json::json;
use sqlx::Row;
use thought_khoral_room_gateway::action_items::ACTION_ITEMS_AGENT_ID;
use uuid::Uuid;

use support::{TestServer, common_params, join, recv_json, rpc, send_json};

fn sorted_ids(ids: impl IntoIterator<Item = Uuid>) -> Vec<String> {
    let mut ids = ids.into_iter().map(|id| id.to_string()).collect::<Vec<_>>();
    ids.sort();
    ids
}

async fn no_message_arrives(socket: &mut support::TestSocket) {
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(150), recv_json(socket))
            .await
            .is_err(),
        "a participant outside the message audience must not receive an event"
    );
}

async fn drain_participant_updates(socket: &mut support::TestSocket) {
    while let Ok(update) =
        tokio::time::timeout(std::time::Duration::from_millis(150), recv_json(socket)).await
    {
        assert_eq!(update["method"], "room.participants.updated");
    }
}

async fn assert_request_was_not_persisted(server: &TestServer, room_id: Uuid, request_id: Uuid) {
    let event_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM room_events WHERE room_id = $1 AND request_id = $2",
    )
    .bind(room_id)
    .bind(request_id)
    .fetch_one(&server.pool)
    .await
    .unwrap();
    assert_eq!(event_count, 0);

    let ledger_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM room_requests WHERE room_id = $1 AND request_id = $2",
    )
    .bind(room_id)
    .bind(request_id)
    .fetch_one(&server.pool)
    .await
    .unwrap();
    assert_eq!(ledger_count, 0);
}

#[tokio::test]
async fn join_returns_named_participants_and_presence_updates() {
    let server = TestServer::start().await;
    let room_id = Uuid::new_v4();
    let first_id = Uuid::new_v4();
    let mut first_options = support::TokenOptions::valid(first_id, "human");
    first_options.name = Some("Maya Chen".to_owned());
    let mut first = server.connect(&server.token_with(first_options)).await;

    let joined = join(&mut first, room_id, None).await;
    let maya = joined["result"]["participants"]
        .as_array()
        .unwrap()
        .iter()
        .find(|participant| participant["displayName"] == "Maya Chen")
        .expect("the joined human must be present in the roster");
    assert_eq!(maya["online"], true);

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
        3
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

// This fails if a direct mention of the registered agent does not create one
// replayable lifecycle and structured result alongside the source message.
#[tokio::test]
async fn human_action_items_mention_emits_one_task_lifecycle() {
    let server = TestServer::start().await;
    let room_id = Uuid::new_v4();
    let token = server.token(Uuid::new_v4(), "human");
    let mut socket = server.connect(&token).await;
    let joined = join(&mut socket, room_id, None).await;
    assert!(
        joined["result"]["participants"]
            .as_array()
            .unwrap()
            .iter()
            .any(|participant| { participant["id"] == ACTION_ITEMS_AGENT_ID.to_string() })
    );

    let mut params = common_params(Uuid::new_v4(), room_id);
    params["text"] =
        json!("@action-items\n- Prepare rollout checklist | owner: Maya | due: Friday");
    params["mentions"] = json!([{
        "type": "participant",
        "id": ACTION_ITEMS_AGENT_ID,
        "token": "action-items"
    }]);
    params["delivery"] = json!("room");
    send_json(&mut socket, rpc("task", "chat.send", params)).await;

    let message = recv_json(&mut socket).await;
    let queued = recv_json(&mut socket).await;
    let running = recv_json(&mut socket).await;
    let completed = recv_json(&mut socket).await;
    assert_eq!(message["eventType"], "message.created");
    assert_eq!(queued["eventType"], "agent.task.queued");
    assert_eq!(running["eventType"], "agent.task.running");
    assert_eq!(completed["eventType"], "agent.task.succeeded");
    assert_eq!(
        completed["payload"]["result"]["actionItems"][0]["owner"],
        "Maya"
    );
    assert_eq!(
        queued["payload"]["agentId"],
        ACTION_ITEMS_AGENT_ID.to_string()
    );
}

// This fails if starting an external task runs it inline or omits its durable requested event.
#[tokio::test]
async fn human_task_start_emits_one_requested_event_without_inline_execution() {
    let server = TestServer::start().await;
    let room_id = Uuid::new_v4();
    let requester_id = Uuid::new_v4();
    let mut socket = server.connect(&server.token(requester_id, "human")).await;
    join(&mut socket, room_id, None).await;

    let request_id = Uuid::new_v4();
    let mut params = common_params(request_id, room_id);
    params["agentId"] = json!("74686f75-6768-746b-686f-72616c000003");
    params["skillId"] = json!("summarize-context");
    params["input"] = json!("Summarize the room.");
    send_json(&mut socket, rpc("task", "agent.task.start", params.clone())).await;

    let requested = recv_json(&mut socket).await;
    assert_eq!(requested["eventType"], "agent.task.requested");
    assert_eq!(
        requested["payload"]["requesterId"],
        requester_id.to_string()
    );
    assert_eq!(
        requested["payload"]["contextRevision"],
        requested["sequence"]
    );

    send_json(&mut socket, rpc("task-retry", "agent.task.start", params)).await;
    assert_eq!(recv_json(&mut socket).await, requested);
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM agent_tasks WHERE room_id = $1 AND request_id = $2",
    )
    .bind(room_id)
    .bind(request_id)
    .fetch_one(&server.pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
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

// This fails if a mentioned message is broadcast to humans outside its direct audience.
#[tokio::test]
async fn direct_agent_message_is_visible_only_to_the_sender_and_target_agent() {
    let server = TestServer::start().await;
    let room_id = Uuid::new_v4();
    let sender_id = Uuid::new_v4();
    let observer_id = Uuid::new_v4();
    let agent_id = Uuid::new_v4();
    let mut sender = server.connect(&server.token(sender_id, "human")).await;
    let mut observer = server.connect(&server.token(observer_id, "human")).await;
    let mut agent_options = support::TokenOptions::valid(agent_id, "agent");
    agent_options.name = Some("Atlas".to_owned());
    let mut agent = server.connect(&server.token_with(agent_options)).await;
    join(&mut sender, room_id, None).await;
    join(&mut observer, room_id, None).await;
    join(&mut agent, room_id, None).await;
    drain_participant_updates(&mut sender).await;
    drain_participant_updates(&mut observer).await;
    drain_participant_updates(&mut agent).await;

    let mut params = common_params(Uuid::new_v4(), room_id);
    params["text"] = json!("Atlas, please review this privately.");
    params["mentions"] = json!([{ "type": "participant", "id": agent_id, "token": "atlas" }]);
    params["delivery"] = json!("mentioned");
    send_json(&mut sender, rpc("direct-agent", "chat.send", params)).await;

    let event = recv_json(&mut sender).await;
    assert_eq!(event["payload"]["delivery"], "mentioned");
    assert_eq!(recv_json(&mut agent).await, event);
    no_message_arrives(&mut observer).await;
}

// This fails if an all-agents mention is not visible to every participant in the room.
#[tokio::test]
async fn all_agents_message_is_visible_to_agents_and_humans() {
    let server = TestServer::start().await;
    let room_id = Uuid::new_v4();
    let sender_id = Uuid::new_v4();
    let observer_id = Uuid::new_v4();
    let agent_id = Uuid::new_v4();
    let mut sender = server.connect(&server.token(sender_id, "human")).await;
    let mut observer = server.connect(&server.token(observer_id, "human")).await;
    let mut agent = server.connect(&server.token(agent_id, "agent")).await;
    join(&mut sender, room_id, None).await;
    join(&mut observer, room_id, None).await;
    join(&mut agent, room_id, None).await;
    drain_participant_updates(&mut sender).await;
    drain_participant_updates(&mut observer).await;
    drain_participant_updates(&mut agent).await;

    let mut params = common_params(Uuid::new_v4(), room_id);
    params["text"] = json!("@allagents, everyone should see this.");
    params["mentions"] = json!([{ "type": "alias", "alias": "allagents" }]);
    params["delivery"] = json!("mentioned");
    send_json(&mut sender, rpc("all-agents-live", "chat.send", params)).await;

    let event = recv_json(&mut sender).await;
    assert_eq!(recv_json(&mut observer).await, event);
    assert_eq!(recv_json(&mut agent).await, event);
}

// This fails if a mentioned all-humans message is broadcast to an agent.
#[tokio::test]
async fn all_humans_message_is_visible_only_to_humans() {
    let server = TestServer::start().await;
    let room_id = Uuid::new_v4();
    let sender_id = Uuid::new_v4();
    let observer_id = Uuid::new_v4();
    let agent_id = Uuid::new_v4();
    let mut sender = server.connect(&server.token(sender_id, "human")).await;
    let mut observer = server.connect(&server.token(observer_id, "human")).await;
    let mut agent = server.connect(&server.token(agent_id, "agent")).await;
    join(&mut sender, room_id, None).await;
    join(&mut observer, room_id, None).await;
    join(&mut agent, room_id, None).await;
    drain_participant_updates(&mut sender).await;
    drain_participant_updates(&mut observer).await;
    drain_participant_updates(&mut agent).await;

    let mut params = common_params(Uuid::new_v4(), room_id);
    params["text"] = json!("@allhumans, this is for people only.");
    params["mentions"] = json!([{ "type": "alias", "alias": "allhumans" }]);
    params["delivery"] = json!("mentioned");
    send_json(&mut sender, rpc("all-humans-live", "chat.send", params)).await;

    let event = recv_json(&mut sender).await;
    assert_eq!(recv_json(&mut observer).await, event);
    no_message_arrives(&mut agent).await;
}

// This fails if a targeted chat omits any direct participant or fails to persist its mention data.
#[tokio::test]
async fn chat_mentions_resolve_direct_participants_and_persist_normalized_payload() {
    let server = TestServer::start().await;
    let room_id = Uuid::new_v4();
    let sender_id = Uuid::new_v4();
    let first_target_id = Uuid::new_v4();
    let second_target_id = Uuid::new_v4();
    let mut sender = server.connect(&server.token(sender_id, "human")).await;
    let mut first_target_options = support::TokenOptions::valid(first_target_id, "human");
    first_target_options.name = Some("Maya Chen".to_owned());
    let mut first_target = server
        .connect(&server.token_with(first_target_options))
        .await;
    let mut second_target_options = support::TokenOptions::valid(second_target_id, "agent");
    second_target_options.name = Some("Atlas Planner".to_owned());
    let mut second_target = server
        .connect(&server.token_with(second_target_options))
        .await;
    join(&mut sender, room_id, None).await;
    join(&mut first_target, room_id, None).await;
    join(&mut second_target, room_id, None).await;
    recv_json(&mut sender).await;
    recv_json(&mut sender).await;

    let mut params = common_params(Uuid::new_v4(), room_id);
    params["text"] = json!("Maya and Atlas, please review.");
    params["mentions"] = json!([
        { "type": "participant", "id": first_target_id, "token": "maya-chen" },
        { "type": "participant", "id": second_target_id, "token": "atlas-planner" }
    ]);
    params["delivery"] = json!("mentioned");
    send_json(&mut sender, rpc("mentioned", "chat.send", params)).await;

    let event = recv_json(&mut sender).await;
    assert_eq!(event["eventType"], "message.created");
    assert_eq!(event["payload"]["text"], "Maya and Atlas, please review.");
    assert_eq!(
        event["payload"]["mentions"],
        json!([
            { "type": "participant", "id": first_target_id, "token": "maya-chen" },
            { "type": "participant", "id": second_target_id, "token": "atlas-planner" }
        ])
    );
    assert_eq!(event["payload"]["delivery"], "mentioned");
    assert_eq!(
        event["payload"]["audienceIds"],
        json!(sorted_ids([sender_id, first_target_id, second_target_id]))
    );
}

// This fails if an unknown direct mention can create a message or idempotency ledger row.
#[tokio::test]
async fn chat_mentions_reject_unknown_direct_participant_without_persistence() {
    let server = TestServer::start().await;
    let room_id = Uuid::new_v4();
    let request_id = Uuid::new_v4();
    let mut sender = server.connect(&server.token(Uuid::new_v4(), "human")).await;
    join(&mut sender, room_id, None).await;

    let mut params = common_params(request_id, room_id);
    params["text"] = json!("This target never joined.");
    params["mentions"] = json!([
        { "type": "participant", "id": Uuid::new_v4(), "token": "unknown" }
    ]);
    params["delivery"] = json!("mentioned");
    send_json(&mut sender, rpc("unknown", "chat.send", params)).await;

    assert_eq!(recv_json(&mut sender).await["error"]["code"], -32013);
    let event_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM room_events WHERE room_id = $1 AND request_id = $2",
    )
    .bind(room_id)
    .bind(request_id)
    .fetch_one(&server.pool)
    .await
    .unwrap();
    assert_eq!(event_count, 0);
    let ledger_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM room_requests WHERE room_id = $1 AND request_id = $2",
    )
    .bind(room_id)
    .bind(request_id)
    .fetch_one(&server.pool)
    .await
    .unwrap();
    assert_eq!(ledger_count, 0);
}

// This fails if global delivery bypasses direct target validation and persists an unknown mention.
#[tokio::test]
async fn chat_mentions_reject_unknown_direct_participant_in_room_delivery() {
    let server = TestServer::start().await;
    let room_id = Uuid::new_v4();
    let request_id = Uuid::new_v4();
    let mut sender = server.connect(&server.token(Uuid::new_v4(), "human")).await;
    join(&mut sender, room_id, None).await;

    let mut params = common_params(request_id, room_id);
    params["text"] = json!("This global message has an unknown mention.");
    params["mentions"] = json!([
        { "type": "participant", "id": Uuid::new_v4(), "token": "unknown" }
    ]);
    params["delivery"] = json!("room");
    send_json(&mut sender, rpc("unknown-room", "chat.send", params)).await;

    assert_eq!(recv_json(&mut sender).await["error"]["code"], -32013);
    let event_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM room_events WHERE room_id = $1 AND request_id = $2",
    )
    .bind(room_id)
    .bind(request_id)
    .fetch_one(&server.pool)
    .await
    .unwrap();
    assert_eq!(event_count, 0);
    let ledger_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM room_requests WHERE room_id = $1 AND request_id = $2",
    )
    .bind(room_id)
    .bind(request_id)
    .fetch_one(&server.pool)
    .await
    .unwrap();
    assert_eq!(ledger_count, 0);
}

// Exact duplicate direct targets are rejected structurally before persistence.
#[tokio::test]
async fn chat_mentions_reject_duplicate_direct_participant_without_persistence() {
    let server = TestServer::start().await;
    let room_id = Uuid::new_v4();
    let request_id = Uuid::new_v4();
    let target_id = Uuid::new_v4();
    let mut sender = server.connect(&server.token(Uuid::new_v4(), "human")).await;
    let mut target_options = support::TokenOptions::valid(target_id, "human");
    target_options.name = Some("Maya Chen".to_owned());
    let mut target = server.connect(&server.token_with(target_options)).await;
    join(&mut sender, room_id, None).await;
    join(&mut target, room_id, None).await;
    drain_participant_updates(&mut sender).await;

    let mut params = common_params(request_id, room_id);
    params["text"] = json!("Do not deliver duplicate direct mentions.");
    params["mentions"] = json!([
        { "type": "participant", "id": target_id, "token": "maya-chen" },
        { "type": "participant", "id": target_id, "token": "maya-chen" }
    ]);
    params["delivery"] = json!("mentioned");
    send_json(&mut sender, rpc("duplicate-direct", "chat.send", params)).await;

    assert_eq!(recv_json(&mut sender).await["error"]["code"], -32600);
    assert_request_was_not_persisted(&server, room_id, request_id).await;
}

// Exact duplicate aliases are rejected structurally before persistence.
#[tokio::test]
async fn chat_mentions_reject_duplicate_alias_without_persistence() {
    let server = TestServer::start().await;
    let room_id = Uuid::new_v4();
    let request_id = Uuid::new_v4();
    let mut sender = server.connect(&server.token(Uuid::new_v4(), "human")).await;
    join(&mut sender, room_id, None).await;

    let mut params = common_params(request_id, room_id);
    params["text"] = json!("Do not deliver duplicate aliases.");
    params["mentions"] = json!([
        { "type": "alias", "alias": "allhumans" },
        { "type": "alias", "alias": "allhumans" }
    ]);
    params["delivery"] = json!("mentioned");
    send_json(&mut sender, rpc("duplicate-alias", "chat.send", params)).await;

    assert_eq!(recv_json(&mut sender).await["error"]["code"], -32600);
    assert_request_was_not_persisted(&server, room_id, request_id).await;
}

// This fails if direct mentions can use a valid-looking token instead of the roster token.
#[tokio::test]
async fn chat_mentions_reject_noncanonical_direct_token_in_room_delivery_without_persistence() {
    let server = TestServer::start().await;
    let room_id = Uuid::new_v4();
    let request_id = Uuid::new_v4();
    let target_id = Uuid::new_v4();
    let mut sender = server.connect(&server.token(Uuid::new_v4(), "human")).await;
    let mut target_options = support::TokenOptions::valid(target_id, "human");
    target_options.name = Some("Maya Chen".to_owned());
    let mut target = server.connect(&server.token_with(target_options)).await;
    join(&mut sender, room_id, None).await;
    join(&mut target, room_id, None).await;
    drain_participant_updates(&mut sender).await;

    let mut params = common_params(request_id, room_id);
    params["text"] = json!("Do not accept a truncated roster token.");
    params["mentions"] = json!([{ "type": "participant", "id": target_id, "token": "maya" }]);
    params["delivery"] = json!("room");
    send_json(&mut sender, rpc("wrong-token-room", "chat.send", params)).await;

    assert_eq!(recv_json(&mut sender).await["error"]["code"], -32013);
    assert_request_was_not_persisted(&server, room_id, request_id).await;

    let mentioned_request_id = Uuid::new_v4();
    let mut mentioned_params = common_params(mentioned_request_id, room_id);
    mentioned_params["text"] = json!("Do not accept a truncated roster token privately.");
    mentioned_params["mentions"] =
        json!([{ "type": "participant", "id": target_id, "token": "maya" }]);
    mentioned_params["delivery"] = json!("mentioned");
    send_json(
        &mut sender,
        rpc("wrong-token-mentioned", "chat.send", mentioned_params),
    )
    .await;

    assert_eq!(recv_json(&mut sender).await["error"]["code"], -32013);
    assert_request_was_not_persisted(&server, room_id, mentioned_request_id).await;
}

// This fails if canonical tokens diverge from UI roster normalization and collision rules.
#[tokio::test]
async fn chat_mentions_accept_ui_canonical_normalized_and_disambiguated_tokens() {
    let server = TestServer::start().await;
    let room_id = Uuid::new_v4();
    let sender_id = Uuid::new_v4();
    let first_id = Uuid::parse_str("12345678-aaaa-4567-8901-abcdef123456").unwrap();
    let second_id = Uuid::parse_str("12345678-bbbb-4567-8901-abcdef123456").unwrap();
    let reserved_id = Uuid::parse_str("f4c0ffee-aaaa-4567-8901-abcdef123456").unwrap();
    let fallback_id = Uuid::parse_str("deadbeef-aaaa-4567-8901-abcdef123456").unwrap();
    let mut sender = server.connect(&server.token(sender_id, "human")).await;
    let mut first_options = support::TokenOptions::valid(first_id, "human");
    first_options.name = Some("M\u{00e4}y\u{00e4} Chen".to_owned());
    let mut first = server.connect(&server.token_with(first_options)).await;
    let mut second_options = support::TokenOptions::valid(second_id, "human");
    second_options.name = Some("maya--chen".to_owned());
    let mut second = server.connect(&server.token_with(second_options)).await;
    let mut reserved_options = support::TokenOptions::valid(reserved_id, "human");
    reserved_options.name = Some("allhumans".to_owned());
    let mut reserved = server.connect(&server.token_with(reserved_options)).await;
    let mut fallback_options = support::TokenOptions::valid(fallback_id, "human");
    fallback_options.name = Some("___".to_owned());
    let mut fallback = server.connect(&server.token_with(fallback_options)).await;
    join(&mut sender, room_id, None).await;
    join(&mut first, room_id, None).await;
    join(&mut second, room_id, None).await;
    join(&mut reserved, room_id, None).await;
    join(&mut fallback, room_id, None).await;
    drain_participant_updates(&mut sender).await;

    let mut params = common_params(Uuid::new_v4(), room_id);
    params["text"] = json!("Canonical roster tokens must be accepted.");
    params["mentions"] = json!([
        { "type": "participant", "id": first_id, "token": "maya-chen-12345678a" },
        { "type": "participant", "id": second_id, "token": "maya-chen-12345678b" },
        { "type": "participant", "id": reserved_id, "token": "allhumans-f4c0ffee" },
        { "type": "participant", "id": fallback_id, "token": "participant" }
    ]);
    params["delivery"] = json!("mentioned");
    send_json(&mut sender, rpc("canonical-tokens", "chat.send", params)).await;

    assert_eq!(recv_json(&mut sender).await["eventType"], "message.created");
}

// This fails if allhumans uses display names or includes agents in the targeted audience.
#[tokio::test]
async fn chat_mentions_expand_allhumans_by_role_only() {
    let server = TestServer::start().await;
    let room_id = Uuid::new_v4();
    let sender_id = Uuid::new_v4();
    let first_human_id = Uuid::new_v4();
    let second_human_id = Uuid::new_v4();
    let agent_id = Uuid::new_v4();
    let mut sender_options = support::TokenOptions::valid(sender_id, "agent");
    sender_options.preferred_username = Some("Human-Looking Sender".to_owned());
    let mut agent_options = support::TokenOptions::valid(agent_id, "agent");
    agent_options.preferred_username = Some("Maya Chen".to_owned());
    let mut sender = server.connect(&server.token_with(sender_options)).await;
    let mut first_human = server.connect(&server.token(first_human_id, "human")).await;
    let mut second_human = server
        .connect(&server.token(second_human_id, "human"))
        .await;
    let mut agent = server.connect(&server.token_with(agent_options)).await;
    join(&mut sender, room_id, None).await;
    join(&mut first_human, room_id, None).await;
    join(&mut second_human, room_id, None).await;
    join(&mut agent, room_id, None).await;
    recv_json(&mut sender).await;
    recv_json(&mut sender).await;
    recv_json(&mut sender).await;

    let mut params = common_params(Uuid::new_v4(), room_id);
    params["text"] = json!("Humans only.");
    params["mentions"] = json!([{ "type": "alias", "alias": "allhumans" }]);
    params["delivery"] = json!("mentioned");
    send_json(&mut sender, rpc("all-humans", "chat.send", params)).await;

    let event = recv_json(&mut sender).await;
    assert_eq!(
        event["payload"]["audienceIds"],
        json!(sorted_ids([sender_id, first_human_id, second_human_id]))
    );
}

// This fails if allagents excludes humans or includes participants based on display-name text.
#[tokio::test]
async fn chat_mentions_expand_allagents_to_agents_and_humans() {
    let server = TestServer::start().await;
    let room_id = Uuid::new_v4();
    let sender_id = Uuid::new_v4();
    let human_id = Uuid::new_v4();
    let first_agent_id = Uuid::new_v4();
    let second_agent_id = Uuid::new_v4();
    let mut sender = server.connect(&server.token(sender_id, "human")).await;
    let mut human = server.connect(&server.token(human_id, "human")).await;
    let mut first_agent = server.connect(&server.token(first_agent_id, "agent")).await;
    let mut second_agent = server
        .connect(&server.token(second_agent_id, "agent"))
        .await;
    join(&mut sender, room_id, None).await;
    join(&mut human, room_id, None).await;
    join(&mut first_agent, room_id, None).await;
    join(&mut second_agent, room_id, None).await;
    recv_json(&mut sender).await;
    recv_json(&mut sender).await;
    recv_json(&mut sender).await;

    let mut params = common_params(Uuid::new_v4(), room_id);
    params["text"] = json!("All agents and humans.");
    params["mentions"] = json!([{ "type": "alias", "alias": "allagents" }]);
    params["delivery"] = json!("mentioned");
    send_json(&mut sender, rpc("all-agents", "chat.send", params)).await;

    let event = recv_json(&mut sender).await;
    assert_eq!(
        event["payload"]["audienceIds"],
        json!(sorted_ids([
            sender_id,
            human_id,
            first_agent_id,
            second_agent_id,
            ACTION_ITEMS_AGENT_ID
        ]))
    );
}

// This fails if direct and alias references to the same participant remain duplicated.
#[tokio::test]
async fn chat_mentions_deduplicate_direct_and_alias_targets() {
    let server = TestServer::start().await;
    let room_id = Uuid::new_v4();
    let sender_id = Uuid::new_v4();
    let target_id = Uuid::new_v4();
    let mut sender = server.connect(&server.token(sender_id, "human")).await;
    let mut target_options = support::TokenOptions::valid(target_id, "human");
    target_options.name = Some("Maya Chen".to_owned());
    let mut target = server.connect(&server.token_with(target_options)).await;
    join(&mut sender, room_id, None).await;
    join(&mut target, room_id, None).await;
    recv_json(&mut sender).await;

    let mut params = common_params(Uuid::new_v4(), room_id);
    params["text"] = json!("No duplicate audience entries.");
    params["mentions"] = json!([
        { "type": "participant", "id": target_id, "token": "maya-chen" },
        { "type": "alias", "alias": "allhumans" }
    ]);
    params["delivery"] = json!("mentioned");
    send_json(&mut sender, rpc("deduplicate", "chat.send", params)).await;

    assert_eq!(
        recv_json(&mut sender).await["payload"]["audienceIds"],
        json!(sorted_ids([sender_id, target_id]))
    );
}

// This fails if a sender cannot see their own targeted message without naming themselves.
#[tokio::test]
async fn chat_mentions_include_the_sender_in_a_targeted_audience() {
    let server = TestServer::start().await;
    let room_id = Uuid::new_v4();
    let sender_id = Uuid::new_v4();
    let target_id = Uuid::new_v4();
    let mut sender = server.connect(&server.token(sender_id, "agent")).await;
    let mut target_options = support::TokenOptions::valid(target_id, "human");
    target_options.name = Some("Maya Chen".to_owned());
    let mut target = server.connect(&server.token_with(target_options)).await;
    join(&mut sender, room_id, None).await;
    join(&mut target, room_id, None).await;
    recv_json(&mut sender).await;

    let mut params = common_params(Uuid::new_v4(), room_id);
    params["text"] = json!("The sender is implicit.");
    params["mentions"] = json!([
        { "type": "participant", "id": target_id, "token": "maya-chen" }
    ]);
    params["delivery"] = json!("mentioned");
    send_json(&mut sender, rpc("sender", "chat.send", params)).await;

    assert_eq!(
        recv_json(&mut sender).await["payload"]["audienceIds"],
        json!(sorted_ids([sender_id, target_id]))
    );
}

// This fails if room delivery is narrowed by mention targets or lacks its explicit global marker.
#[tokio::test]
async fn chat_mentions_keep_room_delivery_global_with_an_empty_audience() {
    let server = TestServer::start().await;
    let room_id = Uuid::new_v4();
    let sender_id = Uuid::new_v4();
    let observer_id = Uuid::new_v4();
    let mut sender = server.connect(&server.token(sender_id, "human")).await;
    let mut observer_options = support::TokenOptions::valid(observer_id, "agent");
    observer_options.name = Some("Atlas Planner".to_owned());
    let mut observer = server.connect(&server.token_with(observer_options)).await;
    join(&mut sender, room_id, None).await;
    join(&mut observer, room_id, None).await;
    recv_json(&mut sender).await;
    recv_json(&mut observer).await;

    let mut params = common_params(Uuid::new_v4(), room_id);
    params["text"] = json!("The entire room can see this.");
    params["mentions"] = json!([
        { "type": "participant", "id": observer_id, "token": "atlas-planner" }
    ]);
    params["delivery"] = json!("room");
    send_json(&mut sender, rpc("room", "chat.send", params)).await;

    let sender_event = recv_json(&mut sender).await;
    assert_eq!(recv_json(&mut observer).await, sender_event);
    assert_eq!(sender_event["payload"]["delivery"], "room");
    assert_eq!(sender_event["payload"]["audienceIds"], json!([]));
    assert_eq!(
        sender_event["payload"]["mentions"],
        json!([
            { "type": "participant", "id": observer_id, "token": "atlas-planner" }
        ])
    );
}

// This fails if a valid alias without matching role suppresses the sender's targeted message.
#[tokio::test]
async fn chat_mentions_keep_sender_when_an_alias_matches_no_participants() {
    let server = TestServer::start().await;
    let room_id = Uuid::new_v4();
    let sender_id = Uuid::new_v4();
    let mut sender = server.connect(&server.token(sender_id, "human")).await;
    join(&mut sender, room_id, None).await;

    let mut params = common_params(Uuid::new_v4(), room_id);
    params["text"] = json!("There are no agents yet.");
    params["mentions"] = json!([{ "type": "alias", "alias": "allagents" }]);
    params["delivery"] = json!("mentioned");
    send_json(&mut sender, rpc("empty-alias", "chat.send", params)).await;

    assert_eq!(
        recv_json(&mut sender).await["payload"]["audienceIds"],
        json!(sorted_ids([sender_id, ACTION_ITEMS_AGENT_ID]))
    );
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
