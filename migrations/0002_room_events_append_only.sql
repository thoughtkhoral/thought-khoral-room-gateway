BEGIN;

CREATE FUNCTION reject_room_event_mutation()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    RAISE EXCEPTION 'room_events is append-only' USING ERRCODE = '55000';
END;
$$;

CREATE TRIGGER room_events_append_only
BEFORE UPDATE OR DELETE ON room_events
FOR EACH ROW
EXECUTE FUNCTION reject_room_event_mutation();

COMMIT;
