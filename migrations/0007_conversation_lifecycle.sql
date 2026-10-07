BEGIN;

ALTER TABLE conversation_tasks
    ADD COLUMN result JSONB,
    ADD COLUMN failure JSONB,
    ADD COLUMN effective_settings JSONB,
    ADD COLUMN usage JSONB,
    ADD COLUMN last_update_ordinal BIGINT NOT NULL DEFAULT 0
        CHECK (last_update_ordinal BETWEEN 0 AND 9007199254740991),
    ADD COLUMN last_progress_at TIMESTAMPTZ;

-- Coalescing browser snapshots must not erase replay fingerprints.
CREATE TABLE conversation_update_requests (
    task_id UUID NOT NULL,
    conversation_id UUID NOT NULL,
    room_id UUID NOT NULL,
    agent_id UUID NOT NULL,
    generation BIGINT NOT NULL,
    update_id UUID NOT NULL,
    ordinal BIGINT NOT NULL CHECK (ordinal BETWEEN 1 AND 9007199254740991),
    request JSONB NOT NULL,
    response JSONB NOT NULL,
    received_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (task_id, update_id),
    UNIQUE (task_id, ordinal),
    FOREIGN KEY (task_id, conversation_id, room_id, agent_id, generation)
        REFERENCES conversation_tasks (task_id, conversation_id, room_id, agent_id, generation)
);

COMMIT;
