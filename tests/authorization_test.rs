mod support;

use jsonwebtoken::Algorithm;
use serde_json::json;
use sqlx::Row;
use tokio_tungstenite::{connect_async, tungstenite::Error};
use uuid::Uuid;

use support::{
    BROWSER_ORIGIN, TestServer, TokenOptions, common_params, join, recv_close_code, recv_json, rpc,
    send_json,
};

async fn wait_until_epoch_second(target: i64) {
    while chrono::Utc::now().timestamp() < target {
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
}

async fn assert_unauthenticated(server: &TestServer, token: &str) {
    let error = server
        .connect_result(token)
        .await
        .expect_err("the invalid token must not establish a WebSocket");
    let Error::Http(response) = error else {
        panic!("expected an HTTP rejection, got {error:?}");
    };
    assert_eq!(response.status(), 401);
    let value: serde_json::Value =
        serde_json::from_slice(response.body().as_ref().unwrap()).unwrap();
    assert_eq!(value["error"]["code"], -32001);
}

// This fails if the public health response is removed from the running gateway.
#[tokio::test]
async fn health_route_reports_the_thought_khoral_service_status() {
    let server = TestServer::start().await;

    let (status, body) = server.get("/health").await;

    assert_eq!(status, 200);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&body).unwrap(),
        json!({
            "product": "ThoughtKhoral",
            "service": "thought-khoral-room-gateway",
            "status": "ok",
        })
    );
}

// This fails if an unauthenticated upgrade can establish a room WebSocket.
#[tokio::test]
async fn unauthenticated_upgrade_returns_structured_error() {
    let server = TestServer::start().await;

    let error = connect_async(server.ws_url())
        .await
        .expect_err("an unauthenticated WebSocket upgrade must fail");
    let Error::Http(response) = error else {
        panic!("expected an HTTP rejection, got {error:?}");
    };

    assert_eq!(response.status(), 401);
    let body = response
        .body()
        .as_ref()
        .expect("the rejection must have a body");
    let value: serde_json::Value = serde_json::from_slice(body).unwrap();
    assert_eq!(value["error"]["code"], -32001);
}

// This fails if an unsupported n2n_role is defaulted or admitted as a participant.
#[tokio::test]
async fn unsupported_token_role_is_unauthenticated() {
    let server = TestServer::start().await;
    let token = server.token(Uuid::new_v4(), "service");

    let error = server
        .connect_result(&token)
        .await
        .expect_err("an unsupported role must not establish a WebSocket");
    let Error::Http(response) = error else {
        panic!("expected an HTTP rejection, got {error:?}");
    };
    assert_eq!(response.status(), 401);
    let value: serde_json::Value =
        serde_json::from_slice(response.body().as_ref().unwrap()).unwrap();
    assert_eq!(value["error"]["code"], -32001);
}

// This fails if an allowed browser cannot bind its token identity before joining a room.
#[tokio::test]
async fn browser_authenticates_then_joins() {
    let server = TestServer::start().await;
    let actor_id = Uuid::new_v4();
    let token = server.token(actor_id, "human");
    let mut socket = server.connect_browser(BROWSER_ORIGIN).await;

    send_json(
        &mut socket,
        rpc(
            "authenticate",
            "session.authenticate",
            json!({ "accessToken": token }),
        ),
    )
    .await;
    let authenticated = recv_json(&mut socket).await;
    assert_eq!(authenticated["id"], "authenticate");
    assert_eq!(authenticated["result"]["actor"]["id"], actor_id.to_string());
    assert_eq!(authenticated["result"]["actor"]["role"], "human");
    assert!(authenticated["result"]["expiresAt"].is_i64());
    assert!(!authenticated.to_string().contains(&token));

    let joined = join(&mut socket, Uuid::new_v4(), None).await;
    assert_eq!(joined["result"]["events"], json!([]));
}

// This fails if an Origin-bearing client can use an upgrade header to bypass first-message authentication.
#[tokio::test]
async fn browser_authorization_header_does_not_bypass_session_authentication() {
    let server = TestServer::start().await;
    let token = server.token(Uuid::new_v4(), "human");
    let mut socket = server
        .connect_browser_result(BROWSER_ORIGIN, Some(&token))
        .await
        .expect("an allowed browser origin may upgrade")
        .0;

    let mut params = common_params(Uuid::new_v4(), Uuid::new_v4());
    params["afterSequence"] = json!(0);
    send_json(&mut socket, rpc("join", "room.join", params)).await;
    assert_eq!(recv_json(&mut socket).await["error"]["code"], -32001);
    assert_eq!(recv_close_code(&mut socket).await, 1008);
}

