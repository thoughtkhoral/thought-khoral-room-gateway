mod support;

use chrono::Utc;
use serde_json::{Value, json};
use sqlx::Row;
use std::{future::Future, pin::Pin, sync::Arc};
use support::{AUDIENCE, BROWSER_ORIGIN, TestServer};
use thought_khoral_room_gateway::{NewEvent, append_event, events_after};
use thought_khoral_room_gateway::{
    conversation_protocol::{CODEX_AGENT_ID, ConversationError, PROFILE_VERSION},
    conversation_service::CatalogQuery,
    conversation_store::ConversationPolicy,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};
use uuid::Uuid;

struct TestCatalog;
impl CatalogQuery for TestCatalog {
    fn page<'a>(
        &'a self,
        _agent: Uuid,
        cursor: Option<String>,
        limit: usize,
    ) -> Pin<Box<dyn Future<Output = Result<Value, ConversationError>> + Send + 'a>> {
        Box::pin(async move {
            if cursor.is_some() || limit == 0 {
                return Err(ConversationError::InvalidTaskInput);
            }
            Ok(
                json!({"profileVersion":PROFILE_VERSION,"catalogRevision":"catalog-1","data":[{"id":"model-a","displayName":"Test model","defaultReasoningEffort":"medium","supportedReasoningEfforts":[{"id":"medium","description":"Test effort"}]}],"nextCursor":null}),
            )
        })
    }
}
fn policy() -> ConversationPolicy {
    serde_json::from_value(json!({"enabled":true,"policyRevision":"policy-1","guidanceRevision":"fixed-1","catalogRevision":"catalog-1","model":"model-a","reasoningEffort":"medium","models":[{"id":"model-a","reasoningEfforts":["medium"]}]})).unwrap()
}
async fn server() -> TestServer {
    let server = TestServer::start_isolated().await;
    server
        .state
        .configure_conversations(policy(), Some(Arc::new(TestCatalog)))
        .await
        .unwrap();
    server
}
fn turn(room: Uuid) -> Value {
    json!({"profileVersion":PROFILE_VERSION,"requestId":Uuid::new_v4(),"roomId":room,"agentId":CODEX_AGENT_ID,"occurredAt":Utc::now(),"text":"@codex-agent What did Maya propose?","mentions":[{"type":"participant","id":CODEX_AGENT_ID,"token":"codex-agent"}],"conversation":{"mode":"new"}})
}
async fn http(
    server: &TestServer,
    method: &str,
    path: &str,
    token: Option<&str>,
    headers: &[(&str, &str)],
    body: Option<&str>,
) -> (u16, Value, String) {
    let mut stream = TcpStream::connect(server.address).await.unwrap();
    let body = body.unwrap_or("");
    let mut request = format!(
        "{method} {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n",
        server.address,
        body.len()
    );
    if let Some(token) = token {
        request.push_str(&format!("Authorization: Bearer {token}\r\n"));
    }
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str("\r\n");
    request.push_str(body);
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = vec![];
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        stream.read_to_end(&mut response),
    )
    .await
    .unwrap()
    .unwrap();
    let response = String::from_utf8(response).unwrap();
    let (head, body) = response.split_once("\r\n\r\n").unwrap();
    (
        head.lines()
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap(),
        serde_json::from_str(body).unwrap_or(Value::Null),
        head.to_ascii_lowercase(),
    )
}
async fn post(
    server: &TestServer,
    path: &str,
    token: &str,
    headers: &[(&str, &str)],
    body: &Value,
) -> (u16, Value, String) {
    http(
        server,
        "POST",
        path,
        Some(token),
        headers,
        Some(&body.to_string()),
    )
    .await
}
fn workload(server: &TestServer) -> String {
    server.agent_gateway_token(AUDIENCE, "thought-khoral-agent-gateway")
}
async fn claim(server: &TestServer) -> Value {
    let result = post(
        server,
        "/internal/agent-conversations/v1/claim",
        &workload(server),
        &[],
        &json!({"agentId":CODEX_AGENT_ID,"leaseOwner":"test-mediator"}),
    )
    .await;
    assert_eq!(result.0, 200, "{}", result.1);
    result.1
}
fn completion(packet: &Value) -> Value {
    json!({"profileVersion":PROFILE_VERSION,"updateId":Uuid::new_v4(),"ordinal":1,"taskId":packet["taskId"],"generation":packet["conversation"]["generation"],"contextDigest":packet["context"]["digest"],"kind":"completed","data":{"kind":"conversation-reply.v1","conversationId":packet["conversation"]["id"],"generation":packet["conversation"]["generation"],"assistantText":"Maya proposed green.","consumedRevision":packet["context"]["revision"],"contextDigest":packet["context"]["digest"],"citations":[],"effectiveSettings":null,"usage":null}})
}
async fn seeded(server: &TestServer, room: Uuid, text: &str, author: Uuid, targeted: bool) -> Uuid {
    append_event(&server.pool,NewEvent{room_id:room,request_id:Uuid::new_v4(),event_type:"message.created".into(),actor_id:author,actor_role:"human".into(),actor_display_name:Some(if text.starts_with("Maya") {"Maya"} else {"Leo"}.into()),payload:json!({"text":text,"delivery":if targeted {"mentioned"} else {"room"},"mentions":[],"audienceIds":if targeted {vec![author]} else {vec![]}}),occurred_at:Utc::now()}).await.unwrap().event_id
}

