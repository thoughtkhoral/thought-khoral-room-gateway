use chrono::{Duration, Utc};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row, postgres::PgPoolOptions};
use thought_khoral_room_gateway::conversation_protocol::{CODEX_AGENT_ID, ConversationError};
use thought_khoral_room_gateway::conversation_store::{ConversationPolicy, ConversationStore};
use thought_khoral_room_gateway::{Actor, ActorRole, NewEvent, append_event, events_after};
use uuid::Uuid;

async fn database() -> PgPool {
    PgPoolOptions::new()
        .max_connections(5)
        .connect(
            &std::env::var("DATABASE_URL").expect("dedicated migrated PostgreSQL DATABASE_URL"),
        )
        .await
        .unwrap()
}
fn policy() -> ConversationPolicy {
    serde_json::from_value(json!({"enabled":true,"policyRevision":"policy-1","guidanceRevision":"fixed-1","catalogRevision":"catalog-1","model":"model-a","reasoningEffort":"medium","models":[{"id":"model-a","reasoningEfforts":["medium","low"]}]})).unwrap()
}
fn actor() -> Actor {
    Actor {
        id: Uuid::new_v4(),
        role: ActorRole::Human,
        display_name: "Test Human".into(),
        expires_at: (Utc::now() + Duration::minutes(10)).timestamp(),
    }
}
fn request(room: Uuid) -> Value {
    let mut value: Value = serde_json::from_str(include_str!(
        "../contracts/agent-conversation-v1/fixtures/valid/first-turn.json"
    ))
    .unwrap();
    value["requestId"] = json!(Uuid::new_v4());
    value["roomId"] = json!(room);
    value
}
async fn reserve(
    store: &ConversationStore,
    actor: &Actor,
    request: &Value,
) -> Result<Value, ConversationError> {
    store
        .reserve(
            actor,
            request,
            chrono::DateTime::from_timestamp(actor.expires_at, 0).unwrap(),
        )
        .await
}
async fn task(pool: &PgPool, accepted: &Value) -> sqlx::postgres::PgRow {
    sqlx::query("SELECT * FROM conversation_tasks WHERE task_id=$1")
        .bind(Uuid::parse_str(accepted["taskId"].as_str().unwrap()).unwrap())
        .fetch_one(pool)
        .await
        .unwrap()
}
async fn seed_message(
    pool: &PgPool,
    room: Uuid,
    targeted: bool,
) -> thought_khoral_room_gateway::RoomEvent {
    append_event(pool, NewEvent { room_id:room, request_id:Uuid::new_v4(), event_type:"message.created".into(), actor_id:Uuid::new_v4(), actor_role:"human".into(), actor_display_name: Some("Maya".into()), payload: if targeted { json!({"text":"private secret","delivery":"mentioned","mentions":[{"type":"participant","id":Uuid::new_v4(),"token":"leo"}],"audienceIds":[Uuid::new_v4()]}) } else { json!({"text":"Use the green example.","delivery":"room","mentions":[],"audienceIds":[]}) }, occurred_at:Utc::now() }).await.unwrap()
}
async fn seed_ready(pool: &PgPool, accepted: &Value, state: &str) {
    let task_id = Uuid::parse_str(accepted["taskId"].as_str().unwrap()).unwrap();
    let conversation = Uuid::parse_str(accepted["conversationId"].as_str().unwrap()).unwrap();
    // Fixture setup represents Task 3's future broker transition; never provider history.
    sqlx::query("UPDATE conversation_tasks SET state='failed' WHERE task_id=$1")
        .bind(task_id)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("UPDATE agent_conversations SET state=$2, active_task_id=NULL, consumed_revision=$3 WHERE conversation_id=$1").bind(conversation).bind(state).bind(accepted["contextRevision"].as_i64().unwrap()).execute(pool).await.unwrap();
}

