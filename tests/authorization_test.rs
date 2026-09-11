mod support;

use serde_json::json;
use sqlx::Row;
use tokio_tungstenite::{connect_async, tungstenite::Error};
use uuid::Uuid;

use support::{TestServer, common_params, join, recv_json, rpc, send_json};

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
