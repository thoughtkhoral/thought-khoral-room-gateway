BEGIN;

ALTER TABLE room_events
    ADD COLUMN actor_display_name TEXT;

COMMIT;