// These fail if request replay resolves mutable defaults, or if requester binding is omitted.
#[tokio::test]
async fn replay_preserves_frozen_settings_and_rejects_changed_intent_or_requester() {
    let pool = database().await;
    let room = Uuid::new_v4();
    let human = actor();
    let input = request(room);
    let store = ConversationStore::new(pool.clone())
        .with_policy(policy())
        .unwrap();
    let accepted = reserve(&store, &human, &input).await.unwrap();
    let mut changed_policy = policy();
    changed_policy.reasoning_effort = "low".into();
    changed_policy.catalog_revision = "catalog-2".into();
    let changed = ConversationStore::new(pool.clone())
        .with_policy(changed_policy)
        .unwrap();
    assert_eq!(reserve(&changed, &human, &input).await.unwrap(), accepted);
    let mut changed_input = input.clone();
    changed_input["text"] = json!("different intent");
    assert_eq!(
        reserve(&store, &human, &changed_input).await,
        Err(ConversationError::DuplicateConflict)
    );
    assert_eq!(
        reserve(&store, &actor(), &input).await,
        Err(ConversationError::DuplicateConflict)
    );
    assert_eq!(events_after(&pool, room, 0).await.unwrap().len(), 1);
    let row = task(&pool, &accepted).await;
    assert_eq!(row.get::<Value, _>("accepted_turn"), accepted);
    assert_eq!(
        row.get::<Value, _>("frozen_input")["reasoningEffort"],
        "medium"
    );
    assert_eq!(
        row.get::<Value, _>("frozen_input")["context"]["digest"]
            .as_str()
            .unwrap()
            .len(),
        64
    );
    assert!(row.get::<Option<String>, _>("lease_owner").is_none());
}

#[tokio::test]
async fn distinct_requests_race_to_one_prompt_and_one_task() {
    let pool = database().await;
    let room = Uuid::new_v4();
    let human = actor();
    let store = ConversationStore::new(pool.clone())
        .with_policy(policy())
        .unwrap();
    let first = request(room);
    let second = request(room);
    let (a, b) = tokio::join!(
        reserve(&store, &human, &first),
        reserve(&store, &human, &second)
    );
    assert!(matches!(
        (&a, &b),
        (Ok(_), Err(ConversationError::ConversationBusy))
            | (Err(ConversationError::ConversationBusy), Ok(_))
    ));
    assert_eq!(events_after(&pool, room, 0).await.unwrap().len(), 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM conversation_tasks WHERE room_id=$1")
            .bind(room)
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn identical_requests_race_to_the_same_acceptance() {
    let pool = database().await;
    let store = ConversationStore::new(pool.clone())
        .with_policy(policy())
        .unwrap();
    let human = actor();
    let input = request(Uuid::new_v4());
    let (a, b) = tokio::join!(
        reserve(&store, &human, &input),
        reserve(&store, &human, &input)
    );
    assert_eq!(a.unwrap(), b.unwrap());
}

#[tokio::test]
async fn authority_and_disabled_policy_are_checked_before_replay() {
    let pool = database().await;
    let room = Uuid::new_v4();
    let mut human = actor();
    let input = request(room);
    assert_eq!(
        reserve(&ConversationStore::new(pool.clone()), &human, &input).await,
        Err(ConversationError::RuntimeUnavailable)
    );
    let store = ConversationStore::new(pool.clone())
        .with_policy(policy())
        .unwrap();
    reserve(&store, &human, &input).await.unwrap();
    human.role = ActorRole::Agent;
    assert_eq!(
        reserve(&store, &human, &input).await,
        Err(ConversationError::Forbidden)
    );
    human.role = ActorRole::Human;
    human.expires_at = (Utc::now() + Duration::seconds(20)).timestamp();
    assert_eq!(
        store
            .reserve(&human, &input, Utc::now() + Duration::hours(1))
            .await,
        Err(ConversationError::AuthenticationRequired)
    );
    assert_eq!(events_after(&pool, room, 0).await.unwrap().len(), 1);
}

#[tokio::test]
async fn frozen_context_excludes_targeted_history_and_contains_one_trigger() {
    let pool = database().await;
    let room = Uuid::new_v4();
    let public = seed_message(&pool, room, false).await;
    seed_message(&pool, room, true).await;
    let store = ConversationStore::new(pool.clone())
        .with_policy(policy())
        .unwrap();
    let accepted = reserve(&store, &actor(), &request(room)).await.unwrap();
    let frozen = task(&pool, &accepted).await.get::<Value, _>("frozen_input");
    assert_eq!(frozen["context"]["entries"].as_array().unwrap().len(), 2);
    assert_eq!(
        frozen["context"]["entries"][0]["eventId"],
        json!(public.event_id)
    );
    assert_eq!(
        frozen["context"]["entries"][1]["eventId"],
        accepted["triggerEventId"]
    );
    assert!(!frozen.to_string().contains("private secret"));
    seed_message(&pool, room, false).await;
    assert_eq!(
        task(&pool, &accepted).await.get::<Value, _>("frozen_input"),
        frozen
    );
}

#[tokio::test]
async fn explicit_new_supersedes_atomically_and_continue_binds_the_room() {
    let pool = database().await;
    let room = Uuid::new_v4();
    let human = actor();
    let store = ConversationStore::new(pool.clone())
        .with_policy(policy())
        .unwrap();
    let first = reserve(&store, &human, &request(room)).await.unwrap();
    assert_eq!(
        reserve(&store, &human, &request(room)).await,
        Err(ConversationError::ConversationBusy)
    );
    seed_ready(&pool, &first, "ready").await;
    let mut continuation = request(Uuid::new_v4());
    continuation["conversation"] =
        json!({"mode":"continue","id":first["conversationId"],"generation":first["generation"]});
    assert_eq!(
        reserve(&store, &human, &continuation).await,
        Err(ConversationError::ConversationStale)
    );
    let second = reserve(&store, &human, &request(room)).await.unwrap();
    assert_eq!(second["generation"], 2);
    assert_ne!(second["conversationId"], first["conversationId"]);
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT state FROM agent_conversations WHERE conversation_id=$1"
        )
        .bind(Uuid::parse_str(first["conversationId"].as_str().unwrap()).unwrap())
        .fetch_one(&pool)
        .await
        .unwrap(),
        "superseded"
    );
}

