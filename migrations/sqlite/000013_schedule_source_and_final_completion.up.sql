-- Lite 与 PostgreSQL 保存相同的本地排期来源；旧 NULL 不推测时区或重写既定时间。
ALTER TABLE scheduled_rounds ADD COLUMN schedule_snapshot TEXT
    CHECK (schedule_snapshot IS NULL
        OR (json_valid(schedule_snapshot) AND json_type(schedule_snapshot) = 'object'));

-- 只根据完整匹配的唯一成功 Final 事实补齐完成标记，不重放业务动作。
WITH ledger AS (
    SELECT command_id, aggregate_id,
           count(*) OVER (PARTITION BY aggregate_id) AS command_count,
           CASE WHEN json_valid(result_payload) THEN result_payload ELSE '{}' END AS payload
    FROM auction_commands
    WHERE command_kind = 'auction_final_phase' AND result_code = 'accepted'
), proven AS (
    SELECT round.id, command.command_id, auction.final_phase_at
    FROM scheduled_rounds AS round
    JOIN auctions AS auction ON auction.id = round.auction_id
    JOIN ledger AS command ON command.aggregate_id = auction.id
    WHERE round.status = 'active'
      AND round.preview_completed_at IS NOT NULL
      AND round.open_completed_at IS NOT NULL
      AND round.final_command_id IS NULL AND round.final_completed_at IS NULL
      AND auction.status IN ('final', 'settling', 'settled')
      AND auction.final_phase_at IS NOT NULL
      AND command.command_count = 1
      AND lower(replace(json_extract(command.payload, '$.command_id'), '-', ''))
          = lower(hex(command.command_id))
      AND lower(replace(json_extract(command.payload, '$.auction_id'), '-', ''))
          = lower(hex(auction.id))
      AND json_extract(command.payload, '$.status') = 'Final'
      AND json_type(command.payload, '$.updated_at') = 'text'
      AND length(trim(json_extract(command.payload, '$.updated_at'))) > 0
      AND json_type(command.payload, '$.state_version') = 'integer'
      AND json_extract(command.payload, '$.state_version') BETWEEN 1 AND auction.state_version
      AND (
          json_type(command.payload, '$.auction') IS NULL
          OR (
              lower(replace(json_extract(command.payload, '$.auction.id'), '-', ''))
                  = lower(hex(auction.id))
              AND json_extract(command.payload, '$.auction.status') = 'Final'
              AND json_extract(command.payload, '$.auction.state_version')
                  = json_extract(command.payload, '$.state_version')
              AND json_extract(command.payload, '$.auction.updated_at')
                  = json_extract(command.payload, '$.updated_at')
          )
      )
)
UPDATE scheduled_rounds AS round
SET final_command_id = proven.command_id,
    final_completed_at = proven.final_phase_at,
    lease_owner = NULL, lease_until = NULL,
    state_version = round.state_version + 1, updated_at = CURRENT_TIMESTAMP
FROM proven
WHERE round.id = proven.id;
