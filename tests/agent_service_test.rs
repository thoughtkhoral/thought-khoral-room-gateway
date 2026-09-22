mod support;

use chrono::{Duration, Utc};
use serde_json::{Value, json};
use sqlx::PgPool;
use support::{AUDIENCE, TestServer};
use thought_khoral_room_gateway::{
    AgentSkillId, AgentTaskStart, AgentTaskStore, NewEvent, append_event,
};
use uuid::Uuid;

const REFERENCE_AGENT_ID: Uuid = Uuid::from_u128(0x74686f756768746b_686f72616c000003);
const AGENT_GATEWAY_CLIENT_ID: &str = "thought-khoral-agent-gateway";

fn task_start(
    room_id: Uuid,
    agent_id: Uuid,
    input: &str,
    occurred_at: chrono::DateTime<Utc>,
) -> AgentTaskStart {
    AgentTaskStart {
        id: "agent-task-start".to_owned(),
        contract_version: "n2n.room.v1".to_owned(),
        request_id: Uuid::new_v4(),
        room_id,
        occurred_at,
        agent_id,
        skill_id: AgentSkillId::SummarizeContext,
        input: input.to_owned(),
    }
}

async fn insert_message(
    pool: &PgPool,
    room_id: Uuid,
    requester_id: Uuid,
    audience_id: Uuid,
    text: &str,
    occurred_at: chrono::DateTime<Utc>,
) -> Uuid {
    append_event(
        pool,
        NewEvent {
            room_id,
            request_id: Uuid::new_v4(),
            event_type: "message.created".to_owned(),
            actor_id: requester_id,
            actor_role: "human".to_owned(),
            actor_display_name: Some("Requesting Human".to_owned()),
            payload: json!({
                "text": text,
                "delivery": "mentioned",
                "mentions": [{
                    "type": "participant",
                    "id": audience_id,
                    "token": "context-recipient",
                }],
                "audienceIds": [audience_id],
            }),
            occurred_at,
        },
    )
    .await
    .unwrap()
    .event_id
}

async fn insert_active_decision(pool: &PgPool, room_id: Uuid, occurred_at: chrono::DateTime<Utc>) {
    append_event(
        pool,
        NewEvent {
            room_id,
            request_id: Uuid::new_v4(),
            event_type: "decision.confirmed".to_owned(),
            actor_id: Uuid::new_v4(),
            actor_role: "human".to_owned(),
            actor_display_name: Some("Decision Maker".to_owned()),
            payload: json!({
                "decisionId": Uuid::new_v4(),
                "status": "active",
                "title": "Use the brokered packet",
                "summary": "The agent must receive a governed immutable context packet.",
                "sourceEventIds": [],
            }),
            occurred_at,
        },
    )
    .await
    .unwrap();
}

async fn insert_ineligible_agent_task(
    pool: &PgPool,
    requester_id: Uuid,
    occurred_at: chrono::DateTime<Utc>,
) -> Uuid {
    let task_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO agent_tasks (
            task_id, room_id, request_id, requester_id, agent_id, skill_id, input,
            context_revision, state, lease_owner, lease_expires_at, created_at, updated_at
        ) VALUES ($1, $2, $3, $4, $5, 'summarize-context', $6, 0, 'queued', NULL, NULL, $7, $7)
        "#,
    )
    .bind(task_id)
    .bind(Uuid::new_v4())
    .bind(Uuid::new_v4())
    .bind(requester_id)
    .bind(Uuid::new_v4())
    .bind("This task belongs to another agent.")
    .bind(occurred_at)
    .execute(pool)
    .await
    .unwrap();
    task_id
}

fn update_body(context_revision: i64, update_id: Uuid) -> Value {
    json!({
        "updateId": update_id,
        "contextRevision": context_revision,
        "update": {
            "eventType": "agent.task.progressed",
            "payload": {
                "phase": "working",
                "text": "Preparing the summary.",
                "percent": 50,
            },
            "occurredAt": Utc::now(),
        },
    })
}

