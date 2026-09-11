BEGIN;

CREATE TABLE room_events (
    event_id UUID PRIMARY KEY,
    room_id UUID NOT NULL,
    sequence BIGINT NOT NULL,
    request_id UUID NOT NULL,
    event_type TEXT NOT NULL,
    actor_id UUID NOT NULL,
    actor_role TEXT NOT NULL,
    payload JSONB NOT NULL,
    occurred_at TIMESTAMPTZ NOT NULL,
    UNIQUE (room_id, sequence),
    UNIQUE (room_id, request_id)
);

CREATE TABLE decisions (
    decision_id UUID PRIMARY KEY,
    room_id UUID NOT NULL,
    status TEXT NOT NULL,
    title TEXT NOT NULL,
    summary TEXT NOT NULL,
    derived_from_decision_id UUID NULL,
    source_event_ids UUID[] NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

COMMIT;