#[tokio::test]
async fn context_and_model_failures_leave_no_partial_reservation() {
    let pool = database().await;
    let room = Uuid::new_v4();
    let human = actor();
    let mut small = policy();
    small.max_context_bytes = 32;
    let store = ConversationStore::new(pool.clone())
        .with_policy(small)
        .unwrap();
    assert_eq!(
        reserve(&store, &human, &request(room)).await,
        Err(ConversationError::ContextTooLarge)
    );
    let store = ConversationStore::new(pool.clone())
        .with_policy(policy())
        .unwrap();
    let mut input = request(room);
    input["settings"] =
        json!({"model":"unknown","reasoningEffort":"medium","catalogRevision":"catalog-1"});
    assert_eq!(
        reserve(&store, &human, &input).await,
        Err(ConversationError::InvalidTaskInput)
    );
    assert!(events_after(&pool, room, 0).await.unwrap().is_empty());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM agent_conversations WHERE room_id=$1")
            .bind(room)
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM room_requests WHERE room_id=$1")
            .bind(room)
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
}

#[test]
fn policy_requires_allowed_defaults_and_can_only_lower_caps() {
    assert!(!ConversationPolicy::default().enabled);
    assert!(policy().validate().is_ok());
    let mut invalid = policy();
    invalid.max_context_bytes = 1_048_577;
    assert!(invalid.validate().is_err());
    invalid = policy();
    invalid.reasoning_effort = "unsupported".into();
    assert!(invalid.validate().is_err());
    invalid = policy();
    invalid.models.push(invalid.models[0].clone());
    assert!(invalid.validate().is_err());
}

