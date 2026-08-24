-- 公开发现流是 auction.created 的持久化派生视图，不是第二份拍卖事实。
-- 迁移先为已有拍卖创建流补齐别名，后续创建事务会在同一事务内继续追加。
INSERT INTO public.realtime_streams (stream_key)
VALUES ('auction:discovery')
ON CONFLICT (stream_key) DO NOTHING;

WITH source_events AS (
    SELECT
        source_event.event_id,
        source_event.kind,
        source_event.schema_version,
        source_event.public_payload,
        source_event.state_version,
        source_event.origin_command_id,
        source_event.created_at
    FROM public.realtime_events AS source_event
    WHERE source_event.stream_key LIKE 'auction:%'
      AND source_event.stream_key <> 'auction:discovery'
      AND source_event.kind = 'auction.created'
      AND source_event.schema_version = 1
      AND NOT EXISTS (
          SELECT 1
          FROM public.realtime_events AS discovery_event
          WHERE discovery_event.stream_key = 'auction:discovery'
            AND discovery_event.kind = source_event.kind
            AND discovery_event.schema_version = source_event.schema_version
            AND discovery_event.origin_command_id IS NOT DISTINCT FROM source_event.origin_command_id
            AND discovery_event.public_payload = source_event.public_payload
      )
), numbered_events AS (
    SELECT
        source_events.*,
        row_number() OVER (ORDER BY created_at, event_id)
            + COALESCE(
                (
                    SELECT MAX(stream_sequence)
                    FROM public.realtime_events
                    WHERE stream_key = 'auction:discovery'
                ),
                0
            ) AS next_discovery_sequence
    FROM source_events
)
INSERT INTO public.realtime_events (
    event_id,
    stream_key,
    stream_sequence,
    kind,
    schema_version,
    public_payload,
    state_version,
    origin_command_id,
    created_at
)
SELECT
    md5('auction:discovery:' || event_id::text)::uuid,
    'auction:discovery',
    next_discovery_sequence,
    kind,
    schema_version,
    public_payload,
    state_version,
    origin_command_id,
    created_at
FROM numbered_events;

UPDATE public.realtime_streams
SET next_sequence = COALESCE(
        (
            SELECT MAX(stream_sequence) + 1
            FROM public.realtime_events
            WHERE stream_key = 'auction:discovery'
        ),
        1
    ),
    updated_at = CURRENT_TIMESTAMP
WHERE stream_key = 'auction:discovery';