// This fails if the handshake accepts an Origin outside the explicit allowlist.
#[tokio::test]
async fn browser_origin_must_be_allowlisted() {
    let server = TestServer::start().await;
    let error = server
        .connect_browser_result("http://attacker.test", None)
        .await
        .expect_err("a disallowed browser Origin must not upgrade");
    let Error::Http(response) = error else {
        panic!("expected an HTTP rejection, got {error:?}");
    };
    assert_eq!(response.status(), 403);
}

// This fails if a session.authenticate token is trusted without full OIDC validation.
#[tokio::test]
async fn browser_invalid_token_is_rejected_and_closed() {
    let server = TestServer::start().await;
    let mut socket = server.connect_browser(BROWSER_ORIGIN).await;
    send_json(
        &mut socket,
        rpc(
            "authenticate",
            "session.authenticate",
            json!({ "accessToken": "not-a-jwt" }),
        ),
    )
    .await;

    assert_eq!(recv_json(&mut socket).await["error"]["code"], -32001);
    assert_eq!(recv_close_code(&mut socket).await, 1008);
}

// This fails if an unauthenticated browser socket can remain open past the bounded deadline.
#[tokio::test]
async fn browser_authentication_timeout_closes_the_socket() {
    let server =
        TestServer::start_with_authentication_timeout(std::time::Duration::from_millis(30)).await;
    let mut socket = server.connect_browser(BROWSER_ORIGIN).await;

    assert_eq!(recv_json(&mut socket).await["error"]["code"], -32001);
    assert_eq!(recv_close_code(&mut socket).await, 1008);
}

// This fails if a browser session remains active after its validated JWT lifetime.
#[tokio::test]
async fn browser_session_closes_at_token_expiration() {
    let server = TestServer::start().await;
    let mut options = TokenOptions::valid(Uuid::new_v4(), "human");
    options.exp = chrono::Utc::now().timestamp() + 2;
    let token = server.token_with(options);
    let mut socket = server.connect_browser(BROWSER_ORIGIN).await;
    send_json(
        &mut socket,
        rpc(
            "authenticate",
            "session.authenticate",
            json!({ "accessToken": token }),
        ),
    )
    .await;
    assert_eq!(recv_json(&mut socket).await["id"], "authenticate");

    assert_eq!(recv_json(&mut socket).await["error"]["code"], -32001);
    assert_eq!(recv_close_code(&mut socket).await, 1008);
}

// This fails if JWT expiration receives any implicit clock-skew grace period.
#[tokio::test]
async fn expired_token_is_unauthenticated_without_leeway() {
    let server = TestServer::start().await;
    let mut options = TokenOptions::valid(Uuid::new_v4(), "human");
    options.exp = chrono::Utc::now().timestamp() - 1;

    assert_unauthenticated(&server, &server.token_with(options)).await;
}

// This fails if the validator treats the expiration instant itself as unexpired.
#[tokio::test]
async fn token_expiring_at_now_is_unauthenticated() {
    let server = TestServer::start().await;
    let boundary = chrono::Utc::now().timestamp() + 1;
    let mut options = TokenOptions::valid(Uuid::new_v4(), "human");
    options.exp = boundary;
    let token = server.token_with(options);

    wait_until_epoch_second(boundary).await;
    assert_unauthenticated(&server, &token).await;
}

// This fails if a token is admitted before its nbf instant.
#[tokio::test]
async fn future_not_before_token_is_unauthenticated_without_leeway() {
    let server = TestServer::start().await;
    let mut options = TokenOptions::valid(Uuid::new_v4(), "human");
    options.nbf = Some(chrono::Utc::now().timestamp() + 30);

    assert_unauthenticated(&server, &server.token_with(options)).await;
}