#[tokio::test]
async fn omitted_mode_continues_ready_generation_with_unadvanced_cursor() {
    let pool = database().await;
    let room = Uuid::new_v4();
    let store = ConversationStore::new(pool.clone())
        .with_policy(policy())
        .unwrap();
    let first = reserve(&store, &actor(), &request(room)).await.unwrap();
    seed_ready(&pool, &first, "ready").await;
    let public = seed_message(&pool, room, false).await;
    let mut next = request(room);
    next.as_object_mut().unwrap().remove("conversation");
    let accepted = reserve(&store, &actor(), &next).await.unwrap();
    assert_eq!(accepted["conversationId"], first["conversationId"]);
    let frozen = task(&pool, &accepted).await.get::<Value, _>("frozen_input");
    assert_eq!(frozen["conversation"]["mode"], "continue");
    assert_eq!(frozen["context"]["kind"], "delta");
    assert_eq!(frozen["context"]["baseRevision"], first["contextRevision"]);
    assert_eq!(
        frozen["context"]["entries"][0]["eventId"],
        json!(public.event_id)
    );
    let cursor: i64 = sqlx::query_scalar(
        "SELECT consumed_revision FROM agent_conversations WHERE room_id=$1 AND state='reserved'",
    )
    .bind(room)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(json!(cursor), first["contextRevision"]);
}

#[tokio::test]
async fn unusable_or_changed_policy_requires_new_and_failed_reset_preserves_previous_state() {
    let pool = database().await;
    let room = Uuid::new_v4();
    let human = actor();
    let store = ConversationStore::new(pool.clone())
        .with_policy(policy())
        .unwrap();
    let first = reserve(&store, &human, &request(room)).await.unwrap();
    seed_ready(&pool, &first, "unusable").await;
    let mut next = request(room);
    next.as_object_mut().unwrap().remove("conversation");
    assert_eq!(
        reserve(&store, &human, &next).await,
        Err(ConversationError::ConversationStale)
    );
    seed_ready(&pool, &first, "ready").await;
    let mut changed = policy();
    changed.policy_revision = "narrower-policy-2".into();
    let changed_store = ConversationStore::new(pool.clone())
        .with_policy(changed)
        .unwrap();
    assert_eq!(
        reserve(&changed_store, &human, &next).await,
        Err(ConversationError::ConversationStale)
    );
    let mut small = policy();
    small.max_context_bytes = 32;
    let small_store = ConversationStore::new(pool.clone())
        .with_policy(small)
        .unwrap();
    assert_eq!(
        reserve(&small_store, &human, &request(room)).await,
        Err(ConversationError::ContextTooLarge)
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT state FROM agent_conversations WHERE room_id=$1 AND state<>'superseded'"
        )
        .bind(room)
        .fetch_one(&pool)
        .await
        .unwrap(),
        "ready"
    );
    assert_eq!(events_after(&pool, room, 0).await.unwrap().len(), 1);
}

#[tokio::test]
async fn ordinary_request_ledger_collision_never_creates_another_message() {
    let pool = database().await;
    let room = Uuid::new_v4();
    let previous = seed_message(&pool, room, false).await;
    sqlx::query("INSERT INTO room_requests(room_id,request_id,request_fingerprint,event_ids) VALUES ($1,$2,$3,$4)").bind(room).bind(previous.request_id).bind(json!({"method":"chat.send"})).bind(vec![previous.event_id]).execute(&pool).await.unwrap();
    let mut input = request(room);
    input["requestId"] = json!(previous.request_id);
    let store = ConversationStore::new(pool.clone())
        .with_policy(policy())
        .unwrap();
    assert_eq!(
        reserve(&store, &actor(), &input).await,
        Err(ConversationError::DuplicateConflict)
    );
    assert_eq!(events_after(&pool, room, 0).await.unwrap().len(), 1);
}