// Missing route/commit, reconstructing context after acceptance, or accepting a
// replay as new work would fail these real HTTP/database assertions.
#[tokio::test]
async fn authenticated_turn_reply_and_replays_persist_two_ordinary_messages() {
    let server = server().await;
    let room = Uuid::new_v4();
    let token = server.token(Uuid::new_v4(), "human");
    let input = turn(room);
    let first = post(
        &server,
        "/api/agent-conversations/v1/turns",
        &token,
        &[("Origin", BROWSER_ORIGIN)],
        &input,
    )
    .await;
    assert_eq!(first.0, 202, "{}", first.1);
    assert!(
        first
            .2
            .contains("access-control-allow-origin: http://workspace.test")
    );
    assert!(!first.1.to_string().contains("lease"));
    let replay = post(
        &server,
        "/api/agent-conversations/v1/turns",
        &token,
        &[],
        &input,
    )
    .await;
    assert_eq!(first.1, replay.1);
    let claimed = claim(&server).await;
    let packet = &claimed["packet"];
    let lease = claimed["leaseToken"].as_str().unwrap();
    assert_eq!(packet["triggerEventId"], first.1["triggerEventId"]);
    let update = completion(packet);
    let path = format!(
        "/internal/agent-conversations/v1/tasks/{}/updates",
        packet["taskId"].as_str().unwrap()
    );
    let completed = post(
        &server,
        &path,
        &workload(&server),
        &[("x-thought-khoral-lease-token", lease)],
        &update,
    )
    .await;
    assert_eq!(completed.0, 200, "{}", completed.1);
    let replay = post(
        &server,
        &path,
        &workload(&server),
        &[("x-thought-khoral-lease-token", lease)],
        &update,
    )
    .await;
    assert_eq!(completed.1, replay.1);
    let events = events_after(&server.pool, room, 0).await.unwrap();
    assert_eq!(events.len(), 2);
    assert!(
        events
            .iter()
            .all(|event| event.event_type == "message.created")
    );
    assert_eq!(events[1].actor_id, CODEX_AGENT_ID);
    assert_eq!(events[1].payload["delivery"], "room");
    let view = http(
        &server,
        "GET",
        &format!(
            "/api/agent-conversations/v1/rooms/{room}/tasks/{}",
            packet["taskId"].as_str().unwrap()
        ),
        Some(&token),
        &[],
        None,
    )
    .await;
    assert_eq!(view.0, 200);
    assert_eq!(view.1["state"], "completed");
    assert_eq!(view.1["replyEventId"], completed.1["replyEventId"]);
    assert!(!view.1.to_string().contains("lease"));
    let conversation = http(
        &server,
        "GET",
        &format!("/api/agent-conversations/v1/rooms/{room}/agents/{CODEX_AGENT_ID}"),
        Some(&token),
        &[],
        None,
    )
    .await;
    assert_eq!(
        conversation.1["conversation"]["consumedRevision"],
        packet["context"]["revision"]
    );
    assert_eq!(conversation.1["conversation"]["state"], "ready");
}