// This protects the inclusive valid side of the not-before boundary.
#[tokio::test]
async fn token_not_before_now_is_authenticated() {
    let server = TestServer::start().await;
    let boundary = chrono::Utc::now().timestamp() + 1;
    let mut options = TokenOptions::valid(Uuid::new_v4(), "human");
    options.nbf = Some(boundary);
    let token = server.token_with(options);

    wait_until_epoch_second(boundary).await;
    let _socket = server.connect(&token).await;
}

// This fails if any required JWT trust dimension is skipped or defaulted.
#[tokio::test]
async fn issuer_audience_signature_kid_algorithm_and_subject_are_enforced() {
    let server = TestServer::start().await;

    let mut wrong_issuer = TokenOptions::valid(Uuid::new_v4(), "human");
    wrong_issuer.issuer = "http://attacker.test/realms/thought-khoral".to_owned();
    let mut wrong_audience = TokenOptions::valid(Uuid::new_v4(), "human");
    wrong_audience.audience = "different-service".to_owned();
    let mut wrong_signature = TokenOptions::valid(Uuid::new_v4(), "human");
    wrong_signature.wrong_signature = true;
    let mut missing_kid = TokenOptions::valid(Uuid::new_v4(), "human");
    missing_kid.kid = None;
    let mut wrong_kid = TokenOptions::valid(Uuid::new_v4(), "human");
    wrong_kid.kid = Some("unknown-key".to_owned());
    let mut wrong_algorithm = TokenOptions::valid(Uuid::new_v4(), "human");
    wrong_algorithm.algorithm = Algorithm::HS256;
    let mut malformed_subject = TokenOptions::valid(Uuid::new_v4(), "human");
    malformed_subject.sub = "not-a-uuid".to_owned();

    for token in [
        server.token_with(wrong_issuer),
        server.token_with(wrong_audience),
        server.token_with(wrong_signature),
        server.token_with(missing_kid),
        server.token_with(wrong_kid),
        server.token_with(wrong_algorithm),
        server.token_with(malformed_subject),
        "not-a-jwt".to_owned(),
    ] {
        assert_unauthenticated(&server, &token).await;
    }
}

// This fails if an agent can cross the human-only decision governance boundary.
#[tokio::test]
async fn agent_cannot_transition_a_decision() {
    let server = TestServer::start().await;
    let room_id = Uuid::new_v4();
    let token = server.token(Uuid::new_v4(), "agent");
    let mut socket = server.connect(&token).await;
    let joined = join(&mut socket, room_id, None).await;
    assert_eq!(joined["result"]["events"], json!([]));

    let mut params = common_params(Uuid::new_v4(), room_id);
    params["decisionId"] = json!(Uuid::new_v4());
    params["action"] = json!("confirm");
    send_json(
        &mut socket,
        rpc("transition", "decision.transition", params),
    )
    .await;

    let error = recv_json(&mut socket).await;
    assert_eq!(error["error"]["code"], -32003);
}

