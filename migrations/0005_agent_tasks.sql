BEGIN;

CREATE TABLE agent_tasks (
    task_id UUID PRIMARY KEY,
    room_id UUID NOT NULL,
    request_id UUID NOT NULL,
    requester_id UUID NOT NULL,
    agent_id UUID NOT NULL,
    skill_id TEXT NOT NULL,
    input TEXT NOT NULL,
    context_revision BIGINT NOT NULL,
    state TEXT NOT NULL,
    lease_owner UUID NULL,
    lease_expires_at TIMESTAMPTZ NULL,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    UNIQUE (room_id, request_id)
);

CREATE INDEX agent_tasks_claimable_idx
    ON agent_tasks (state, lease_expires_at);

CREATE TABLE agent_task_updates (
    task_id UUID NOT NULL REFERENCES agent_tasks(task_id),
    update_id UUID NOT NULL,
    event_ids UUID[] NOT NULL,
    PRIMARY KEY (task_id, update_id)
);

COMMIT;
