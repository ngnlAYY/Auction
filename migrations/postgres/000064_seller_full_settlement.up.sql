-- settlement owner：只允许新策略和卖方全额金额矩阵，不改写任何历史快照、结算、义务、余额或投递。
-- 新旧程序须停稳后同版升级；v1 保留旧公式，v2 的全部买方优惠由奖池承担。
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
        AND policy_snapshot #>> '{recovery,version}' = '1'
        AND policy_snapshot #>> '{recovery,mode}' = 'remind_v1'
    );

ALTER TABLE public.auction_settlements
    DROP CONSTRAINT auction_settlements_policy_mode_check,
    DROP CONSTRAINT auction_settlements_check,
    ADD CONSTRAINT auction_settlements_policy_mode_check
        CHECK (policy_mode IN ('legacy_zero_v0', 'hold_curve_v1', 'hold_curve_v2')),
    ADD CONSTRAINT auction_settlements_check CHECK (
        (policy_version = 0 AND policy_mode = 'legacy_zero_v0')
        OR (policy_version = 1 AND policy_mode = 'hold_curve_v1')
        OR (policy_version = 2 AND policy_mode = 'hold_curve_v2' AND pool_enabled)
    );

-- 原 B+C=S+T 守恒约束保留；v2 的 C 仅为按全场预算同比缩放后的基础优惠。
-- 可为零且不超过一份名义基础折扣；精确全场权重仍由锁内领域计划和重放校验拥有。
-- metadata 没有 v2 标记的旧行仍必须使用原 seller_amount=listed_price，不能借升级改变旧价格。
ALTER TABLE public.lot_settlements
    DROP CONSTRAINT lot_settlements_check1,
    ADD CONSTRAINT lot_settlements_check1 CHECK (
        (
            end_price = 0 AND listed_price = 0 AND buyer_amount = 0
            AND seller_amount = 0 AND tax_amount = 0 AND compensation_amount = 0
        )
        OR (
            end_price > 0 AND listed_price > 0
            AND CASE WHEN adjustment_metadata ->> 'policy' = 'hold_curve_v2' THEN
                seller_amount = end_price
                AND buyer_amount > 0
                AND listed_price <= end_price
                AND (tax_amount = 0 OR compensation_amount = 0)
                AND compensation_amount::numeric <=
                    (end_price::numeric - listed_price::numeric)
            ELSE seller_amount = listed_price END
        )
    );
