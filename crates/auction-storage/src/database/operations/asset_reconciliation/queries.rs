//! 只读对账查询；参数固定为 world、观察到的外部装备 ID 与样本上限。

/// 随装备履约的义务种类：COD、异世界交付与流拍/撤拍退回。
macro_rules! equipment_obligation_kinds {
    () => {
        "'winner_equipment_cod', 'winner_equipment_handoff', 'unsold_return', 'withdrawn_return'"
    };
}

/// 应持装备集合的查询共用同一 CTE；`$1` 是 world、`$2` 是观察到的外部装备 ID。
/// 预览分支的世界口径由调用方传入：计数按装备所属世界，样本按场次世界（保持既有行为）。
/// 装备义务一经成功寄出（投递 `succeeded`），装备就随邮件离开了 Armory：不论履约仍在等待
/// 领取（含已寄出待付款的 COD），还是寄出后因领取证据不符等转为 `manual_reconcile`/`failed`，
/// 义务分支与投稿分支都不再把它算作应持，由 `CUSTODY_IN_TRANSIT` 单列为在途或在途待核对。
/// 资产位置只看这条持久寄出事实，不随可变的执行态重新计入应持。
///
/// 成交撤销后装备的去向由恢复记录决定：被恢复收口精确终结的原义务（见
/// `recovery_closed_fulfillment_sql!`）不再计入，改由恢复分支按去向计入——已收回待重列
/// （`recovery_relist_pending`）与已重列到尚未预展的草稿场次（`relisted_draft`）；重列场次进入
/// 预展后由预览分支计入，退回卖家由新的 `withdrawn_return` 义务计入。同一外部装备只算一次。
///
/// 结算退件：原派发的装备经精确领回证明回到 Bot 后（`settlement_return_takes`），代管中的原义务
/// 重新计入应持（原因 `custody_<义务种类>`）；授权补寄后履约换成新派发，按普通未寄出义务计入，
/// 新派发寄出后才离开应持。
macro_rules! expected_equipment_cte {
    ($preview_world:literal) => {
        concat!(
            r#"
WITH expected AS (
    SELECT e.external_equipment_id,
           CASE WHEN taken.return_mail_id IS NOT NULL THEN 'custody_' ELSE 'obligation_' END
               || o.obligation_kind AS reason
    FROM public.settlement_obligations o
    JOIN public.equipment e ON e.id = o.equipment_id
    LEFT JOIN public.settlement_obligation_fulfillments f ON f.obligation_id = o.id
    LEFT JOIN public.deliveries d ON d.id = f.delivery_id
    LEFT JOIN public.settlement_return_takes taken ON taken.original_delivery_id = d.id
    WHERE e.world_id = $1
      AND o.obligation_kind IN ("#,
            equipment_obligation_kinds!(),
            r#")
      AND COALESCE(f.execution_state, 'pending') <> 'satisfied'
      AND (taken.return_mail_id IS NOT NULL OR NOT COALESCE("#,
            crate::database::settlement::fulfillment::sent_unsatisfied_sql!("f", "d"),
            r#", false))
      AND NOT COALESCE("#,
            crate::database::settlement::fulfillment::recovery_closed_fulfillment_sql!("o", "f"),
            r#", false)
    UNION ALL
    SELECT e.external_equipment_id,
           CASE WHEN r.relisted_lot_id IS NULL THEN 'recovery_' || r.stage
                ELSE 'relisted_' || relisted_auction.status END AS reason
    FROM public.settlement_recoveries r
    JOIN public.equipment e ON e.id = r.equipment_id
    LEFT JOIN public.lots relisted ON relisted.id = r.relisted_lot_id
    LEFT JOIN public.auctions relisted_auction ON relisted_auction.id = relisted.auction_id
    WHERE e.world_id = $1
      AND r.resolution IN ('reclaimed', 'cancelled', 'equipment_recovered')
      AND ((r.relisted_lot_id IS NULL AND r.stage = 'relist_pending')
           OR (relisted.status = 'pending' AND relisted_auction.status = 'draft'))
    UNION ALL
    SELECT e.external_equipment_id,
           'delivery_' || d.status || '_equipment' AS reason
    FROM public.deliveries d
    CROSS JOIN LATERAL jsonb_array_elements(
        CASE
            WHEN jsonb_typeof(d.payload -> 'equipment_secret_refs') = 'array'
                THEN d.payload -> 'equipment_secret_refs'
            ELSE '[]'::jsonb
        END
    ) AS ref
    JOIN public.equipment e ON e.id = CASE
        WHEN (ref ->> 'id') ~
            '^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$'
            THEN (ref ->> 'id')::uuid
    END
    WHERE d.status IN ('prepared', 'pending', 'claimed', 'sending', 'manual_reconcile')
      -- 结算生成方与 MoogleMail 传输只写入、只接受 canonical kind `auction_equipment_key`。
      -- 兼容声明：早期载荷写 `equipment` 或省略 kind，仍按装备附件识别；只在读侧兼容，
      -- 不改写任何已冻结的投递载荷与摘要。
      AND COALESCE(ref ->> 'kind', 'equipment') IN ('auction_equipment_key', 'equipment')
      AND e.world_id = $1
    UNION ALL
    SELECT e.external_equipment_id,
           'preview_' || a.status AS reason
    FROM public.auction_preview_items p
    JOIN public.auctions a ON a.id = p.auction_id
    JOIN public.equipment e ON e.id = p.equipment_id
    WHERE "#,
            $preview_world,
            r#" = $1
      AND a.status IN ('preview', 'live', 'final')
    UNION ALL
    SELECT split_part(s.source_reference, ':', 2) AS external_equipment_id,
           'submission_' || a.status AS reason
    FROM public.auction_submissions s
    JOIN public.auctions a ON a.id = s.auction_id
    WHERE a.world_id = $1
      AND a.status IN ('preview', 'live', 'final')
      AND s.source_reference ~ '^hentaiverse-equipment-v1:[0-9]+$'
      -- 与义务分支同一判定：satisfied 或已成功寄出的退回都表示装备已离开库房。
      AND NOT EXISTS (
          SELECT 1
          FROM public.settlement_obligations returned
          JOIN public.settlement_obligation_fulfillments returned_fulfillment
              ON returned_fulfillment.obligation_id = returned.id
          LEFT JOIN public.deliveries returned_delivery
              ON returned_delivery.id = returned_fulfillment.delivery_id
          WHERE returned.submission_id = s.id
            AND returned.obligation_kind IN ('withdrawn_return', 'unsold_return')
            AND (returned_fulfillment.execution_state = 'satisfied'
                 OR COALESCE("#,
            crate::database::settlement::fulfillment::sent_unsatisfied_sql!(
                "returned_fulfillment",
                "returned_delivery"
            ),
            r#", false))
      )
)"#
        )
    };
}

/// 应持装备：未满足且尚未寄出的装备类义务，以及尚未结束且携带装备附件的出站投递。
/// 义务 `execution_state = 'satisfied'` 表示外部证明已成立，不再计入应持集合；已成功寄出的
/// 义务装备在邮件里，改由在途查询单列；尚未寄出（或寄出结果未知）的 `failed`/
/// `manual_reconcile` 仍可能持有实物，因此继续计入并要求人工解释。
/// 投稿分支同样排除已满足或已寄出的 `withdrawn_return`/`unsold_return`：投稿行与来源引用在
/// 装备退回后仍然存在，拍卖重新进入 preview 时不能把已退回的装备重新算作应持。
pub(super) const CUSTODY_COUNTS: &str = concat!(
    expected_equipment_cte!("e.world_id"),
    r#", expected_ids AS (
    SELECT DISTINCT external_equipment_id FROM expected
), observed AS (
    SELECT unnest($2::text[]) AS external_equipment_id
)
SELECT
    (SELECT count(*) FROM expected_ids) AS expected_count,
    (SELECT count(*) FROM observed) AS observed_count,
    (SELECT count(*) FROM expected_ids ids
        WHERE NOT EXISTS (
            SELECT 1 FROM observed o
            WHERE o.external_equipment_id = ids.external_equipment_id
        )) AS missing_count,
    (SELECT count(*) FROM observed o
        WHERE NOT EXISTS (
            SELECT 1 FROM expected_ids ids
            WHERE ids.external_equipment_id = o.external_equipment_id
        )
        AND EXISTS (
            SELECT 1 FROM public.equipment e
            WHERE e.world_id = $1 AND e.external_equipment_id = o.external_equipment_id
        )) AS stock_count,
    (SELECT count(*) FROM observed o
        WHERE NOT EXISTS (
            SELECT 1 FROM expected_ids ids
            WHERE ids.external_equipment_id = o.external_equipment_id
        )
        AND NOT EXISTS (
            SELECT 1 FROM public.equipment e
            WHERE e.world_id = $1 AND e.external_equipment_id = o.external_equipment_id
        )) AS personal_count,
    0::bigint AS unexpected_count
"#,
);

/// 有界缺失样本：`reason` 只来自固定前缀加数据库状态名。未知个人装备不属于对账差异
/// （`unexpected_count` 恒为 0），因此只有缺失一类样本。
pub(super) const CUSTODY_SAMPLES: &str = concat!(
    expected_equipment_cte!("a.world_id"),
    r#", observed AS (
    SELECT unnest($2::text[]) AS external_equipment_id
), missing AS (
    SELECT ids.external_equipment_id, (array_agg(expected.reason ORDER BY expected.reason))[1] AS reason
    FROM (SELECT DISTINCT external_equipment_id FROM expected) ids
    JOIN expected ON expected.external_equipment_id = ids.external_equipment_id
    WHERE NOT EXISTS (
        SELECT 1 FROM observed o
        WHERE o.external_equipment_id = ids.external_equipment_id
    )
    GROUP BY ids.external_equipment_id
)
SELECT external_equipment_id, reason FROM missing
ORDER BY external_equipment_id LIMIT $3
"#,
);

/// 在途装备按类别与原因计数：已成功寄出、义务尚未满足的装备义务，排除仍因其他来源应持的
/// 装备（仍按应持核对）、本轮已在 Armory 观察到的装备（不在途），以及原投递当前 fence 上已有
/// 恢复来源权威处置的义务（装备已由恢复收回，按恢复去向计入应持，见
/// `recovery_disposed_delivery_sql!`）。履约仍在等待领取的是在途
/// （`review = false`，原因 `obligation_<义务种类>`）；寄出后转 `manual_reconcile`/`failed` 的是
/// 在途待核对（`review = true`，原因 `obligation_<义务种类>_<履约状态>`）。一件装备只计一次：
/// 同时有两类义务时按待核对计（需要人工解释），同一类别内取最小原因；按装备所属世界归属，与
/// 计数口径一致。结算退件的位置与在途互斥：已领回的原派发不再在途（改按应持或新派发计）；
/// 已绑定退件但尚未领回的，资产在 Bot 收件箱，原因记 `returned_in_inbox_<义务种类>`，Take 结果
/// 未知的记 `location_unknown_<义务种类>`，都按待核对计。
pub(super) const CUSTODY_IN_TRANSIT: &str = concat!(
    expected_equipment_cte!("e.world_id"),
    r#", expected_ids AS (
    SELECT DISTINCT external_equipment_id FROM expected
), observed AS (
    SELECT unnest($2::text[]) AS external_equipment_id
), in_transit AS (
    SELECT e.external_equipment_id,
           NOT "#,
    crate::database::settlement::fulfillment::sent_awaiting_collection_sql!("f", "d"),
    r#" AS review,
           CASE
               WHEN returned.status IS NOT NULL THEN
                   CASE WHEN returned.status = 'take_unknown' THEN 'location_unknown_'
                        ELSE 'returned_in_inbox_' END || o.obligation_kind
               ELSE 'obligation_' || o.obligation_kind || CASE WHEN "#,
    crate::database::settlement::fulfillment::sent_awaiting_collection_sql!("f", "d"),
    r#" THEN '' ELSE '_' || f.execution_state END
           END AS reason
    FROM public.settlement_obligations o
    JOIN public.equipment e ON e.id = o.equipment_id
    JOIN public.settlement_obligation_fulfillments f ON f.obligation_id = o.id
    JOIN public.deliveries d ON d.id = f.delivery_id
    LEFT JOIN public.settlement_return_cases returned ON returned.delivery_id = d.id
    WHERE e.world_id = $1
      AND NOT EXISTS (
          SELECT 1 FROM public.settlement_return_takes taken
          WHERE taken.original_delivery_id = d.id
      )
      AND o.obligation_kind IN ("#,
    equipment_obligation_kinds!(),
    r#")
      AND "#,
    crate::database::settlement::fulfillment::sent_unsatisfied_sql!("f", "d"),
    r#"
      AND NOT "#,
    crate::database::settlement::fulfillment::recovery_disposed_delivery_sql!("o", "d"),
    r#"
), in_transit_ids AS (
    SELECT DISTINCT ON (t.external_equipment_id) t.external_equipment_id, t.review, t.reason
    FROM in_transit t
    WHERE NOT EXISTS (
        SELECT 1 FROM expected_ids ids
        WHERE ids.external_equipment_id = t.external_equipment_id
    )
    AND NOT EXISTS (
        SELECT 1 FROM observed o
        WHERE o.external_equipment_id = t.external_equipment_id
    )
    ORDER BY t.external_equipment_id, t.review DESC, t.reason
)
SELECT review, reason, count(*)::bigint AS item_count
FROM in_transit_ids
GROUP BY review, reason
ORDER BY review, reason
"#,
);

/// 偿付查询共用的现金义务分类；`$1` 是 world。
///
/// 金额与计数来自义务投影；`satisfied` 不再计入，其余未满足义务默认仍然欠款（含尚未寄出
/// 或寄出结果未知的 `failed`/`manual_reconcile`），只有下文按持久寄出事实与余额观察时点
/// 判定为已支出的现金才从应付移出，计为在途或在途待核对。现金义务按履约记录的
/// `dispatch_world` 归属实际支付世界：异世界场次的卖方货款与买方退款固定由 main 派发，
/// 必须计入 main 的应付合计。`business_state = 'contingent'` 的买方
/// 退款只有 recovery 显式激活为 `ready` 后才可能执行，未激活不计入应付负债。成交撤销后被恢复
/// 收口精确终结的卖方货款（见 `recovery_closed_fulfillment_sql!`）不再是负债，从应付与在途中
/// 移出；买方退款从不由这些收口终结，照常计入。
///
/// 在途只认持久事实：投递已成功寄出、资金分配与同一投递和义务绑定且已 `spent`，并且寄出时刻
/// （投递完成的持久时刻 `deliveries.completed_at`）不晚于余额观察时刻（`balance_observed_at`）。
/// 只有这样，观察到的余额才已扣除这笔钱，才能从应付移出，否则会与余额重复扣减；寄出晚于观察
/// 时，观察到的余额仍含这笔钱，继续计入应付。分配缺失、未支出、绑定其他投递或义务，以及任一
/// 时刻缺失（比较为 NULL）时，一律按应付计（保守）。寄出后履约仍在等待领取的是在途；已转
/// `manual_reconcile`/`failed` 的同样按寄出事实计在途、不重新计入应付，单列为在途待核对，
/// 原因为 `obligation_<义务种类>_<履约状态>`。
///
/// 结算退件的现金位置互斥且只计一次：已绑定但尚未领回的退件，Credits 在 Bot 收件箱而不在余额
/// 中，仍按在途待核对计，原因改记 `returned_in_inbox_<义务种类>`（Take 结果未知记
/// `location_unknown_<义务种类>`）；领回后代管的原义务（案件 `held` 且有领回证明）计入代管负债，
/// 不再计入待付；领回后授权补寄的，履约换成新派发，按普通待付计，新派发寄出后才转在途。
macro_rules! solvency_cash_cte {
    () => {
        concat!(
            r#"
WITH health AS (
    SELECT balance_credits, balance_observed_at, balance_status, evidence_status
    FROM public.settlement_recovery_health
    WHERE world_id = $1
), outstanding AS (
    SELECT o.obligation_kind, o.amount,
           COALESCE("#,
            crate::database::settlement::fulfillment::sent_unsatisfied_sql!("f", "d"),
            r#"
               AND allocation.state = 'spent'
               AND d.completed_at <= (SELECT balance_observed_at FROM health), false)
               AS spent_before_balance,
           COALESCE("#,
            crate::database::settlement::fulfillment::sent_awaiting_collection_sql!("f", "d"),
            r#", false) AS awaiting_collection,
           CASE
               WHEN returned.status IS NOT NULL AND NOT returned.taken THEN
                   CASE WHEN returned.status = 'take_unknown' THEN 'location_unknown_'
                        ELSE 'returned_in_inbox_' END || o.obligation_kind
               ELSE 'obligation_' || o.obligation_kind || '_'
                   || COALESCE(f.execution_state, 'pending')
           END AS review_reason,
           COALESCE(returned.status = 'held' AND returned.taken, false) AS custody
    FROM public.settlement_obligations o
    LEFT JOIN public.settlement_obligation_fulfillments f ON f.obligation_id = o.id
    LEFT JOIN public.deliveries d ON d.id = f.delivery_id
    LEFT JOIN public.settlement_funding_allocations allocation
        ON allocation.delivery_id = f.delivery_id AND allocation.obligation_id = o.id
    LEFT JOIN LATERAL (
        SELECT c.status,
               EXISTS (SELECT 1 FROM public.settlement_return_takes t
                       WHERE t.return_mail_id = c.return_mail_id) AS taken
        FROM public.settlement_return_cases c
        WHERE c.delivery_id = f.delivery_id
    ) AS returned ON true
    WHERE o.obligation_kind IN ('seller_payout', 'buyer_refund')
      AND COALESCE(f.dispatch_world, o.world_id) = $1
      AND NOT (o.obligation_kind = 'buyer_refund' AND o.business_state = 'contingent')
      AND COALESCE(f.execution_state, 'pending') <> 'satisfied'
      AND NOT COALESCE("#,
            crate::database::settlement::fulfillment::recovery_closed_fulfillment_sql!("o", "f"),
            r#", false)
), classified AS (
    SELECT obligation_kind, amount, review_reason, custody,
           NOT spent_before_balance AND NOT custody AS pending,
           spent_before_balance AND awaiting_collection AS in_transit,
           spent_before_balance AND NOT awaiting_collection AND NOT custody AS in_transit_review
    FROM outstanding
)"#
        )
    };
}

/// 待付卖方货款、待执行退款、在途现金、在途待核对、待发出站邮件与最后成功余额观察；分类见
/// `solvency_cash_cte!`。一个世界只展开一次分类：合计与在途待核对的按原因明细读同一份
/// `classified`，明细以按原因排序的并列数组返回。待发出站邮件按两段状态分别计数再相加，
/// 各自走投递领取与租约部分索引，不随历史顺序扫描投递表；两段状态互不重叠。
/// `balance_age_seconds` 用数据库时钟给出余额证据年龄，供报告追加时重新判定是否过期。
pub(super) const SOLVENCY_WORLD: &str = concat!(
    solvency_cash_cte!(),
    r#", review AS (
    SELECT review_reason AS reason,
           count(*)::bigint AS item_count,
           COALESCE(sum(amount), 0)::bigint AS credits
    FROM classified
    WHERE in_transit_review
    GROUP BY review_reason
)
SELECT
    (SELECT count(*) FROM classified
        WHERE obligation_kind = 'seller_payout' AND pending)
        AS pending_seller_payout_count,
    (SELECT COALESCE(sum(amount), 0)::bigint FROM classified
        WHERE obligation_kind = 'seller_payout' AND pending)
        AS pending_seller_payout_credits,
    (SELECT count(*) FROM classified
        WHERE obligation_kind = 'buyer_refund' AND pending)
        AS pending_refund_count,
    (SELECT COALESCE(sum(amount), 0)::bigint FROM classified
        WHERE obligation_kind = 'buyer_refund' AND pending)
        AS pending_refund_credits,
    (SELECT count(*) FROM classified
        WHERE obligation_kind = 'seller_payout' AND in_transit)
        AS in_transit_seller_payout_count,
    (SELECT COALESCE(sum(amount), 0)::bigint FROM classified
        WHERE obligation_kind = 'seller_payout' AND in_transit)
        AS in_transit_seller_payout_credits,
    (SELECT count(*) FROM classified
        WHERE obligation_kind = 'buyer_refund' AND in_transit)
        AS in_transit_refund_count,
    (SELECT COALESCE(sum(amount), 0)::bigint FROM classified
        WHERE obligation_kind = 'buyer_refund' AND in_transit)
        AS in_transit_refund_credits,
    (SELECT COALESCE(array_agg(reason ORDER BY reason), '{}') FROM review)
        AS in_transit_review_reasons,
    (SELECT COALESCE(array_agg(item_count ORDER BY reason), '{}') FROM review)
        AS in_transit_review_counts,
    (SELECT COALESCE(array_agg(credits ORDER BY reason), '{}') FROM review)
        AS in_transit_review_credits,
    (SELECT count(*) FROM classified WHERE custody) AS custody_count,
    (SELECT COALESCE(sum(amount), 0)::bigint FROM classified WHERE custody) AS custody_credits,
    EXISTS (
        SELECT 1
        FROM public.settlement_return_takes t
        JOIN public.settlement_obligations take_obligation ON take_obligation.id = t.obligation_id
        LEFT JOIN public.settlement_obligation_fulfillments take_fulfillment
            ON take_fulfillment.obligation_id = t.obligation_id
        WHERE t.credits > 0
          AND COALESCE(take_fulfillment.dispatch_world, take_obligation.world_id) = $1
          AND t.proven_at > COALESCE(health.balance_observed_at, '-infinity'::timestamptz)
    ) AS balance_before_take,
    (SELECT count(*) FROM public.deliveries d
        WHERE d.status IN ('prepared', 'pending', 'claimed')
          AND d.payload_operation = 'send'
          AND COALESCE(d.payload_world, 'main') = $1)
    + (SELECT count(*) FROM public.deliveries d
        WHERE d.status = 'sending'
          AND d.payload_operation = 'send'
          AND COALESCE(d.payload_world, 'main') = $1) AS pending_outbound_mail_count,
    health.balance_credits,
    health.balance_observed_at,
    health.balance_status,
    health.evidence_status,
    EXTRACT(EPOCH FROM (clock_timestamp() - health.balance_observed_at))::bigint
        AS balance_age_seconds
FROM (SELECT 1) AS one
LEFT JOIN health ON true
"#,
);

/// 最近一份追加报告；每种实体只返回一行，缺失时为空。
pub(super) const LATEST_REPORTS: &str = r#"
SELECT DISTINCT ON (entity_type) entity_type, occurred_at, metadata
FROM public.audit_records
WHERE entity_type IN ('custody_report', 'solvency_report')
ORDER BY entity_type, occurred_at DESC, id DESC
"#;
