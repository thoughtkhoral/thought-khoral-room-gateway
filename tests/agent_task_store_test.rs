use chrono::{Duration, Utc};
use serde_json::json;
use sqlx::{PgPool, postgres::PgPoolOptions};
use thought_khoral_room_gateway::{
    AgentSkillId, AgentTaskLease, AgentTaskStart, AgentTaskState, AgentTaskStore, AgentTaskUpdate,
};
use uuid::Uuid;

async fn test_store() -> (AgentTaskStore, PgPool) {
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(
            &std::env::var("DATABASE_URL")
                .expect("DATABASE_URL must name a migrated PostgreSQL database"),
        )
        .await
        .expect("the integration database must be reachable");
    (
        AgentTaskStore::new(pool.clone(), Uuid::new_v4(), "Test Human"),
        pool,
    )
}

fn test_task_start() -> AgentTaskStart {
    AgentTaskStart {
        id: "agent-task-start".to_owned(),
        contract_version: "n2n.room.v1".to_owned(),
        request_id: Uuid::new_v4(),
        room_id: Uuid::new_v4(),
        occurred_at: Utc::now(),
        agent_id: Uuid::from_u128(0x74686f756768746b_686f72616c000003),
        skill_id: AgentSkillId::SummarizeContext,
        input: "Summarize the room.".to_owned(),
    }
}

fn first_lease_owner() -> Uuid {
    Uuid::from_u128(1)
}

fn second_lease_owner() -> Uuid {
    Uuid::from_u128(2)
}

fn test_progress_update() -> AgentTaskUpdate {
    AgentTaskUpdate {
        update_id: Uuid::from_u128(3),
        event_type: "agent.task.progressed".to_owned(),
        payload: json!({
            "phase": "working",
            "text": "Preparing the summary.",
            "percent": 50,
        }),
        occurred_at: Utc::now(),
    }
}

fn test_success_update() -> AgentTaskUpdate {
    AgentTaskUpdate {
        update_id: Uuid::from_u128(4),
        event_type: "agent.task.succeeded".to_owned(),
        payload: json!({
            "result": {
                "kind": "context-summary.v1",
                "summary": "The room agreed to ship the gateway foundation.",
                "citations": [],
            },
        }),
        occurred_at: Utc::now(),
    }
}

#[tokio::test]
async fn handoff_policy_is_enforced_before_persistence() {
    let (store, _) = test_store().await;
    let task = store.start_agent_task(test_task_start()).await.unwrap();
    let lease = store
        .claim_agent_task(task.task_id, first_lease_owner(), Utc::now())
        .await
        .unwrap()
        .unwrap();
    let valid = json!({"handoff": {
        "instruction": "Confirm the requested action.",
        "url": "https://reference-agent.thought-khoral.local/continue",
        "host": "reference-agent.thought-khoral.local",
        "expiresAt": Utc::now() + Duration::seconds(30),
    }});
    let mut invalid = Vec::new();
    for url in [
        "http://reference-agent.thought-khoral.local/continue",
        "https://user@reference-agent.thought-khoral.local/continue",
        "https://evil.example/continue",
        "https://reference-agent.thought-khoral.local/continue#secret",
    ] {
        let mut payload = valid.clone();
        payload["handoff"]["url"] = json!(url);
        invalid.push(payload);
    }
    let mut unregistered = valid.clone();
    let mut blank_instruction = valid.clone();
    blank_instruction["handoff"]["instruction"] = json!("   ");
    invalid.push(blank_instruction);
    unregistered["handoff"]["url"] = json!("https://evil.example/continue");
    unregistered["handoff"]["host"] = json!("evil.example");
    invalid.push(unregistered);
    for expiry in [
        Utc::now() - Duration::seconds(1),
        lease.expires_at + Duration::seconds(1),
    ] {
        let mut payload = valid.clone();
        payload["handoff"]["expiresAt"] = json!(expiry);
        invalid.push(payload);
    }
    let mut wrong_task = valid.clone();
    wrong_task["taskId"] = json!(Uuid::new_v4());
    invalid.push(wrong_task);
    let mut wrong_revision = valid.clone();
    wrong_revision["contextRevision"] = json!(999999);
    invalid.push(wrong_revision);
    for payload in invalid {
        assert!(
            store
                .record_agent_task_update(
                    &lease,
                    AgentTaskUpdate {
                        update_id: Uuid::new_v4(),
                        event_type: "agent.task.awaiting_external_input".to_owned(),
                        payload: payload.clone(),
                        occurred_at: Utc::now(),
                    }
                )
                .await
                .is_err(),
            "must reject {payload}"
        );
    }
    assert_eq!(
        store
            .room_event_count(task.events[0].room_id)
            .await
            .unwrap(),
        1
    );
    assert!(
        store
            .record_agent_task_update(
                &lease,
                AgentTaskUpdate {
                    update_id: Uuid::new_v4(),
                    event_type: "agent.task.awaiting_external_input".to_owned(),
                    payload: valid,
                    occurred_at: Utc::now(),
                }
            )
            .await
            .is_ok()
    );
}