#[tokio::test]
async fn browser_authentication_origins_and_strict_json_fail_before_persistence() {
    let server = server().await;
    let room = Uuid::new_v4();
    let input = turn(room);
    let token = server.token(Uuid::new_v4(), "human");
    let path = "/api/agent-conversations/v1/turns";
    assert_eq!(
        http(&server, "POST", path, None, &[], Some(&input.to_string()))
            .await
            .0,
        401
    );
    assert_eq!(
        post(
            &server,
            path,
            &server.token(Uuid::new_v4(), "agent"),
            &[],
            &input
        )
        .await
        .0,
        403
    );
    assert_eq!(
        post(
            &server,
            path,
            &token,
            &[("Origin", "http://untrusted.test")],
            &input
        )
        .await
        .0,
        403
    );
    let preflight = http(
        &server,
        "OPTIONS",
        path,
        None,
        &[
            ("Origin", BROWSER_ORIGIN),
            ("Access-Control-Request-Method", "POST"),
            (
                "Access-Control-Request-Headers",
                "authorization,content-type",
            ),
        ],
        None,
    )
    .await;
    assert_eq!(preflight.0, 204);
    let raw = input.to_string();
    let duplicate = format!("{{\"text\":\"spoof\",{}", &raw[1..]);
    assert_eq!(
        http(&server, "POST", path, Some(&token), &[], Some(&duplicate))
            .await
            .0,
        400
    );
    assert!(
        events_after(&server.pool, room, 0)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn claimed_context_keeps_public_baseline_and_delta_with_native_binding() {
    let server = server().await;
    let room = Uuid::new_v4();
    let maya = Uuid::new_v4();
    let leo = Uuid::new_v4();
    let public = seeded(&server, room, "Maya proposed green.", maya, false).await;
    seeded(&server, room, "private secret", leo, true).await;
    let correction = seeded(&server, room, "Leo confirmed green.", leo, false).await;
    let token = server.token(leo, "human");
    let first = post(
        &server,
        "/api/agent-conversations/v1/turns",
        &token,
        &[],
        &turn(room),
    )
    .await;
    assert_eq!(first.0, 202);
    let later = seeded(&server, room, "intervening discussion", maya, false).await;
    let claimed = claim(&server).await;
    let packet = &claimed["packet"];
    assert_eq!(
        packet["context"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["sequence"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        vec![1, 3, 4]
    );
    assert_eq!(packet["context"]["entries"][0]["authorId"], json!(maya));
    assert_eq!(packet["context"]["entries"][1]["authorId"], json!(leo));
    assert_eq!(packet["context"]["entries"][0]["eventId"], json!(public));
    assert_eq!(
        packet["context"]["entries"][1]["eventId"],
        json!(correction)
    );
    assert!(!packet.to_string().contains("private secret"));
    assert!(!packet.to_string().contains("intervening discussion"));
    let update = completion(packet);
    let path = format!(
        "/internal/agent-conversations/v1/tasks/{}/updates",
        packet["taskId"].as_str().unwrap()
    );
    let result = post(
        &server,
        &path,
        &workload(&server),
        &[(
            "x-thought-khoral-lease-token",
            claimed["leaseToken"].as_str().unwrap(),
        )],
        &update,
    )
    .await;
    assert_eq!(result.0, 200);
    let mut next = turn(room);
    next.as_object_mut().unwrap().remove("conversation");
    assert_eq!(
        post(
            &server,
            "/api/agent-conversations/v1/turns",
            &token,
            &[],
            &next
        )
        .await
        .0,
        202
    );
    let delta = claim(&server).await;
    assert_eq!(delta["packet"]["context"]["kind"], "delta");
    assert_eq!(delta["packet"]["context"]["baseRevision"], 4);
    assert_eq!(
        delta["packet"]["context"]["entries"][0]["eventId"],
        json!(later)
    );
    assert_eq!(
        delta["packet"]["context"]["nativeReplyBindings"][0]["eventId"],
        result.1["replyEventId"]
    );
    assert!(
        !delta["packet"]["context"]["entries"]
            .to_string()
            .contains("Maya proposed green.")
    );
}

#[tokio::test]
async fn updates_reject_wrong_binding_citations_and_lease_without_a_reply() {
    let server = server().await;
    let room = Uuid::new_v4();
    let token = server.token(Uuid::new_v4(), "human");
    assert_eq!(
        post(
            &server,
            "/api/agent-conversations/v1/turns",
            &token,
            &[],
            &turn(room)
        )
        .await
        .0,
        202
    );
    let claimed = claim(&server).await;
    let packet = &claimed["packet"];
    let path = format!(
        "/internal/agent-conversations/v1/tasks/{}/updates",
        packet["taskId"].as_str().unwrap()
    );
    let lease = claimed["leaseToken"].as_str().unwrap();
    let reply = completion(packet);
    for (pointer, value, status) in [
        ("/generation", json!(99), 409),
        ("/contextDigest", json!("0".repeat(64)), 409),
        ("/taskId", json!(Uuid::new_v4()), 409),
        ("/data/consumedRevision", json!(99), 409),
        ("/data/citations", json!([Uuid::new_v4()]), 403),
        ("/data/assistantText", json!("🦀".repeat(16385)), 413),
    ] {
        let mut invalid = reply.clone();
        *invalid.pointer_mut(pointer).unwrap() = value;
        assert_eq!(
            post(
                &server,
                &path,
                &workload(&server),
                &[("x-thought-khoral-lease-token", lease)],
                &invalid
            )
            .await
            .0,
            status,
            "{pointer}"
        );
    }
    assert_eq!(
        post(
            &server,
            &path,
            &workload(&server),
            &[("x-thought-khoral-lease-token", &Uuid::new_v4().to_string())],
            &reply
        )
        .await
        .0,
        403
    );
    assert_eq!(events_after(&server.pool, room, 0).await.unwrap().len(), 1);
    let task = packet["taskId"].as_str().unwrap();
    assert_eq!(
        http(
            &server,
            "GET",
            &format!(
                "/api/agent-conversations/v1/rooms/{}/tasks/{task}",
                Uuid::new_v4()
            ),
            Some(&token),
            &[],
            None
        )
        .await
        .0,
        404
    );
    assert_eq!(
        http(
            &server,
            "GET",
            &format!("/internal/agent-conversations/v1/tasks/{task}/context"),
            Some(&token),
            &[("x-thought-khoral-lease-token", lease)],
            None
        )
        .await
        .0,
        403
    );
}

#[tokio::test]
async fn expired_requester_authority_and_narrowed_policy_deny_terminal_submission() {
    for narrow in [false, true] {
        let server = server().await;
        let room = Uuid::new_v4();
        let token = server.token(Uuid::new_v4(), "human");
        assert_eq!(
            post(
                &server,
                "/api/agent-conversations/v1/turns",
                &token,
                &[],
                &turn(room)
            )
            .await
            .0,
            202
        );
        let claimed = claim(&server).await;
        let packet = &claimed["packet"];
        let task_id = Uuid::parse_str(packet["taskId"].as_str().unwrap()).unwrap();
        if narrow {
            let mut changed = policy();
            changed.policy_revision = "narrowed-2".into();
            server
                .state
                .configure_conversations(changed, Some(Arc::new(TestCatalog)))
                .await
                .unwrap();
        } else {
            sqlx::query("UPDATE conversation_tasks SET issued_at=CURRENT_TIMESTAMP-interval '2 seconds',authorization_expires_at=CURRENT_TIMESTAMP-interval '1 second',expires_at=CURRENT_TIMESTAMP-interval '1 second' WHERE task_id=$1").bind(task_id).execute(&server.pool).await.unwrap();
        }
        let result = post(
            &server,
            &format!("/internal/agent-conversations/v1/tasks/{task_id}/updates"),
            &workload(&server),
            &[(
                "x-thought-khoral-lease-token",
                claimed["leaseToken"].as_str().unwrap(),
            )],
            &completion(packet),
        )
        .await;
        assert_eq!(result.0, if narrow { 403 } else { 401 }, "{}", result.1);
        assert_eq!(events_after(&server.pool, room, 0).await.unwrap().len(), 1);
        let row=sqlx::query("SELECT t.state,c.state AS conversation_state FROM conversation_tasks t JOIN agent_conversations c USING(conversation_id) WHERE t.task_id=$1").bind(task_id).fetch_one(&server.pool).await.unwrap();
        assert_eq!(row.get::<&str, _>("state"), "failed");
        assert_eq!(row.get::<&str, _>("conversation_state"), "unusable");
    }
}

#[tokio::test]
async fn receipt_recovers_an_accepted_acknowledgement_after_lease_expiry() {
    let server = server().await;
    let room = Uuid::new_v4();
    let token = server.token(Uuid::new_v4(), "human");
    post(
        &server,
        "/api/agent-conversations/v1/turns",
        &token,
        &[],
        &turn(room),
    )
    .await;
    let claimed = claim(&server).await;
    let packet = &claimed["packet"];
    let task_id = Uuid::parse_str(packet["taskId"].as_str().unwrap()).unwrap();
    let lease = claimed["leaseToken"].as_str().unwrap();
    let ack = post(
        &server,
        &format!("/internal/agent-conversations/v1/tasks/{task_id}/updates"),
        &workload(&server),
        &[("x-thought-khoral-lease-token", lease)],
        &completion(packet),
    )
    .await;
    assert_eq!(ack.0, 200);
    sqlx::query("UPDATE conversation_tasks SET lease_expires_at=CURRENT_TIMESTAMP-interval '1 second' WHERE task_id=$1").bind(task_id).execute(&server.pool).await.unwrap();
    let receipt = http(
        &server,
        "GET",
        &format!("/internal/agent-conversations/v1/tasks/{task_id}/receipt"),
        Some(&workload(&server)),
        &[],
        None,
    )
    .await;
    assert_eq!(receipt.0, 200);
    assert_eq!(receipt.1["acknowledgement"], ack.1);
    assert!(!receipt.1.to_string().contains("leaseToken"));
    assert_eq!(
        http(
            &server,
            "GET",
            &format!("/internal/agent-conversations/v1/tasks/{task_id}/authority"),
            Some(&workload(&server)),
            &[("x-thought-khoral-lease-token", lease)],
            None
        )
        .await
        .0,
        409
    );
    assert_eq!(
        post(
            &server,
            "/internal/agent-conversations/v1/claim",
            &workload(&server),
            &[],
            &json!({"agentId":CODEX_AGENT_ID,"leaseOwner":"another-mediator"})
        )
        .await
        .0,
        204
    );
    assert_eq!(events_after(&server.pool, room, 0).await.unwrap().len(), 2);
}

#[tokio::test]
async fn coalescing_preserves_request_replay_and_accepts_terminal_ordinal_gaps() {
    let server = server().await;
    let room = Uuid::new_v4();
    let token = server.token(Uuid::new_v4(), "human");
    post(
        &server,
        "/api/agent-conversations/v1/turns",
        &token,
        &[],
        &turn(room),
    )
    .await;
    let claimed = claim(&server).await;
    let packet = &claimed["packet"];
    let path = format!(
        "/internal/agent-conversations/v1/tasks/{}/updates",
        packet["taskId"].as_str().unwrap()
    );
    let headers = [(
        "x-thought-khoral-lease-token",
        claimed["leaseToken"].as_str().unwrap(),
    )];
    let mut progress = completion(packet);
    progress["kind"] = json!("progress");
    progress["data"] = json!({"phase":"accepted","text":"Accepted"});
    assert_eq!(
        post(&server, &path, &workload(&server), &headers, &progress)
            .await
            .0,
        200
    );
    progress["updateId"] = json!(Uuid::new_v4());
    progress["ordinal"] = json!(2);
    progress["data"] = json!({"phase":"working","text":"Working"});
    let second = post(&server, &path, &workload(&server), &headers, &progress).await;
    assert_eq!(second.0, 200);
    assert_eq!(
        post(&server, &path, &workload(&server), &headers, &progress)
            .await
            .1,
        second.1
    );
    let mut changed = progress.clone();
    changed["data"]["text"] = json!("Changed");
    assert_eq!(
        post(&server, &path, &workload(&server), &headers, &changed)
            .await
            .0,
        409
    );
    let mut terminal = completion(packet);
    terminal["ordinal"] = json!(257);
    assert_eq!(
        post(&server, &path, &workload(&server), &headers, &terminal)
            .await
            .0,
        200
    );
    let view = http(
        &server,
        "GET",
        &format!(
            "/api/agent-conversations/v1/rooms/{room}/tasks/{}",
            packet["taskId"].as_str().unwrap()
        ),
        Some(&token),
        &[],
        None,
    )
    .await
    .1;
    let updates = view["updates"].as_array().unwrap();
    assert_eq!(updates.last().unwrap()["ordinal"], 257);
    assert_eq!(
        updates
            .iter()
            .filter(|entry| entry["kind"] == "completed")
            .count(),
        1
    );
    assert!(
        updates
            .iter()
            .any(|entry| entry["data"]["phase"] == "accepted")
    );
}

#[tokio::test]
async fn terminal_replays_need_a_live_lease_and_failures_are_idempotent() {
    for failed in [false, true] {
        let server = server().await;
        let room = Uuid::new_v4();
        let token = server.token(Uuid::new_v4(), "human");
        assert_eq!(
            post(
                &server,
                "/api/agent-conversations/v1/turns",
                &token,
                &[],
                &turn(room)
            )
            .await
            .0,
            202
        );
        let claimed = claim(&server).await;
        let packet = &claimed["packet"];
        let task = Uuid::parse_str(packet["taskId"].as_str().unwrap()).unwrap();
        let path = format!("/internal/agent-conversations/v1/tasks/{task}/updates");
        let headers = [(
            "x-thought-khoral-lease-token",
            claimed["leaseToken"].as_str().unwrap(),
        )];
        let mut terminal = completion(packet);
        if failed {
            terminal["kind"] = json!("failed");
            terminal["data"] = json!({"code":"execution_failed"});
        }
        let accepted = post(&server, &path, &workload(&server), &headers, &terminal).await;
        assert_eq!(accepted.0, 200);
        let replay = post(&server, &path, &workload(&server), &headers, &terminal).await;
        assert_eq!(replay.0, 200, "{}", replay.1);
        assert_eq!(replay.1, accepted.1);
        sqlx::query("UPDATE conversation_tasks SET lease_expires_at=CURRENT_TIMESTAMP-interval '1 second' WHERE task_id=$1").bind(task).execute(&server.pool).await.unwrap();
        assert_eq!(
            post(&server, &path, &workload(&server), &headers, &terminal)
                .await
                .0,
            409
        );
        assert_eq!(
            events_after(&server.pool, room, 0).await.unwrap().len(),
            if failed { 1 } else { 2 }
        );
    }
}

#[tokio::test]
async fn an_expired_running_lease_is_failed_without_reclaiming_the_turn() {
    let server = server().await;
    let room = Uuid::new_v4();
    let token = server.token(Uuid::new_v4(), "human");
    post(
        &server,
        "/api/agent-conversations/v1/turns",
        &token,
        &[],
        &turn(room),
    )
    .await;
    let claimed = claim(&server).await;
    let task = Uuid::parse_str(claimed["packet"]["taskId"].as_str().unwrap()).unwrap();
    sqlx::query("UPDATE conversation_tasks SET lease_expires_at=CURRENT_TIMESTAMP-interval '1 second' WHERE task_id=$1").bind(task).execute(&server.pool).await.unwrap();
    assert_eq!(
        post(
            &server,
            "/internal/agent-conversations/v1/claim",
            &workload(&server),
            &[],
            &json!({"agentId":CODEX_AGENT_ID,"leaseOwner":"replacement"})
        )
        .await
        .0,
        204
    );
    let view = http(
        &server,
        "GET",
        &format!("/api/agent-conversations/v1/rooms/{room}/tasks/{task}"),
        Some(&token),
        &[],
        None,
    )
    .await;
    assert_eq!(view.1["state"], "failed");
    assert_eq!(view.1["failure"]["code"], "conversation_interrupted");
    assert_eq!(
        view.1["updates"].as_array().unwrap().last().unwrap()["kind"],
        "failed"
    );
}

#[tokio::test]
async fn retained_chat_with_the_pinned_codex_mention_remains_ordinary_chat() {
    let server = server().await;
    let room = Uuid::new_v4();
    let actor = Uuid::new_v4();
    let token = server.token(actor, "human");
    let mut socket = server.connect(&token).await;
    let joined = support::join(&mut socket, room, None).await;
    assert!(
        joined["result"]["participants"]
            .as_array()
            .unwrap()
            .iter()
            .any(|participant| participant["id"] == json!(CODEX_AGENT_ID))
    );
    let mut params = support::common_params(Uuid::new_v4(), room);
    params["text"] = json!("@codex-agent ordinary chat");
    params["mentions"] = turn(room)["mentions"].clone();
    support::send_json(&mut socket, support::rpc("ordinary", "chat.send", params)).await;
    assert_eq!(
        support::recv_json(&mut socket).await["eventType"],
        "message.created"
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM conversation_tasks WHERE room_id=$1")
        .bind(room)
        .fetch_one(&server.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    let mut invalid = turn(room);
    invalid["mentions"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type":"participant","id":Uuid::new_v4(),"token":"unknown"}));
    assert_eq!(
        post(
            &server,
            "/api/agent-conversations/v1/turns",
            &token,
            &[],
            &invalid
        )
        .await
        .0,
        400
    );
    assert_eq!(events_after(&server.pool, room, 0).await.unwrap().len(), 1);
}

#[tokio::test]
async fn semantically_invalid_runtime_metadata_is_rejected_before_storage() {
    let server = server().await;
    let room = Uuid::new_v4();
    let token = server.token(Uuid::new_v4(), "human");
    post(
        &server,
        "/api/agent-conversations/v1/turns",
        &token,
        &[],
        &turn(room),
    )
    .await;
    let claimed = claim(&server).await;
    let packet = &claimed["packet"];
    let path = format!(
        "/internal/agent-conversations/v1/tasks/{}/updates",
        packet["taskId"].as_str().unwrap()
    );
    let headers = [(
        "x-thought-khoral-lease-token",
        claimed["leaseToken"].as_str().unwrap(),
    )];
    let invalid_settings = json!({"model":null,"reasoningEffort":"medium","confirmation":"confirmed","reroutedModel":null});
    let invalid_usage = json!({"lastTotalTokens":12,"modelContextWindow":null,"reportedAt":Utc::now(),"model":"model-a","freshness":"fresh"});
    for (kind, data) in [
        ("settings", invalid_settings.clone()),
        ("usage", invalid_usage.clone()),
    ] {
        let mut update = completion(packet);
        update["kind"] = json!(kind);
        update["data"] = data;
        assert_eq!(
            post(&server, &path, &workload(&server), &headers, &update)
                .await
                .0,
            400
        );
    }
    for (key, data) in [
        ("effectiveSettings", invalid_settings),
        ("usage", invalid_usage),
    ] {
        let mut update = completion(packet);
        update["data"][key] = data;
        assert_eq!(
            post(&server, &path, &workload(&server), &headers, &update)
                .await
                .0,
            400
        );
    }
    assert_eq!(events_after(&server.pool, room, 0).await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_nonterminal_update_must_leave_a_representable_terminal_ordinal() {
    let server = server().await;
    let room = Uuid::new_v4();
    let token = server.token(Uuid::new_v4(), "human");
    post(
        &server,
        "/api/agent-conversations/v1/turns",
        &token,
        &[],
        &turn(room),
    )
    .await;
    let claimed = claim(&server).await;
    let packet = &claimed["packet"];
    let path = format!(
        "/internal/agent-conversations/v1/tasks/{}/updates",
        packet["taskId"].as_str().unwrap()
    );
    let headers = [(
        "x-thought-khoral-lease-token",
        claimed["leaseToken"].as_str().unwrap(),
    )];
    let mut progress = completion(packet);
    progress["ordinal"] = json!(9_007_199_254_740_991_u64);
    progress["kind"] = json!("progress");
    progress["data"] = json!({"phase":"working","text":"Working"});
    assert_eq!(
        post(&server, &path, &workload(&server), &headers, &progress)
            .await
            .0,
        400
    );
    let mut terminal = completion(packet);
    terminal["ordinal"] = progress["ordinal"].clone();
    assert_eq!(
        post(&server, &path, &workload(&server), &headers, &terminal)
            .await
            .0,
        200
    );
}

struct PagedCatalog;
impl CatalogQuery for PagedCatalog {
    fn page<'a>(
        &'a self,
        _agent: Uuid,
        cursor: Option<String>,
        _limit: usize,
    ) -> Pin<Box<dyn Future<Output = Result<Value, ConversationError>> + Send + 'a>> {
        Box::pin(async move {
            let first = cursor.is_none();
            if !first && cursor.as_deref() != Some("mediator-page-two") {
                return Err(ConversationError::InvalidTaskInput);
            }
            Ok(
                json!({"profileVersion":PROFILE_VERSION,"catalogRevision":"catalog-1","data":[{"id":if first {"model-a"} else {"blocked-model"},"displayName":"Catalog model","defaultReasoningEffort":"medium","supportedReasoningEfforts":[{"id":"medium","description":"Supported"}]}],"nextCursor":if first {Some("mediator-page-two")} else {None}}),
            )
        })
    }
}

#[tokio::test]
async fn catalog_pagination_is_opaque_filtered_and_revision_bound() {
    let server = server().await;
    server
        .state
        .configure_conversations(policy(), Some(Arc::new(PagedCatalog)))
        .await
        .unwrap();
    let room = Uuid::new_v4();
    let token = server.token(Uuid::new_v4(), "human");
    let path = format!("/api/agent-conversations/v1/rooms/{room}/agents/{CODEX_AGENT_ID}/models");
    let first = http(
        &server,
        "GET",
        &format!("{path}?limit=1"),
        Some(&token),
        &[],
        None,
    )
    .await;
    assert_eq!(first.0, 200, "{}", first.1);
    let cursor = first.1["nextCursor"].as_str().unwrap();
    assert_ne!(cursor, "mediator-page-two");
    let second = http(
        &server,
        "GET",
        &format!("{path}?cursor={cursor}&limit=1"),
        Some(&token),
        &[],
        None,
    )
    .await;
    assert_eq!(second.0, 200);
    assert_eq!(second.1["data"], json!([]));
    assert_eq!(second.1["nextCursor"], Value::Null);
    assert_eq!(
        http(
            &server,
            "GET",
            &format!("{path}?limit=1&limit=2"),
            Some(&token),
            &[],
            None
        )
        .await
        .0,
        400
    );
    let mut changed = policy();
    changed.catalog_revision = "catalog-2".into();
    server
        .state
        .configure_conversations(changed, Some(Arc::new(PagedCatalog)))
        .await
        .unwrap();
    assert_eq!(
        http(
            &server,
            "GET",
            &format!("{path}?cursor={cursor}"),
            Some(&token),
            &[],
            None
        )
        .await
        .0,
        409
    );
}

#[tokio::test]
async fn accepted_turn_replay_survives_an_unavailable_catalog_but_new_work_fails_closed() {
    let server = server().await;
    let room = Uuid::new_v4();
    let token = server.token(Uuid::new_v4(), "human");
    let input = turn(room);
    let accepted = post(
        &server,
        "/api/agent-conversations/v1/turns",
        &token,
        &[],
        &input,
    )
    .await;
    assert_eq!(accepted.0, 202);
    server
        .state
        .configure_conversations(policy(), None)
        .await
        .unwrap();
    let replay = post(
        &server,
        "/api/agent-conversations/v1/turns",
        &token,
        &[],
        &input,
    )
    .await;
    assert_eq!(replay.0, 202);
    assert_eq!(replay.1, accepted.1);
    let other = Uuid::new_v4();
    assert_eq!(
        post(
            &server,
            "/api/agent-conversations/v1/turns",
            &token,
            &[],
            &turn(other)
        )
        .await
        .0,
        503
    );
    assert!(
        events_after(&server.pool, other, 0)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn completed_update_replay_rejects_a_superseded_generation() {
    let server = server().await;
    let room = Uuid::new_v4();
    let token = server.token(Uuid::new_v4(), "human");
    post(
        &server,
        "/api/agent-conversations/v1/turns",
        &token,
        &[],
        &turn(room),
    )
    .await;
    let claimed = claim(&server).await;
    let packet = &claimed["packet"];
    let terminal = completion(packet);
    let path = format!(
        "/internal/agent-conversations/v1/tasks/{}/updates",
        packet["taskId"].as_str().unwrap()
    );
    let headers = [(
        "x-thought-khoral-lease-token",
        claimed["leaseToken"].as_str().unwrap(),
    )];
    assert_eq!(
        post(&server, &path, &workload(&server), &headers, &terminal)
            .await
            .0,
        200
    );
    assert_eq!(
        post(
            &server,
            "/api/agent-conversations/v1/turns",
            &token,
            &[],
            &turn(room)
        )
        .await
        .0,
        202
    );
    assert_eq!(
        post(&server, &path, &workload(&server), &headers, &terminal)
            .await
            .0,
        409
    );
    let receipt = http(
        &server,
        "GET",
        &path.replace("/updates", "/receipt"),
        Some(&workload(&server)),
        &[],
        None,
    )
    .await;
    assert_eq!(receipt.0, 200);
    assert!(!receipt.1["acknowledgement"].is_null());
}

#[tokio::test]
async fn first_acceptance_is_preserved_even_after_a_working_progress_update() {
    let server = server().await;
    let room = Uuid::new_v4();
    let token = server.token(Uuid::new_v4(), "human");
    post(
        &server,
        "/api/agent-conversations/v1/turns",
        &token,
        &[],
        &turn(room),
    )
    .await;
    let claimed = claim(&server).await;
    let packet = &claimed["packet"];
    let task = Uuid::parse_str(packet["taskId"].as_str().unwrap()).unwrap();
    let path = format!("/internal/agent-conversations/v1/tasks/{task}/updates");
    let headers = [(
        "x-thought-khoral-lease-token",
        claimed["leaseToken"].as_str().unwrap(),
    )];
    let mut progress = completion(packet);
    progress["kind"] = json!("progress");
    progress["data"] = json!({"phase":"working","text":"Working"});
    assert_eq!(
        post(&server, &path, &workload(&server), &headers, &progress)
            .await
            .0,
        200
    );
    progress["ordinal"] = json!(2);
    progress["updateId"] = json!(Uuid::new_v4());
    progress["data"] = json!({"phase":"accepted","text":"Accepted"});
    assert_eq!(
        post(&server, &path, &workload(&server), &headers, &progress)
            .await
            .0,
        200
    );
    let first_accepted = progress["updateId"].clone();
    sqlx::query("UPDATE conversation_tasks SET last_progress_at=CURRENT_TIMESTAMP-interval '2 seconds' WHERE task_id=$1").bind(task).execute(&server.pool).await.unwrap();
    progress["ordinal"] = json!(3);
    progress["updateId"] = json!(Uuid::new_v4());
    assert_eq!(
        post(&server, &path, &workload(&server), &headers, &progress)
            .await
            .0,
        200
    );
    let mut terminal = completion(packet);
    terminal["ordinal"] = json!(4);
    assert_eq!(
        post(&server, &path, &workload(&server), &headers, &terminal)
            .await
            .0,
        200
    );
    let view = http(
        &server,
        "GET",
        &format!("/api/agent-conversations/v1/rooms/{room}/tasks/{task}"),
        Some(&token),
        &[],
        None,
    )
    .await;
    let accepted = view.1["updates"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["data"]["phase"] == "accepted")
        .collect::<Vec<_>>();
    assert_eq!(accepted.len(), 1);
    assert_eq!(accepted[0]["updateId"], first_accepted);
}

#[tokio::test]
async fn accepted_replay_keeps_original_mentions_after_roster_tokens_change() {
    let server = server().await;
    let room = Uuid::new_v4();
    let token = server.token(Uuid::new_v4(), "human");
    let maya = Uuid::new_v4();
    seeded(&server, room, "Maya proposes green", maya, false).await;
    let mut input = turn(room);
    input["mentions"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type":"participant","id":maya,"token":"maya"}));
    let accepted = post(
        &server,
        "/api/agent-conversations/v1/turns",
        &token,
        &[],
        &input,
    )
    .await;
    assert_eq!(accepted.0, 202);
    seeded(
        &server,
        room,
        "Maya shares another idea",
        Uuid::new_v4(),
        false,
    )
    .await;
    let replay = post(
        &server,
        "/api/agent-conversations/v1/turns",
        &token,
        &[],
        &input,
    )
    .await;
    assert_eq!(replay.0, 202, "{}", replay.1);
    assert_eq!(replay.1, accepted.1);
    assert_eq!(events_after(&server.pool, room, 0).await.unwrap().len(), 3);
}

#[tokio::test]
async fn additional_mentions_fail_closed_at_the_bounded_identity_projection() {
    let server = server().await;
    let room = Uuid::new_v4();
    let token = server.token(Uuid::new_v4(), "human");
    let maya = Uuid::new_v4();
    seeded(&server, room, "Maya private synthetic", maya, true).await;
    sqlx::query("INSERT INTO room_events(event_id,room_id,sequence,request_id,event_type,actor_id,actor_role,actor_display_name,payload,occurred_at) SELECT gen_random_uuid(),$1,n+1,gen_random_uuid(),'message.created',id,'human','Synthetic participant',jsonb_build_object('text','Synthetic private history','delivery','mentioned','mentions',jsonb_build_array(jsonb_build_object('type','participant','id',$2::uuid,'token','codex-agent')),'audienceIds',jsonb_build_array(id,$2::uuid)),CURRENT_TIMESTAMP FROM (SELECT n,gen_random_uuid() AS id FROM generate_series(1,2000) n) actors").bind(room).bind(CODEX_AGENT_ID).execute(&server.pool).await.unwrap();
    let mut input = turn(room);
    input["mentions"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type":"participant","id":maya,"token":"maya"}));
    assert_eq!(
        post(
            &server,
            "/api/agent-conversations/v1/turns",
            &token,
            &[],
            &input
        )
        .await
        .0,
        413
    );
    let accepted = post(
        &server,
        "/api/agent-conversations/v1/turns",
        &token,
        &[],
        &turn(room),
    )
    .await;
    assert_eq!(accepted.0, 202, "{}", accepted.1);
    let claimed = claim(&server).await;
    let baseline = claimed["packet"]["context"]["entries"].as_array().unwrap();
    assert_eq!(baseline.len(), 1);
    assert_eq!(baseline[0]["sequence"], 2002);
}
