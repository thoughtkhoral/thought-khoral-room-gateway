BEGIN;

-- Independent profile storage: no native provider/session identifiers.
ALTER TABLE room_events ADD CONSTRAINT room_events_event_room_unique UNIQUE (event_id, room_id);

CREATE TABLE agent_conversations (
    conversation_id UUID PRIMARY KEY,
    room_id UUID NOT NULL,
    agent_id UUID NOT NULL CHECK (agent_id = '74686f75-6768-746b-686f-72616c000004'),
    generation BIGINT NOT NULL CHECK (generation BETWEEN 1 AND 9007199254740991),
    state TEXT NOT NULL CHECK (state IN ('reserved', 'running', 'ready', 'unusable', 'superseded')),
    consumed_revision BIGINT NOT NULL DEFAULT 0 CHECK (consumed_revision BETWEEN 0 AND 9007199254740991),
    policy_revision TEXT NOT NULL CHECK (char_length(policy_revision) BETWEEN 1 AND 128),
    guidance_revision TEXT NOT NULL CHECK (char_length(guidance_revision) BETWEEN 1 AND 128),
    selected_settings JSONB NOT NULL,
    active_task_id UUID,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    UNIQUE (room_id, agent_id, generation),
    UNIQUE (conversation_id, room_id, agent_id, generation),
    CHECK ((state IN ('reserved', 'running')) = (active_task_id IS NOT NULL)),
    CHECK (state <> 'ready' OR consumed_revision > 0)
);
CREATE UNIQUE INDEX agent_conversations_active_room_agent
    ON agent_conversations (room_id, agent_id) WHERE state <> 'superseded';

CREATE TABLE conversation_tasks (
    task_id UUID PRIMARY KEY,
    room_id UUID NOT NULL,
    request_id UUID NOT NULL,
    requester_id UUID NOT NULL,
    agent_id UUID NOT NULL,
    conversation_id UUID NOT NULL,
    generation BIGINT NOT NULL CHECK (generation BETWEEN 1 AND 9007199254740991),
    trigger_event_id UUID NOT NULL,
    context_revision BIGINT NOT NULL CHECK (context_revision BETWEEN 1 AND 9007199254740991),
    state TEXT NOT NULL CHECK (state IN ('reserved', 'running', 'completed', 'failed')),
    frozen_input JSONB NOT NULL,
    selected_settings JSONB NOT NULL,
    source_manifest JSONB NOT NULL,
    policy_revision TEXT NOT NULL CHECK (char_length(policy_revision) BETWEEN 1 AND 128),
    authorization_expires_at TIMESTAMPTZ NOT NULL,
    issued_at TIMESTAMPTZ NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    lease_owner TEXT CHECK (char_length(lease_owner) BETWEEN 1 AND 128),
    lease_expires_at TIMESTAMPTZ,
    lease_token UUID,
    receipt_acknowledgement JSONB,
    reply_event_id UUID,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    accepted_turn JSONB NOT NULL,
    UNIQUE (room_id, request_id),
    UNIQUE (task_id, conversation_id, room_id, agent_id, generation),
    FOREIGN KEY (room_id, request_id) REFERENCES room_requests (room_id, request_id),
    FOREIGN KEY (conversation_id, room_id, agent_id, generation)
        REFERENCES agent_conversations (conversation_id, room_id, agent_id, generation),
    FOREIGN KEY (trigger_event_id, room_id) REFERENCES room_events (event_id, room_id),
    FOREIGN KEY (reply_event_id, room_id) REFERENCES room_events (event_id, room_id),
    CHECK (issued_at < expires_at AND expires_at <= authorization_expires_at),
    CHECK (expires_at <= issued_at + interval '180 seconds'),
    CHECK ((lease_owner IS NULL AND lease_expires_at IS NULL AND lease_token IS NULL)
        OR (lease_owner IS NOT NULL AND lease_expires_at IS NOT NULL AND lease_token IS NOT NULL)),
    CHECK (jsonb_typeof(frozen_input) = 'object' AND jsonb_typeof(source_manifest) = 'array')
);
ALTER TABLE agent_conversations ADD CONSTRAINT conversation_active_task_binding
    FOREIGN KEY (active_task_id, conversation_id, room_id, agent_id, generation)
    REFERENCES conversation_tasks (task_id, conversation_id, room_id, agent_id, generation)
    DEFERRABLE INITIALLY DEFERRED;

CREATE TABLE conversation_updates (
    task_id UUID NOT NULL,
    conversation_id UUID NOT NULL,
    room_id UUID NOT NULL,
    agent_id UUID NOT NULL,
    generation BIGINT NOT NULL,
    update_id UUID NOT NULL,
    -- 256 caps retained rows in the service, not monotonically increasing ordinals.
    ordinal BIGINT NOT NULL CHECK (ordinal BETWEEN 1 AND 9007199254740991),
    kind TEXT NOT NULL CHECK (kind IN ('progress', 'settings', 'usage', 'completed', 'failed')),
    occurred_at TIMESTAMPTZ NOT NULL,
    data JSONB NOT NULL,
    PRIMARY KEY (task_id, update_id),
    UNIQUE (task_id, ordinal),
    FOREIGN KEY (task_id, conversation_id, room_id, agent_id, generation)
        REFERENCES conversation_tasks (task_id, conversation_id, room_id, agent_id, generation)
);

CREATE TABLE conversation_disclosures (
    conversation_id UUID NOT NULL,
    room_id UUID NOT NULL,
    agent_id UUID NOT NULL,
    generation BIGINT NOT NULL,
    source_id UUID NOT NULL,
    source_kind TEXT NOT NULL CHECK (source_kind IN ('event', 'decision')),
    first_task_id UUID NOT NULL,
    policy_revision TEXT NOT NULL CHECK (char_length(policy_revision) BETWEEN 1 AND 128),
    disclosed_revision BIGINT NOT NULL CHECK (disclosed_revision BETWEEN 1 AND 9007199254740991),
    PRIMARY KEY (conversation_id, source_id),
    FOREIGN KEY (first_task_id, conversation_id, room_id, agent_id, generation)
        REFERENCES conversation_tasks (task_id, conversation_id, room_id, agent_id, generation)
);

COMMIT;
