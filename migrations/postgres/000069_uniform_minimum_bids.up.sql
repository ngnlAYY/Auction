-- competition owner：新场次冻结 schema v5 / bidding_version=2，统一最低价并允许代理用尽上限反超。
-- 只放宽快照形状，不改写既有 schema v4 场次、代理授权、出价或命令账本。
-- 升级前须停止全部旧角色；迁移后同版启动，避免旧程序忽略新竞价策略版本。
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
        AND (
            policy_snapshot ->> 'schema_version' = '4'
            OR (policy_snapshot ->> 'schema_version' = '5'
                AND policy_snapshot -> 'bidding_version' = '2'::jsonb)
        ) IS TRUE
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