// This fails if task creation omits either the immutable requested event or its queued record.
#[tokio::test]
async fn task_start_creates_one_requested_event_and_one_queued_record() {
    let (store, _) = test_store().await;
    let result = store.start_agent_task(test_task_start()).await.unwrap();

    assert_eq!(
        result
            .events
            .iter()
            .filter(|event| event.event_type == "agent.task.requested")
            .count(),
        1
    );
    assert_eq!(
        store.agent_task(result.task_id).await.unwrap().state,
        AgentTaskState::Queued
    );
}

// This fails if an untrusted client occurrence time can reorder the gateway-owned task queue.
#[tokio::test]
async fn task_enqueue_timestamp_is_not_backdated_from_client_occurrence_time() {
    let (store, _) = test_store().await;
    let mut start = test_task_start();
    start.occurred_at = Utc::now() - Duration::days(365);

    let task = store.start_agent_task(start.clone()).await.unwrap();
    let record = store.agent_task(task.task_id).await.unwrap();

    assert!(record.created_at > start.occurred_at);
    assert!(record.updated_at > start.occurred_at);
}

// This fails if an active worker lease can be stolen, or an expired one cannot be recovered.
#[tokio::test]
async fn expired_lease_can_be_claimed_again_but_live_lease_cannot() {
    let (store, _) = test_store().await;
    let task = store
        .start_agent_task(test_task_start())
        .await
        .unwrap()
        .task_id;
    let now = Utc::now();

    assert!(
        store
            .claim_agent_task(task, first_lease_owner(), now)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        store
            .claim_agent_task(task, second_lease_owner(), now)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .claim_agent_task(task, second_lease_owner(), now + Duration::minutes(6))
            .await
            .unwrap()
            .is_some()
    );
}

// This fails if an update replay appends another event instead of returning the original event.
#[tokio::test]
async fn duplicate_update_id_returns_prior_events_without_reappend() {
    let (store, _) = test_store().await;
    let task = store
        .start_agent_task(test_task_start())
        .await
        .unwrap()
        .task_id;
    let lease = store
        .claim_agent_task(task, first_lease_owner(), Utc::now())
        .await
        .unwrap()
        .unwrap();
    let update = test_progress_update();

    let first = store
        .record_agent_task_update(&lease, update.clone())
        .await
        .unwrap();
    let replay = store
        .record_agent_task_update(&lease, update)
        .await
        .unwrap();

    assert_eq!(first.events, replay.events);
    assert_eq!(
        store
            .room_event_count(first.events[0].room_id)
            .await
            .unwrap(),
        2
    );
}

// This fails if an expired worker can read context or append work after its lease ends.
#[tokio::test]
async fn expired_lease_cannot_read_context_or_record_an_update() {
    let (store, pool) = test_store().await;
    let task = store
        .start_agent_task(test_task_start())
        .await
        .unwrap()
        .task_id;
    let lease = store
        .claim_agent_task(task, first_lease_owner(), Utc::now())
        .await
        .unwrap()
        .unwrap();
    let expired_at = Utc::now() - Duration::seconds(1);
    let expired_lease = AgentTaskLease {
        expires_at: expired_at,
        ..lease
    };
    sqlx::query("UPDATE agent_tasks SET lease_expires_at = $1 WHERE task_id = $2")
        .bind(expired_at)
        .bind(task)
        .execute(&pool)
        .await
        .unwrap();

    assert!(
        store
            .context_for_lease(&expired_lease)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .record_agent_task_update(&expired_lease, test_progress_update())
            .await
            .is_err()
    );
    assert_eq!(
        store
            .room_event_count(store.agent_task(task).await.unwrap().room_id)
            .await
            .unwrap(),
        1
    );
}

// This fails if a completed task accepts a second update instead of only replaying its terminal one.
#[tokio::test]
async fn terminal_task_replays_its_terminal_update_but_rejects_a_new_one() {
    let (store, _) = test_store().await;
    let task = store
        .start_agent_task(test_task_start())
        .await
        .unwrap()
        .task_id;
    let lease = store
        .claim_agent_task(task, first_lease_owner(), Utc::now())
        .await
        .unwrap()
        .unwrap();
    let succeeded = test_success_update();

    let first = store
        .record_agent_task_update(&lease, succeeded.clone())
        .await
        .unwrap();
    let replay = store
        .record_agent_task_update(&lease, succeeded)
        .await
        .unwrap();
    assert_eq!(first.events, replay.events);
    assert!(
        store
            .record_agent_task_update(&lease, test_progress_update())
            .await
            .is_err()
    );
    assert_eq!(
        store
            .room_event_count(first.events[0].room_id)
            .await
            .unwrap(),
        2
    );
}