// This fails if the internal API accepts a room identity, ignores the fixed-agent FIFO queue,
// reveals context without its live lease capability, or appends an update more than once.
#[tokio::test]
async fn brokered_context_requires_the_agent_gateway_identity_and_a_matching_live_lease() {
    let server = TestServer::start().await;
    let requester_id = Uuid::new_v4();
    let room_id = Uuid::new_v4();
    let now = Utc::now();
    let store = AgentTaskStore::new(server.pool.clone(), requester_id, "Requesting Human");

    sqlx::query("UPDATE agent_tasks SET state = 'failed', updated_at = NOW() WHERE agent_id = $1 AND state IN ('queued', 'running', 'awaiting_external_input')")
        .bind(REFERENCE_AGENT_ID)
        .execute(&server.pool)
        .await
        .unwrap();

    let _visible_event_id = insert_message(
        &server.pool,
        room_id,
        requester_id,
        requester_id,
        "Visible to the requester.",
        now - Duration::hours(2),
    )
    .await;
    let hidden_event_id = insert_message(
        &server.pool,
        room_id,
        requester_id,
        Uuid::new_v4(),
        "Not visible to the requester.",
        now - Duration::hours(2) + Duration::minutes(1),
    )
    .await;
    insert_active_decision(&server.pool, room_id, now - Duration::hours(2)).await;

    let wrong_agent_task =
        insert_ineligible_agent_task(&server.pool, requester_id, now - Duration::days(1)).await;
    let first_reference_task = store
        .start_agent_task(task_start(
            room_id,
            REFERENCE_AGENT_ID,
            "First Reference Agent task.",
            now,
        ))
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    let second_reference_task = store
        .start_agent_task(task_start(
            room_id,
            REFERENCE_AGENT_ID,
            "Second Reference Agent task.",
            now - Duration::days(365),
        ))
        .await
        .unwrap();

    let claim_body = json!({ "leaseOwner": Uuid::new_v4() });
    let (missing_status, _) = server
        .internal_json(
            "POST",
            "/internal/v1/agent-tasks/claim",
            None,
            None,
            Some(claim_body.clone()),
        )
        .await;
    assert_eq!(missing_status, 401);

    let (malformed_unauthenticated_status, _) = server
        .internal_json(
            "POST",
            "/internal/v1/agent-tasks/claim",
            None,
            None,
            Some(json!({ "unexpected": "body" })),
        )
        .await;
    assert_eq!(malformed_unauthenticated_status, 401);

    let (human_status, _) = server
        .internal_json(
            "POST",
            "/internal/v1/agent-tasks/claim",
            Some(&server.token(Uuid::new_v4(), "human")),
            None,
            Some(claim_body.clone()),
        )
        .await;
    assert_eq!(human_status, 403);

    let wrong_audience = server.agent_gateway_token("another-service", AGENT_GATEWAY_CLIENT_ID);
    let (wrong_audience_status, _) = server
        .internal_json(
            "POST",
            "/internal/v1/agent-tasks/claim",
            Some(&wrong_audience),
            None,
            Some(claim_body.clone()),
        )
        .await;
    assert_eq!(wrong_audience_status, 401);

    let multi_audience = server.agent_gateway_token_with_audiences(
        &[AUDIENCE, "another-service"],
        AGENT_GATEWAY_CLIENT_ID,
    );
    let (multi_audience_status, _) = server
        .internal_json(
            "POST",
            "/internal/v1/agent-tasks/claim",
            Some(&multi_audience),
            None,
            Some(claim_body.clone()),
        )
        .await;
    assert_eq!(multi_audience_status, 401);

    let wrong_authorized_party = server.agent_gateway_token(AUDIENCE, "another-client");
    let (wrong_authorized_party_status, _) = server
        .internal_json(
            "POST",
            "/internal/v1/agent-tasks/claim",
            Some(&wrong_authorized_party),
            None,
            Some(claim_body.clone()),
        )
        .await;
    assert_eq!(wrong_authorized_party_status, 403);

    let gateway_token = server.agent_gateway_token(AUDIENCE, AGENT_GATEWAY_CLIENT_ID);
    let (claim_status, claim) = server
        .internal_json(
            "POST",
            "/internal/v1/agent-tasks/claim",
            Some(&gateway_token),
            None,
            Some(claim_body.clone()),
        )
        .await;
    assert_eq!(claim_status, 200);
    assert_eq!(
        claim["packet"]["taskId"],
        json!(first_reference_task.task_id)
    );
    assert_eq!(claim["packet"]["agentId"], json!(REFERENCE_AGENT_ID));
    assert_eq!(claim["packet"]["input"], "First Reference Agent task.");
    assert_ne!(claim["packet"]["taskId"], json!(wrong_agent_task));
    assert_eq!(
        claim["packet"]["events"][0]["payload"]["text"],
        "Visible to the requester."
    );
    assert!(
        claim["packet"]["events"]
            .as_array()
            .unwrap()
            .iter()
            .all(|event| event["payload"]["text"] != "Not visible to the requester.")
    );
    assert_eq!(
        claim["packet"]["activeDecisions"][0]["title"],
        "Use the brokered packet"
    );
    assert_eq!(
        claim["packet"]["canonicalSha256"].as_str().unwrap().len(),
        64
    );

    let task_id = Uuid::parse_str(claim["packet"]["taskId"].as_str().unwrap()).unwrap();
    let lease_token = Uuid::parse_str(claim["leaseToken"].as_str().unwrap()).unwrap();
    let context_path = format!("/internal/v1/agent-tasks/{task_id}/context");
    let (wrong_lease_status, wrong_lease_body) = server
        .internal_json(
            "GET",
            &context_path,
            Some(&gateway_token),
            Some(Uuid::new_v4()),
            None,
        )
        .await;
    assert_eq!(wrong_lease_status, 404);
    assert_eq!(wrong_lease_body, Value::Null);

    let (context_status, context) = server
        .internal_json(
            "GET",
            &context_path,
            Some(&gateway_token),
            Some(lease_token),
            None,
        )
        .await;
    assert_eq!(context_status, 200);
    assert_eq!(
        context["packet"]["canonicalSha256"],
        claim["packet"]["canonicalSha256"]
    );

    let room_event_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM room_events WHERE room_id = $1")
            .bind(room_id)
            .fetch_one(&server.pool)
            .await
            .unwrap();
    let updates_path = format!("/internal/v1/agent-tasks/{task_id}/updates");
    let update_id = Uuid::new_v4();
    let update = update_body(
        claim["packet"]["contextRevision"].as_i64().unwrap(),
        update_id,
    );

    let (forged_status, _) = server
        .internal_json(
            "POST",
            &updates_path,
            Some(&gateway_token),
            Some(Uuid::new_v4()),
            Some(update.clone()),
        )
        .await;
    assert_eq!(forged_status, 404);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM room_events WHERE room_id = $1")
            .bind(room_id)
            .fetch_one(&server.pool)
            .await
            .unwrap(),
        room_event_count
    );

    let (stale_revision_status, _) = server
        .internal_json(
            "POST",
            &updates_path,
            Some(&gateway_token),
            Some(lease_token),
            Some(update_body(
                claim["packet"]["contextRevision"].as_i64().unwrap() + 1,
                Uuid::new_v4(),
            )),
        )
        .await;
    assert_eq!(stale_revision_status, 409);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM room_events WHERE room_id = $1")
            .bind(room_id)
            .fetch_one(&server.pool)
            .await
            .unwrap(),
        room_event_count
    );

    let (first_update_status, first_update) = server
        .internal_json(
            "POST",
            &updates_path,
            Some(&gateway_token),
            Some(lease_token),
            Some(update.clone()),
        )
        .await;
    assert_eq!(first_update_status, 200);
    let (duplicate_status, duplicate) = server
        .internal_json(
            "POST",
            &updates_path,
            Some(&gateway_token),
            Some(lease_token),
            Some(update.clone()),
        )
        .await;
    assert_eq!(duplicate_status, 200);
    assert_eq!(duplicate["events"], first_update["events"]);

    // The real polling route must recover both abandoned running tasks and
    // awaiting-input tasks, while invalidating the old lease capability.
    let mut current_lease = lease_token;
    for state in ["running", "awaiting_external_input"] {
        sqlx::query("UPDATE agent_tasks SET state = $1, lease_expires_at = NOW() - INTERVAL '1 second' WHERE task_id = $2")
            .bind(state).bind(task_id).execute(&server.pool).await.unwrap();
        // A restarted poll from the same worker must revoke its old capability.
        let owner = Uuid::parse_str(claim_body["leaseOwner"].as_str().unwrap()).unwrap();
        let (status, recovered) = server
            .internal_json(
                "POST",
                "/internal/v1/agent-tasks/claim",
                Some(&gateway_token),
                None,
                Some(json!({"leaseOwner": owner})),
            )
            .await;
        assert_eq!(status, 200);
        assert_eq!(recovered["packet"]["taskId"], json!(task_id));
        assert_ne!(
            recovered["packet"]["taskId"],
            json!(second_reference_task.task_id)
        );
        assert_eq!(
            server
                .internal_json(
                    "GET",
                    &context_path,
                    Some(&gateway_token),
                    Some(current_lease),
                    None
                )
                .await
                .0,
            404
        );
        current_lease = Uuid::parse_str(recovered["leaseToken"].as_str().unwrap()).unwrap();
        let (replay_status, replay) = server
            .internal_json(
                "POST",
                &updates_path,
                Some(&gateway_token),
                Some(current_lease),
                Some(update.clone()),
            )
            .await;
        assert_eq!(replay_status, 200);
        assert_eq!(replay["events"], first_update["events"]);
    }
    let lease_token = current_lease;

    let mut conflicting_duplicate = update.clone();
    conflicting_duplicate["update"]["payload"]["text"] = json!("Conflicting retry payload.");
    let (conflicting_duplicate_status, _) = server
        .internal_json(
            "POST",
            &updates_path,
            Some(&gateway_token),
            Some(lease_token),
            Some(conflicting_duplicate),
        )
        .await;
    assert_eq!(conflicting_duplicate_status, 409);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM room_events WHERE room_id = $1")
            .bind(room_id)
            .fetch_one(&server.pool)
            .await
            .unwrap(),
        room_event_count + 1
    );

    let forbidden_terminal_update = json!({
        "updateId": Uuid::new_v4(),
        "contextRevision": claim["packet"]["contextRevision"],
        "update": {
            "eventType": "agent.task.succeeded",
            "payload": {
                "result": {
                    "kind": "context-summary.v1",
                    "summary": "This must not cite a hidden event.",
                    "citations": [hidden_event_id],
                },
            },
            "occurredAt": Utc::now(),
        },
    });
    let (hidden_citation_status, _) = server
        .internal_json(
            "POST",
            &updates_path,
            Some(&gateway_token),
            Some(lease_token),
            Some(forbidden_terminal_update),
        )
        .await;
    assert_eq!(hidden_citation_status, 422);

    let foreign_terminal_update = json!({
        "updateId": Uuid::new_v4(),
        "contextRevision": claim["packet"]["contextRevision"],
        "update": {
            "eventType": "agent.task.succeeded",
            "payload": {
                "result": {
                    "kind": "context-summary.v1",
                    "summary": "This must not cite a foreign value.",
                    "citations": [Uuid::new_v4()],
                },
            },
            "occurredAt": Utc::now(),
        },
    });
    let (foreign_citation_status, _) = server
        .internal_json(
            "POST",
            &updates_path,
            Some(&gateway_token),
            Some(lease_token),
            Some(foreign_terminal_update),
        )
        .await;
    assert_eq!(foreign_citation_status, 422);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM room_events WHERE room_id = $1")
            .bind(room_id)
            .fetch_one(&server.pool)
            .await
            .unwrap(),
        room_event_count + 1
    );
}
