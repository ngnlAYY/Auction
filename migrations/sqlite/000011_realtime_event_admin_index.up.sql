CREATE INDEX realtime_events_created_at_idx
    ON realtime_events (created_at DESC, event_id DESC);
