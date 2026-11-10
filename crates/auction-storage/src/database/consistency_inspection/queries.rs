//! 六类不变量 SQL；每项完整计数，只返回固定上限的标识/原因样本。

pub(super) const CHECKS: &[(&str, &str)] = &[
    ("lot_bid_projection", LOT_BIDS),
    ("compensation_pool_ledger", COMPENSATION),
    ("obligation_chain", OBLIGATIONS),
    ("delivery_obligation_state", DELIVERIES),
    ("realtime_sequence", REALTIME),
    ("expired_lot_slow_proxy", SLOW_PROXY),
];

// 结束时间可能被提前结算/Final 阶段合法缩短，不与旧 bid 的结束时间强行相等。
const LOT_BIDS: &str = r#"
WITH violations AS (
    SELECT lot.id::text AS entity_key, 'latest_bid_projection_mismatch'::text AS reason
    FROM public.lots lot
    LEFT JOIN LATERAL (
        SELECT bidder_user_id, amount, mechanism, accepted_state_version
        FROM public.bids WHERE lot_id = lot.id
        ORDER BY accepted_state_version DESC LIMIT 1
    ) bid ON true
    WHERE lot.current_price IS DISTINCT FROM bid.amount
       OR lot.leader_user_id IS DISTINCT FROM bid.bidder_user_id
       OR lot.leader_proxy_last IS DISTINCT FROM COALESCE(bid.mechanism = 'proxy', false)
       OR lot.state_version < bid.accepted_state_version
)
SELECT *, count(*) OVER () AS violation_count FROM violations
ORDER BY entity_key, reason LIMIT $1
"#;

