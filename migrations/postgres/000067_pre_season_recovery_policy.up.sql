-- settlement owner：赛季前场次冻结恢复策略 v2，付款期限 24 小时，提醒规则同 remind_v1。
-- 只放宽恢复策略版本闭集，并把 v2 与 86400 秒付款期限绑定；既有快照与 recovery 记录保持 v1 原样。
-- 新旧程序须停稳后同版升级：旧程序读取 v2 快照会按未知版本失败关闭。
ALTER TABLE public.auctions
    DROP CONSTRAINT auctions_policy_snapshot_shape_check,
    ADD CONSTRAINT auctions_policy_snapshot_shape_check CHECK (
        jsonb_typeof(policy_snapshot) = 'object'
        AND policy_snapshot ?& ARRAY[
            'schema_version', 'start_price', 'bid_increment_ppm', 'minimum_increment',
            'soft_close_window_seconds', 'soft_close_extension_seconds', 'proxy_enabled',
            'proxy_default_mode', 'proxy_slow_interval_seconds', 'proxy_max_outbid_uses',
            'final_phase_lead_seconds', 'scheduled_end_at', 'settlement', 'recovery'
        ]
        AND policy_snapshot ->> 'schema_version' = '4'
        AND policy_snapshot ->> 'proxy_default_mode' IN ('fast', 'slow')
        AND policy_snapshot ->> 'scheduled_end_at' IS NOT NULL
        AND jsonb_typeof(policy_snapshot -> 'settlement') = 'object'
        AND (
            (policy_snapshot #>> '{settlement,version}' = '0'
                AND policy_snapshot #>> '{settlement,mode}' = 'legacy_zero_v0')
            OR (policy_snapshot #>> '{settlement,version}' = '1'
                AND policy_snapshot #>> '{settlement,mode}' = 'hold_curve_v1')
            OR (policy_snapshot #>> '{settlement,version}' = '2'
                AND policy_snapshot #>> '{settlement,mode}' = 'hold_curve_v2'
                AND policy_snapshot #> '{settlement,compensation_pool_enabled}' = 'true'::jsonb)
        ) IS TRUE
        AND jsonb_typeof(policy_snapshot #> '{settlement,anchors}') = 'array'
        AND jsonb_typeof(policy_snapshot -> 'recovery') = 'object'
        AND (
            policy_snapshot #>> '{recovery,version}' = '1'
            OR (policy_snapshot #>> '{recovery,version}' = '2'
                AND policy_snapshot #>> '{recovery,payment_timeout_seconds}' = '86400')
        ) IS TRUE
        AND policy_snapshot #>> '{recovery,mode}' = 'remind_v1'
    );

ALTER TABLE public.settlement_recoveries
    DROP CONSTRAINT settlement_recoveries_policy_version_check,
    ADD CONSTRAINT settlement_recoveries_policy_version_check CHECK (policy_version IN (1, 2));
