use chrono::Utc;
use n2n_room_gateway::{NewEvent, append_event};
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

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

    let first = append_event(
        &pool,
        NewEvent {
            room_id,
            request_id: Uuid::new_v4(),
            event_type: "message.created".to_owned(),
            actor_id,
            actor_role: "human".to_owned(),
            payload: json!({ "text": "first" }),
            occurred_at: Utc::now(),
        },
    )
    .await
    .expect("the first event must append");
    let second = append_event(
        &pool,
        NewEvent {
            room_id,
            request_id: Uuid::new_v4(),
            event_type: "message.created".to_owned(),
            actor_id,
            actor_role: "human".to_owned(),
            payload: json!({ "text": "second" }),
            occurred_at: Utc::now(),
        },
    )
    .await
    .expect("the second event must append");

    assert_eq!(first.sequence, 1);
    assert_eq!(second.sequence, 2);
    assert_eq!(first.room_id, second.room_id);
}