// opening 是期初余额而非增量；closing 的 amount 固定为 0。
// 全局池从 000071 合并审计起算，不能把旧世界池的同版本流水混入全局链。
const COMPENSATION: &str = r#"
WITH baseline AS (
    SELECT occurred_at, (metadata->>'balance')::numeric AS balance,
           (metadata->>'state_version')::bigint AS version
    FROM public.audit_records
    WHERE action = 'auction.compensation.merge' AND entity_id = 'global'
), ordered_ledger AS (
    SELECT ledger.*, row_number() OVER w AS expected_sequence,
           lag(balance_after) OVER w AS previous_balance,
           max(entry_sequence) OVER (PARTITION BY auction_id) AS last_sequence
    FROM public.compensation_ledger ledger
    WINDOW w AS (PARTITION BY auction_id ORDER BY entry_sequence)
), movements AS (
    SELECT settlement.auction_id::text AS entity_key,
           settlement.opening_pool_version AS opening_version,
           settlement.closing_pool_version AS closing_version,
           settlement.opening_pool_balance::numeric AS opening_balance,
           settlement.closing_pool_balance::numeric AS closing_balance,
           COALESCE((SELECT sum(amount::numeric) FROM public.compensation_ledger ledger
                     WHERE ledger.auction_id = settlement.auction_id
                       AND movement_kind IN ('tax', 'compensation')), 0) AS delta
    FROM public.auction_settlements settlement CROSS JOIN baseline
    WHERE settlement.pool_enabled AND settlement.settled_at >= baseline.occurred_at
    UNION ALL
    SELECT reversal.id::text, opening_pool_version, closing_pool_version,
           opening_balance::numeric, closing_balance::numeric,
           compensation_amount::numeric - tax_reversed::numeric
    FROM public.compensation_pool_reversals reversal CROSS JOIN baseline
    WHERE reversal.created_at >= baseline.occurred_at
), ordered_movements AS (
    SELECT movement.*,
           lag(closing_balance) OVER w AS previous_balance,
           lag(closing_version) OVER w AS previous_version
    FROM movements movement WINDOW w AS (ORDER BY opening_version, entity_key)
), violations AS (
    SELECT ledger.id::text AS entity_key, 'ledger_sequence_or_balance_mismatch'::text AS reason
    FROM ordered_ledger ledger
    WHERE entry_sequence <> expected_sequence
       OR (entry_sequence = 1 AND (movement_kind <> 'opening' OR amount <> balance_after))
       OR (entry_sequence > 1 AND (movement_kind = 'opening'
           OR balance_after::numeric <> previous_balance::numeric + amount::numeric))
       OR (entry_sequence = last_sequence AND (movement_kind <> 'closing' OR amount <> 0))
       OR (entry_sequence <> last_sequence AND movement_kind = 'closing')
    UNION ALL
    SELECT DISTINCT ledger.auction_id::text, 'ledger_settlement_missing'
    FROM public.compensation_ledger ledger
    LEFT JOIN public.auction_settlements settlement ON settlement.auction_id = ledger.auction_id
    WHERE settlement.auction_id IS NULL
    UNION ALL
    SELECT settlement.auction_id::text, 'settlement_ledger_totals_mismatch'
    FROM public.auction_settlements settlement
    LEFT JOIN LATERAL (
        SELECT count(*) AS entries,
               max(balance_after) FILTER (WHERE movement_kind = 'opening') AS opening,
               max(balance_after) FILTER (WHERE movement_kind = 'closing') AS closing,
               COALESCE(sum(amount::numeric) FILTER (WHERE movement_kind = 'tax'), 0) AS tax,
               -COALESCE(sum(amount::numeric) FILTER (WHERE movement_kind = 'compensation'), 0) AS compensation
        FROM public.compensation_ledger WHERE auction_id = settlement.auction_id
    ) ledger ON true
    WHERE ledger.entries < 2 OR ledger.opening IS DISTINCT FROM settlement.opening_pool_balance
       OR ledger.closing IS DISTINCT FROM settlement.closing_pool_balance
       OR ledger.tax <> CASE WHEN settlement.pool_enabled THEN settlement.tax_total ELSE 0 END
       OR ledger.compensation <> CASE WHEN settlement.pool_enabled THEN settlement.compensation_total ELSE 0 END
    UNION ALL
    SELECT movement.entity_key, 'global_pool_movement_chain_mismatch'
    FROM ordered_movements movement CROSS JOIN baseline
    WHERE movement.opening_version <> COALESCE(movement.previous_version, baseline.version)
       OR movement.opening_balance <> COALESCE(movement.previous_balance, baseline.balance)
       OR movement.closing_version <> movement.opening_version + 1
       OR movement.closing_balance <> movement.opening_balance + movement.delta
    UNION ALL
    SELECT 'global', 'global_pool_balance_or_version_mismatch'
    FROM public.compensation_pool pool CROSS JOIN baseline
    WHERE pool.balance::numeric <> baseline.balance + COALESCE((SELECT sum(delta) FROM movements), 0)
       OR pool.state_version::numeric <> baseline.version::numeric + (SELECT count(*) FROM movements)
    UNION ALL
    SELECT 'global', 'global_pool_or_merge_baseline_missing_or_duplicate'
    WHERE (SELECT count(*) FROM baseline) <> 1 OR (SELECT count(*) FROM public.compensation_pool) <> 1
)
SELECT *, count(*) OVER () AS violation_count FROM violations
ORDER BY entity_key, reason LIMIT $1
"#;

