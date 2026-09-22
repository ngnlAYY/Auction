-- 新周轮次冻结原始 IANA 本地计划；旧轮次无法证明原时区，保持 NULL 和既有 UTC 间隔。
ALTER TABLE public.scheduled_rounds
    ADD COLUMN schedule_snapshot jsonb
        CHECK (schedule_snapshot IS NULL OR jsonb_typeof(schedule_snapshot) = 'object');

-- 只修复唯一成功 Final 命令且结果身份/状态/版本均与权威拍卖相符的遗留未完成轮次。
-- 使用拍卖已经持久化的 final_phase_at，不解析或猜测命令中未知格式的时间。
WITH ledger AS (
    SELECT command_id, aggregate_id, result_payload,
           count(*) OVER (PARTITION BY aggregate_id) AS command_count
    FROM public.auction_commands
    WHERE command_kind = 'auction_final_phase' AND result_code = 'accepted'
), proven AS (
    SELECT round.id, command.command_id, auction.final_phase_at
    FROM public.scheduled_rounds AS round
    JOIN public.auctions AS auction ON auction.id = round.auction_id
    JOIN ledger AS command ON command.aggregate_id = auction.id
    WHERE round.status = 'active'
      AND round.preview_completed_at IS NOT NULL
      AND round.open_completed_at IS NOT NULL
      AND round.final_command_id IS NULL AND round.final_completed_at IS NULL
      AND auction.status IN ('final', 'settling', 'settled')
      AND auction.final_phase_at IS NOT NULL
      AND command.command_count = 1
      AND command.result_payload ->> 'command_id' = command.command_id::text
      AND command.result_payload ->> 'auction_id' = auction.id::text
      AND command.result_payload ->> 'status' = 'Final'
      AND jsonb_typeof(command.result_payload -> 'updated_at') = 'string'
      AND length(btrim(command.result_payload ->> 'updated_at')) > 0
      AND CASE WHEN jsonb_typeof(command.result_payload -> 'state_version') = 'number'
          THEN (command.result_payload ->> 'state_version')::numeric
               BETWEEN 1 AND auction.state_version
               AND trunc((command.result_payload ->> 'state_version')::numeric)
                   = (command.result_payload ->> 'state_version')::numeric
          ELSE false END
      AND (
          NOT (command.result_payload ? 'auction')
          OR (
              command.result_payload -> 'auction' ->> 'id' = auction.id::text
              AND command.result_payload -> 'auction' ->> 'status' = 'Final'
              AND command.result_payload -> 'auction' -> 'state_version'
                  = command.result_payload -> 'state_version'
              AND command.result_payload -> 'auction' -> 'updated_at'
                  = command.result_payload -> 'updated_at'
          )
      )
)
UPDATE public.scheduled_rounds AS round
SET final_command_id = proven.command_id,
    final_completed_at = proven.final_phase_at,
    lease_owner = NULL, lease_until = NULL,
    state_version = round.state_version + 1, updated_at = clock_timestamp()
FROM proven
WHERE round.id = proven.id;
