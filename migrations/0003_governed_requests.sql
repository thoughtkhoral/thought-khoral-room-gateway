BEGIN;

ALTER TABLE room_events
    DROP CONSTRAINT room_events_room_id_request_id_key;

CREATE TABLE room_requests (
    room_id UUID NOT NULL,
    request_id UUID NOT NULL,
    request_fingerprint JSONB NOT NULL,
    event_ids UUID[] NOT NULL,
    PRIMARY KEY (room_id, request_id)
);

COMMIT;
