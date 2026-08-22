-- replay 事件仍可引用同一原始 bid fact；唯一性只约束原始事实事件，不能阻止运维重放。
ALTER TABLE public.realtime_events
    DROP CONSTRAINT realtime_events_origin_bid_id_origin_fact_index_key;

CREATE UNIQUE INDEX realtime_events_origin_bid_fact_idx
    ON public.realtime_events (origin_bid_id, origin_fact_index)
    WHERE replay_of_event_id IS NULL
      AND origin_bid_id IS NOT NULL
      AND origin_fact_index IS NOT NULL;