// 深度 64 是检查边界：超界明确报告，不把未遍历的链误报为正常。
const OBLIGATIONS: &str = r#"
WITH RECURSIVE chain AS (
    SELECT id AS root_id, id AS current_id, prerequisite_obligation_id AS next_id,
           ARRAY[id] AS path, false AS cycle, 1 AS depth
    FROM public.settlement_obligations
    UNION ALL
    SELECT chain.root_id, parent.id, parent.prerequisite_obligation_id,
           chain.path || parent.id, parent.id = ANY(chain.path), chain.depth + 1
    FROM chain JOIN public.settlement_obligations parent ON parent.id = chain.next_id
    WHERE NOT chain.cycle AND chain.depth < 64
), violations AS (
    SELECT obligation.id::text AS entity_key, 'obligation_prerequisite_identity_mismatch'::text AS reason
    FROM public.settlement_obligations obligation
    LEFT JOIN public.settlement_obligations parent ON parent.id = obligation.prerequisite_obligation_id
    WHERE obligation.prerequisite_obligation_id IS NOT NULL
      AND (parent.id IS NULL OR parent.auction_id <> obligation.auction_id
           OR parent.lot_id <> obligation.lot_id OR parent.equipment_id <> obligation.equipment_id)
    UNION ALL
    SELECT DISTINCT root_id::text,
           CASE WHEN cycle THEN 'obligation_cycle' ELSE 'obligation_chain_depth_limit' END
    FROM chain WHERE cycle OR (depth = 64 AND next_id IS NOT NULL)
    UNION ALL
    SELECT obligation.id::text, 'obligation_fulfillment_missing'
    FROM public.settlement_obligations obligation
    LEFT JOIN public.settlement_obligation_fulfillments fulfillment ON fulfillment.obligation_id = obligation.id
    WHERE fulfillment.obligation_id IS NULL
    UNION ALL
    SELECT obligation.id::text, 'obligation_prerequisite_state_mismatch'
    FROM public.settlement_obligations obligation
    LEFT JOIN public.settlement_obligation_fulfillments parent ON parent.obligation_id = obligation.prerequisite_obligation_id
    WHERE obligation.prerequisite_obligation_id IS NOT NULL
      AND ((obligation.business_state = 'ready' AND parent.execution_state IS DISTINCT FROM 'satisfied'
            AND obligation.obligation_kind <> 'buyer_refund')
           OR (obligation.business_state = 'blocked' AND parent.execution_state = 'satisfied'))
    UNION ALL
    SELECT payment.id::text, 'payment_recovery_chain_missing'
    FROM public.settlement_obligations payment
    LEFT JOIN public.settlement_recoveries recovery ON recovery.payment_obligation_id = payment.id
    WHERE payment.obligation_kind IN ('winner_equipment_cod', 'winner_payment_carrier_cod')
      AND recovery.id IS NULL
    UNION ALL
    SELECT recovery.id::text, 'recovery_obligation_binding_mismatch'
    FROM public.settlement_recoveries recovery
    CROSS JOIN LATERAL (VALUES
        (recovery.payment_obligation_id,
         CASE WHEN recovery.world_id = 'main' THEN 'winner_equipment_cod' ELSE 'winner_payment_carrier_cod' END),
        (recovery.seller_payout_obligation_id, 'seller_payout'),
        (recovery.buyer_refund_obligation_id, 'buyer_refund'),
        (recovery.equipment_obligation_id, 'winner_equipment_handoff')
    ) expected(obligation_id, kind)
    LEFT JOIN public.settlement_obligations obligation ON obligation.id = expected.obligation_id
    WHERE (expected.kind <> 'winner_equipment_handoff' OR recovery.world_id = 'isekai')
      AND (obligation.id IS NULL OR obligation.obligation_kind <> expected.kind
           OR obligation.auction_id <> recovery.auction_id OR obligation.lot_id <> recovery.lot_id
           OR obligation.equipment_id <> recovery.equipment_id)
)
SELECT *, count(*) OVER () AS violation_count FROM violations
ORDER BY entity_key, reason LIMIT $1
"#;

