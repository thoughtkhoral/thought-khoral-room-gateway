use chrono::Utc;
use n2n_room_gateway::{NewEvent, StoreError, append_event};
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

fn new_event(room_id: Uuid, actor_id: Uuid) -> NewEvent {
    NewEvent {
        room_id,
        request_id: Uuid::new_v4(),
        event_type: "message.created".to_owned(),
        actor_id,
        actor_role: "human".to_owned(),
        payload: json!({ "text": "message" }),
        occurred_at: Utc::now(),
    }
}

// This fails if appends do not atomically allocate consecutive room-local sequences.
#[tokio::test]
async fn appends_immutable_events_with_room_local_sequences() {
    let database_url = std::env::var("DATABASE_URL")
        .expect("DATABASE_URL must name a migrated PostgreSQL database for this integration test");
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await
        .expect("the migration-test database must be reachable");
    let room_id = Uuid::new_v4();
    let actor_id = Uuid::new_v4();

    let first = append_event(&pool, new_event(room_id, actor_id))
        .await
        .expect("the first event must append");
    let second = append_event(&pool, new_event(room_id, actor_id))
        .await
        .expect("the second event must append");

    assert_eq!(first.sequence, 1);
    assert_eq!(second.sequence, 2);
    assert_eq!(first.room_id, second.room_id);
}

// This fails if arbitrary values can bypass the pinned persisted-event contract.
#[tokio::test]
async fn rejects_invalid_event_type_before_inserting() {
    let database_url = std::env::var("DATABASE_URL").unwrap();
    let pool = PgPoolOptions::new().connect(&database_url).await.unwrap();
    let mut event = new_event(Uuid::new_v4(), Uuid::new_v4());
    event.event_type = "message.deleted".to_owned();

    assert!(matches!(
        append_event(&pool, event).await,
        Err(StoreError::InvalidEvent)
    ));
}

// This fails if an unsupported actor role can bypass the pinned persisted-event contract.
#[tokio::test]
async fn rejects_invalid_actor_role_before_inserting() {
    let database_url = std::env::var("DATABASE_URL").unwrap();
    let pool = PgPoolOptions::new().connect(&database_url).await.unwrap();
    let mut event = new_event(Uuid::new_v4(), Uuid::new_v4());
    event.actor_role = "service".to_owned();

    assert!(matches!(
        append_event(&pool, event).await,
        Err(StoreError::InvalidEvent)
    ));
}

// This fails if a JSON scalar or array is persisted where the contract requires an object.
#[tokio::test]
async fn rejects_non_object_payload_before_inserting() {
    let database_url = std::env::var("DATABASE_URL").unwrap();
    let pool = PgPoolOptions::new().connect(&database_url).await.unwrap();
    let mut event = new_event(Uuid::new_v4(), Uuid::new_v4());
    event.payload = json!(["not an object"]);

    assert!(matches!(
        append_event(&pool, event).await,
        Err(StoreError::InvalidEvent)
    ));
}

// This fails if any direct SQL UPDATE path can mutate a persisted room event.
#[tokio::test]
async fn database_rejects_room_event_updates() {
    let database_url = std::env::var("DATABASE_URL").unwrap();
    let pool = PgPoolOptions::new().connect(&database_url).await.unwrap();
    let event = append_event(&pool, new_event(Uuid::new_v4(), Uuid::new_v4()))
        .await
        .unwrap();

    let error = sqlx::query("UPDATE room_events SET event_type = 'changed' WHERE event_id = $1")
        .bind(event.event_id)
        .execute(&pool)
        .await
        .expect_err("the append-only trigger must reject updates");
    assert_eq!(
        error
            .as_database_error()
            .and_then(|database_error| database_error.code().map(|code| code.into_owned())),
        Some("55000".to_owned())
    );
}

// This fails if any direct SQL DELETE path can remove the highest event and enable reuse.
#[tokio::test]
async fn database_rejects_room_event_deletes_and_preserves_sequence_progress() {
    let database_url = std::env::var("DATABASE_URL").unwrap();
    let pool = PgPoolOptions::new().connect(&database_url).await.unwrap();
    let room_id = Uuid::new_v4();
    let actor_id = Uuid::new_v4();
    let event = append_event(&pool, new_event(room_id, actor_id))
        .await
        .unwrap();

    let error = sqlx::query("DELETE FROM room_events WHERE event_id = $1")
        .bind(event.event_id)
        .execute(&pool)
        .await
        .expect_err("the append-only trigger must reject deletes");
    assert_eq!(
        error
            .as_database_error()
            .and_then(|database_error| database_error.code().map(|code| code.into_owned())),
        Some("55000".to_owned())
    );

    let next = append_event(&pool, new_event(room_id, actor_id))
        .await
        .unwrap();
    assert_eq!(next.sequence, 2);
}

// This fails if concurrent multi-connection appends produce duplicate/gapped sequences or cross rooms.
#[tokio::test]
async fn concurrent_connections_allocate_independent_contiguous_room_sequences() {
    let database_url = std::env::var("DATABASE_URL").unwrap();
    let room_one = Uuid::new_v4();
    let room_two = Uuid::new_v4();
    let actor_id = Uuid::new_v4();
    let first = PgPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await
        .unwrap();
    let second = PgPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await
        .unwrap();
    let third = PgPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await
        .unwrap();
    let fourth = PgPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await
        .unwrap();

    let (one_a, one_b, two_a, two_b) = tokio::join!(
        append_event(&first, new_event(room_one, actor_id)),
        append_event(&second, new_event(room_one, actor_id)),
        append_event(&third, new_event(room_two, actor_id)),
        append_event(&fourth, new_event(room_two, actor_id)),
    );
    let mut one_sequences = vec![one_a.unwrap().sequence, one_b.unwrap().sequence];
    let mut two_sequences = vec![two_a.unwrap().sequence, two_b.unwrap().sequence];
    one_sequences.sort_unstable();
    two_sequences.sort_unstable();

    assert_eq!(one_sequences, vec![1, 2]);
    assert_eq!(two_sequences, vec![1, 2]);
    assert_ne!(room_one, room_two);
}