#[tokio::test]
async fn decision_projection_and_source_manifest_exclude_targeted_or_missing_sources() {
    let pool = database().await;
    let room = Uuid::new_v4();
    let public = seed_message(&pool, room, false).await;
    let secret = seed_message(&pool, room, true).await;
    let visible = Uuid::new_v4();
    for (id, source) in [
        (visible, public.event_id),
        (Uuid::new_v4(), secret.event_id),
        (Uuid::new_v4(), Uuid::new_v4()),
    ] {
        sqlx::query("INSERT INTO decisions(decision_id,room_id,status,title,summary,source_event_ids) VALUES ($1,$2,'active','Example color','Use green',$3)").bind(id).bind(room).bind(vec![source]).execute(&pool).await.unwrap();
    }
    let store = ConversationStore::new(pool.clone())
        .with_policy(policy())
        .unwrap();
    let accepted = reserve(&store, &actor(), &request(room)).await.unwrap();
    let row = task(&pool, &accepted).await;
    let frozen = row.get::<Value, _>("frozen_input");
    assert_eq!(
        frozen["context"]["activeDecisions"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        frozen["context"]["activeDecisions"][0]["decisionId"],
        json!(visible)
    );
    let manifest = row.get::<Value, _>("source_manifest");
    assert!(
        manifest.as_array().unwrap().iter().any(
            |source| source["sourceId"] == json!(visible) && source["sourceKind"] == "decision"
        )
    );
    assert!(!manifest.to_string().contains(&secret.event_id.to_string()));
}

async fn acknowledged_reply(
    pool: &PgPool,
    accepted: &Value,
) -> thought_khoral_room_gateway::RoomEvent {
    let room = Uuid::parse_str(accepted["roomId"].as_str().unwrap()).unwrap();
    let reply=append_event(pool,NewEvent {room_id:room,request_id:Uuid::new_v4(),event_type:"message.created".into(),actor_id:CODEX_AGENT_ID,actor_role:"agent".into(),actor_display_name:Some("Codex".into()),payload:json!({"text":"Accepted assistant reply","delivery":"room","mentions":[],"audienceIds":[]}),occurred_at:Utc::now()}).await.unwrap();
    let frozen = task(pool, accepted).await.get::<Value, _>("frozen_input");
    let ack = json!({"profileVersion":frozen["profileVersion"],"taskId":accepted["taskId"],"conversationId":accepted["conversationId"],"generation":accepted["generation"],"replyEventId":reply.event_id,"replySequence":reply.sequence,"textDigest":format!("{:x}",Sha256::digest("Accepted assistant reply")),"consumedRevision":accepted["contextRevision"],"contextDigest":frozen["context"]["digest"]});
    sqlx::query("UPDATE conversation_tasks SET state='completed',reply_event_id=$2,receipt_acknowledgement=$3 WHERE task_id=$1").bind(Uuid::parse_str(accepted["taskId"].as_str().unwrap()).unwrap()).bind(reply.event_id).bind(ack).execute(pool).await.unwrap();
    sqlx::query("UPDATE agent_conversations SET state='ready',active_task_id=NULL,consumed_revision=$2 WHERE conversation_id=$1").bind(Uuid::parse_str(accepted["conversationId"].as_str().unwrap()).unwrap()).bind(accepted["contextRevision"].as_i64().unwrap()).execute(pool).await.unwrap();
    reply
}

#[tokio::test]
async fn delta_substitutes_only_a_verified_same_generation_broker_acknowledgement() {
    let pool = database().await;
    let room = Uuid::new_v4();
    let store = ConversationStore::new(pool.clone())
        .with_policy(policy())
        .unwrap();
    let human = actor();
    let first = reserve(&store, &human, &request(room)).await.unwrap();
    let reply = acknowledged_reply(&pool, &first).await;
    let mut next = request(room);
    next.as_object_mut().unwrap().remove("conversation");
    let accepted = reserve(&store, &human, &next).await.unwrap();
    let frozen = task(&pool, &accepted).await.get::<Value, _>("frozen_input");
    assert_eq!(
        frozen["context"]["nativeReplyBindings"][0]["eventId"],
        json!(reply.event_id)
    );
    assert!(!frozen.to_string().contains("Accepted assistant reply"));
    seed_ready(&pool, &accepted, "ready").await;
    let reset = reserve(&store, &human, &request(room)).await.unwrap();
    let baseline = task(&pool, &reset).await.get::<Value, _>("frozen_input");
    assert!(baseline.to_string().contains("Accepted assistant reply"));
    assert!(
        baseline["context"]["nativeReplyBindings"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn acknowledgement_with_wrong_context_digest_cannot_suppress_public_text() {
    let pool = database().await;
    let room = Uuid::new_v4();
    let store = ConversationStore::new(pool.clone())
        .with_policy(policy())
        .unwrap();
    let human = actor();
    let first = reserve(&store, &human, &request(room)).await.unwrap();
    acknowledged_reply(&pool, &first).await;
    sqlx::query("UPDATE conversation_tasks SET receipt_acknowledgement=jsonb_set(receipt_acknowledgement,'{contextDigest}',to_jsonb(repeat('0',64))) WHERE task_id=$1").bind(Uuid::parse_str(first["taskId"].as_str().unwrap()).unwrap()).execute(&pool).await.unwrap();
    let mut next = request(room);
    next.as_object_mut().unwrap().remove("conversation");
    assert_eq!(
        reserve(&store, &human, &next).await,
        Err(ConversationError::ContextMismatch)
    );
    assert_eq!(events_after(&pool, room, 0).await.unwrap().len(), 2);
}

#[tokio::test]
async fn database_constraints_reject_cross_room_trigger_and_generation_bindings() {
    let pool = database().await;
    let store = ConversationStore::new(pool.clone())
        .with_policy(policy())
        .unwrap();
    let first = reserve(&store, &actor(), &request(Uuid::new_v4()))
        .await
        .unwrap();
    let other = seed_message(&pool, Uuid::new_v4(), false).await;
    let task_id = Uuid::parse_str(first["taskId"].as_str().unwrap()).unwrap();
    assert!(
        sqlx::query("UPDATE conversation_tasks SET trigger_event_id=$2 WHERE task_id=$1")
            .bind(task_id)
            .bind(other.event_id)
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("UPDATE conversation_tasks SET generation=generation+1 WHERE task_id=$1")
            .bind(task_id)
            .execute(&pool)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn coalesced_update_ordinals_can_exceed_the_retained_row_limit() {
    let pool = database().await;
    let store = ConversationStore::new(pool.clone())
        .with_policy(policy())
        .unwrap();
    let accepted = reserve(&store, &actor(), &request(Uuid::new_v4()))
        .await
        .unwrap();
    let result=sqlx::query("INSERT INTO conversation_updates(task_id,conversation_id,room_id,agent_id,generation,update_id,ordinal,kind,occurred_at,data) SELECT task_id,conversation_id,room_id,agent_id,generation,$2,257,'progress',CURRENT_TIMESTAMP,'{\"phase\":\"working\",\"text\":\"Synthetic progress\"}'::jsonb FROM conversation_tasks WHERE task_id=$1")
        .bind(Uuid::parse_str(accepted["taskId"].as_str().unwrap()).unwrap()).bind(Uuid::new_v4()).execute(&pool).await;
    assert!(
        result.is_ok(),
        "valid ordinals preserve gaps after coalescing: {result:?}"
    );
}

#[tokio::test]
async fn context_byte_cap_counts_disclosed_fields_instead_of_retained_message_metadata() {
    let pool = database().await;
    let room = Uuid::new_v4();
    append_event(&pool,NewEvent{room_id:room,request_id:Uuid::new_v4(),event_type:"message.created".into(),actor_id:Uuid::new_v4(),actor_role:"human".into(),actor_display_name:Some("Maya".into()),payload:json!({"text":"Public fact","delivery":"room","audienceIds":[],"mentions":[{"type":"participant","id":CODEX_AGENT_ID,"token":"a".repeat(5000)}]}),occurred_at:Utc::now()}).await.unwrap();
    let mut small = policy();
    small.max_context_bytes = 2000;
    let store = ConversationStore::new(pool.clone())
        .with_policy(small)
        .unwrap();
    assert!(reserve(&store, &actor(), &request(room)).await.is_ok());
}

#[tokio::test]
async fn ready_generation_requires_a_committed_positive_cursor() {
    let pool = database().await;
    let store = ConversationStore::new(pool.clone())
        .with_policy(policy())
        .unwrap();
    let accepted = reserve(&store, &actor(), &request(Uuid::new_v4()))
        .await
        .unwrap();
    let result=sqlx::query("UPDATE agent_conversations SET state='ready',active_task_id=NULL,consumed_revision=0 WHERE conversation_id=$1").bind(Uuid::parse_str(accepted["conversationId"].as_str().unwrap()).unwrap()).execute(&pool).await;
    assert!(result.is_err());
}