// succeeded/enqueued 是合法的等待收取/对账中间态；不把发送成功误当履约已满足。
// 内部持有证明可在没有 delivery 的情况下满足义务。
const DELIVERIES: &str = r#"
WITH violations AS (
    SELECT fulfillment.obligation_id::text AS entity_key, 'delivery_obligation_state_mismatch'::text AS reason
    FROM public.settlement_obligation_fulfillments fulfillment
    JOIN public.settlement_obligations obligation ON obligation.id = fulfillment.obligation_id
    LEFT JOIN public.deliveries delivery ON delivery.id = fulfillment.delivery_id
    WHERE (fulfillment.execution_state = 'pending' AND fulfillment.delivery_id IS NOT NULL)
       OR (fulfillment.execution_state IN ('prepared', 'enqueued') AND delivery.id IS NULL)
       OR (fulfillment.execution_state = 'prepared' AND delivery.status <> 'prepared')
       OR (fulfillment.execution_state = 'enqueued' AND delivery.status = 'prepared')
       OR (fulfillment.execution_state = 'satisfied' AND fulfillment.proof_digest IS NULL)
       -- 投递成功证明才要求 delivery 成功；内部持有/损失证明可在原投递失败后满足义务。
       OR (fulfillment.execution_state = 'satisfied'
           AND fulfillment.proof_kind = 'mooglemail_sent_collected'
           AND (delivery.id IS NULL OR delivery.status <> 'succeeded'))
       OR (fulfillment.delivery_id IS NOT NULL AND (delivery.origin_kind <> 'settlement'
           OR delivery.origin_id <> fulfillment.obligation_id::text))
       OR (obligation.business_state <> 'ready' AND fulfillment.execution_state IN ('prepared', 'enqueued', 'satisfied'))
)
SELECT *, count(*) OVER () AS violation_count FROM violations
ORDER BY entity_key, reason LIMIT $1
"#;

// 当前保留策略尚未授权删除 realtime 前缀，因此从 1 开始逐流连续；同时核对分配器尾部。
// 竞价事件允许在提交后短暂处于“未分配序号”（stream_sequence IS NULL）。这类行不进入
// 回放，也不参与连续性与尾部核对；真正的问题是它们长期滞留，因此单独检查待定积压年龄。
const REALTIME: &str = r#"
WITH violations AS (
    SELECT stream.stream_key AS entity_key, 'realtime_sequence_gap_or_tail_mismatch'::text AS reason
    FROM public.realtime_streams stream
    LEFT JOIN LATERAL (
        SELECT count(*)::numeric AS count, min(stream_sequence) AS first,
               max(stream_sequence) AS last
        FROM public.realtime_events
        WHERE stream_key = stream.stream_key AND stream_sequence IS NOT NULL
    ) events ON true
    WHERE (events.count = 0 AND stream.next_sequence <> 1)
       OR (events.count > 0 AND (events.first <> 1 OR events.last::numeric <> events.count
           OR stream.next_sequence::numeric <> events.last::numeric + 1))
    UNION ALL
    SELECT pending.stream_key, 'realtime_unsequenced_backlog'::text
    FROM public.realtime_events AS pending
    WHERE pending.stream_sequence IS NULL
      AND pending.created_at < transaction_timestamp() - interval '5 minutes'
    GROUP BY pending.stream_key
    UNION ALL
    SELECT offsets.consumer_key || ':' || offsets.stream_key, 'consumer_offset_ahead_of_stream'
    FROM public.realtime_consumer_offsets offsets
    JOIN public.realtime_streams stream ON stream.stream_key = offsets.stream_key
    WHERE offsets.last_sequence >= stream.next_sequence
)
SELECT *, count(*) OVER () AS violation_count FROM violations
ORDER BY entity_key, reason LIMIT $1
"#;

const SLOW_PROXY: &str = r#"
WITH violations AS (
    SELECT id::text AS entity_key, 'closed_or_expired_lot_has_slow_proxy_schedule'::text AS reason
    FROM public.lots
    WHERE (status <> 'live' OR effective_end_at <= transaction_timestamp())
      AND (slow_authorization_id IS NOT NULL OR slow_due_at IS NOT NULL)
)
SELECT *, count(*) OVER () AS violation_count FROM violations
ORDER BY entity_key, reason LIMIT $1
"#;