// This fails if an agent can physically delete a decision across the human-only boundary.
#[tokio::test]
async fn agent_cannot_delete_a_decision() {
    let server = TestServer::start().await;
    let room_id = Uuid::new_v4();
    let mut human = server.connect(&server.token(Uuid::new_v4(), "human")).await;
    join(&mut human, room_id, None).await;
    let mut proposal = common_params(Uuid::new_v4(), room_id);
    proposal["title"] = json!("Keep this draft");
    proposal["summary"] = json!("It must survive an unauthorized delete.");
    proposal["sourceEventIds"] = json!([]);
    send_json(&mut human, rpc("proposal", "decision.propose", proposal)).await;
    let proposed = recv_json(&mut human).await;
    let decision_id = Uuid::parse_str(proposed["payload"]["decisionId"].as_str().unwrap()).unwrap();

    let mut agent = server.connect(&server.token(Uuid::new_v4(), "agent")).await;
    join(&mut agent, room_id, None).await;
    let mut delete = common_params(Uuid::new_v4(), room_id);
    delete["decisionId"] = json!(decision_id);
    send_json(&mut agent, rpc("delete", "decision.delete", delete)).await;
    assert_eq!(recv_json(&mut agent).await["error"]["code"], -32003);

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM decisions WHERE decision_id = $1")
        .bind(decision_id)
        .fetch_one(&server.pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

// This fails if a valid human cannot activate a draft decision.
#[tokio::test]
async fn human_can_confirm_a_draft_decision() {
    let server = TestServer::start().await;
    let room_id = Uuid::new_v4();
    let actor_id = Uuid::new_v4();
    let token = server.token(actor_id, "human");
    let mut socket = server.connect(&token).await;
    join(&mut socket, room_id, None).await;

    let mut proposal = common_params(Uuid::new_v4(), room_id);
    proposal["title"] = json!("Ship the governed room");
    proposal["summary"] = json!("The MVP acceptance boundary is satisfied.");
    proposal["sourceEventIds"] = json!([Uuid::new_v4()]);
    send_json(&mut socket, rpc("proposal", "decision.propose", proposal)).await;
    let proposed = recv_json(&mut socket).await;
    assert_eq!(proposed["eventType"], "decision.proposed");
    let decision_id = proposed["payload"]["decisionId"].as_str().unwrap();

    let mut transition = common_params(Uuid::new_v4(), room_id);
    transition["decisionId"] = json!(decision_id);
    transition["action"] = json!("confirm");
    send_json(
        &mut socket,
        rpc("transition", "decision.transition", transition),
    )
    .await;

    let confirmed = recv_json(&mut socket).await;
    assert_eq!(confirmed["eventType"], "decision.confirmed");
    assert_eq!(confirmed["payload"]["decisionId"], decision_id);
}

// This fails if edit does not supersede the draft, activate one replacement, and persist both events atomically.
#[tokio::test]
async fn human_edit_atomically_supersedes_and_replaces_a_draft() {
    let server = TestServer::start().await;
    let room_id = Uuid::new_v4();
    let token = server.token(Uuid::new_v4(), "human");
    let mut socket = server.connect(&token).await;
    join(&mut socket, room_id, None).await;

    let mut proposal = common_params(Uuid::new_v4(), room_id);
    proposal["title"] = json!("Original title");
    proposal["summary"] = json!("Original summary");
    proposal["sourceEventIds"] = json!([Uuid::new_v4()]);
    send_json(&mut socket, rpc("proposal", "decision.propose", proposal)).await;
    let proposed = recv_json(&mut socket).await;
    let old_id = Uuid::parse_str(proposed["payload"]["decisionId"].as_str().unwrap()).unwrap();

    let edit_request_id = Uuid::new_v4();
    let mut edit = common_params(edit_request_id, room_id);
    edit["decisionId"] = json!(old_id);
    edit["action"] = json!("edit");
    edit["editedTitle"] = json!("Approved title");
    edit["editedSummary"] = json!("Approved summary");
    send_json(&mut socket, rpc("edit", "decision.transition", edit)).await;
    let edited = recv_json(&mut socket).await;
    let activated = recv_json(&mut socket).await;

    assert_eq!(edited["eventType"], "decision.edited");
    assert_eq!(activated["eventType"], "decision.confirmed");
    assert_eq!(
        edited["sequence"].as_i64().unwrap() + 1,
        activated["sequence"]
    );
    let replacement_id =
        Uuid::parse_str(activated["payload"]["decisionId"].as_str().unwrap()).unwrap();
    let rows = sqlx::query(
        "SELECT decision_id, status, derived_from_decision_id FROM decisions WHERE decision_id = $1 OR decision_id = $2 ORDER BY decision_id",
    )
    .bind(old_id)
    .bind(replacement_id)
    .fetch_all(&server.pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 2);
    let old = rows
        .iter()
        .find(|row| row.try_get::<Uuid, _>("decision_id").unwrap() == old_id)
        .unwrap();
    let replacement = rows
        .iter()
        .find(|row| row.try_get::<Uuid, _>("decision_id").unwrap() == replacement_id)
        .unwrap();
    assert_eq!(old.try_get::<String, _>("status").unwrap(), "superseded");
    assert_eq!(
        replacement.try_get::<String, _>("status").unwrap(),
        "active"
    );
    assert_eq!(
        replacement
            .try_get::<Option<Uuid>, _>("derived_from_decision_id")
            .unwrap(),
        Some(old_id)
    );
    let event_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM room_events WHERE room_id = $1 AND request_id = $2",
    )
    .bind(room_id)
    .bind(edit_request_id)
    .fetch_one(&server.pool)
    .await
    .unwrap();
    assert_eq!(event_count, 2);
}
