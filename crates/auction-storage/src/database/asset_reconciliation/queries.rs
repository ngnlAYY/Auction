//! 只读对账查询；参数固定为 world、观察到的外部装备 ID 与样本上限。

/// 应持装备：未满足的装备类义务，以及尚未结束且携带装备附件的出站投递。
/// 义务 `execution_state = 'satisfied'` 表示外部证明已成立，不再计入应持集合；
/// `failed`/`manual_reconcile` 仍可能持有实物，因此继续计入并要求人工解释。
/// 投稿分支同样排除已满足的 `withdrawn_return`/`unsold_return`：投稿行与来源引用在
/// 装备退回后仍然存在，拍卖重新进入 preview 时不能把已退回的装备重新算作应持。
pub(super) const CUSTODY_COUNTS: &str = r#"
WITH expected AS (
    SELECT e.external_equipment_id,
           'obligation_' || o.obligation_kind AS reason
    FROM public.settlement_obligations o
    JOIN public.equipment e ON e.id = o.equipment_id
    LEFT JOIN public.settlement_obligation_fulfillments f ON f.obligation_id = o.id
    WHERE e.world_id = $1
      AND o.obligation_kind IN (
          'winner_equipment_cod', 'winner_equipment_handoff',
          'unsold_return', 'withdrawn_return'
      )
      AND COALESCE(f.execution_state, 'pending') <> 'satisfied'
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
      AND COALESCE(ref ->> 'kind', 'equipment') = 'equipment'
      AND e.world_id = $1
    UNION ALL
    SELECT e.external_equipment_id,
           'preview_' || a.status AS reason
    FROM public.auction_preview_items p
    JOIN public.auctions a ON a.id = p.auction_id
    JOIN public.equipment e ON e.id = p.equipment_id
    WHERE e.world_id = $1
      AND a.status IN ('preview', 'live', 'final')
    UNION ALL
    SELECT split_part(s.source_reference, ':', 2) AS external_equipment_id,
           'submission_' || a.status AS reason
    FROM public.auction_submissions s
    JOIN public.auctions a ON a.id = s.auction_id
    WHERE a.world_id = $1
      AND a.status IN ('preview', 'live', 'final')
      AND s.source_reference ~ '^hentaiverse-equipment-v1:[0-9]+$'
      -- 与义务分支同一判定：只有 satisfied 的退回才表示装备已离开库房。
      AND NOT EXISTS (
          SELECT 1
          FROM public.settlement_obligations returned
          JOIN public.settlement_obligation_fulfillments returned_fulfillment
              ON returned_fulfillment.obligation_id = returned.id
          WHERE returned.submission_id = s.id
            AND returned.obligation_kind IN ('withdrawn_return', 'unsold_return')
            AND returned_fulfillment.execution_state = 'satisfied'
      )
), expected_ids AS (
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
"#;

/// 有界差异样本：`reason` 只来自固定前缀加数据库状态名。
pub(super) const CUSTODY_SAMPLES: &str = r#"
WITH expected AS (
    SELECT e.external_equipment_id,
           'obligation_' || o.obligation_kind AS reason
    FROM public.settlement_obligations o
    JOIN public.equipment e ON e.id = o.equipment_id
    LEFT JOIN public.settlement_obligation_fulfillments f ON f.obligation_id = o.id
    WHERE e.world_id = $1
      AND o.obligation_kind IN (
          'winner_equipment_cod', 'winner_equipment_handoff',
          'unsold_return', 'withdrawn_return'
      )
      AND COALESCE(f.execution_state, 'pending') <> 'satisfied'
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
      AND COALESCE(ref ->> 'kind', 'equipment') = 'equipment'
      AND e.world_id = $1
    UNION ALL
    SELECT e.external_equipment_id,
           'preview_' || a.status AS reason
    FROM public.auction_preview_items p
    JOIN public.auctions a ON a.id = p.auction_id
    JOIN public.equipment e ON e.id = p.equipment_id
    WHERE a.world_id = $1
      AND a.status IN ('preview', 'live', 'final')
    UNION ALL
    SELECT split_part(s.source_reference, ':', 2) AS external_equipment_id,
           'submission_' || a.status AS reason
    FROM public.auction_submissions s
    JOIN public.auctions a ON a.id = s.auction_id
    WHERE a.world_id = $1
      AND a.status IN ('preview', 'live', 'final')
      AND s.source_reference ~ '^hentaiverse-equipment-v1:[0-9]+$'
      -- 与义务分支同一判定：只有 satisfied 的退回才表示装备已离开库房。
      AND NOT EXISTS (
          SELECT 1
          FROM public.settlement_obligations returned
          JOIN public.settlement_obligation_fulfillments returned_fulfillment
              ON returned_fulfillment.obligation_id = returned.id
          WHERE returned.submission_id = s.id
            AND returned.obligation_kind IN ('withdrawn_return', 'unsold_return')
            AND returned_fulfillment.execution_state = 'satisfied'
      )
), observed AS (
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
), unexpected AS (
    SELECT o.external_equipment_id,
           CASE WHEN e.id IS NULL THEN 'unknown_equipment' ELSE 'unexpected_equipment' END AS reason
    FROM observed o
    LEFT JOIN public.equipment e
        ON e.world_id = $1 AND e.external_equipment_id = o.external_equipment_id
    WHERE false
      AND e.id IS NULL
      AND NOT EXISTS (
        SELECT 1 FROM expected x
        WHERE x.external_equipment_id = o.external_equipment_id
    )
)
(SELECT 'missing'::text AS kind, external_equipment_id, reason FROM missing
    ORDER BY external_equipment_id LIMIT $3)
UNION ALL
(SELECT 'unexpected'::text AS kind, external_equipment_id, reason FROM unexpected
    ORDER BY external_equipment_id LIMIT $3)
"#;

/// 待付卖方货款、待执行退款、待发出站邮件与最后成功余额观察。
/// 金额与计数来自义务投影；`failed`/`manual_reconcile` 义务仍然欠款，只有 `satisfied`
/// 才从应付合计中移除。现金义务按履约记录的 `dispatch_world` 归属实际支付世界：
/// 异世界场次的卖方货款与买方退款固定由 main 派发，必须计入 main 的应付合计，
/// 不能继续挂在异世界账上。`business_state = 'contingent'` 的买方退款只有 recovery
/// 显式激活为 `ready` 后才可能执行，未激活不计入应付负债。
/// `balance_age_seconds` 用数据库时钟给出余额证据年龄，供报告追加时重新判定是否过期。
pub(super) const SOLVENCY_WORLD: &str = r#"
WITH outstanding AS (
    SELECT o.obligation_kind, o.amount
    FROM public.settlement_obligations o
    LEFT JOIN public.settlement_obligation_fulfillments f ON f.obligation_id = o.id
    WHERE o.obligation_kind IN ('seller_payout', 'buyer_refund')
      AND COALESCE(f.dispatch_world, o.world_id) = $1
      AND NOT (o.obligation_kind = 'buyer_refund' AND o.business_state = 'contingent')
      AND COALESCE(f.execution_state, 'pending') <> 'satisfied'
)
SELECT
    (SELECT count(*) FROM outstanding WHERE obligation_kind = 'seller_payout')
        AS pending_seller_payout_count,
    (SELECT COALESCE(sum(amount), 0)::bigint FROM outstanding WHERE obligation_kind = 'seller_payout')
        AS pending_seller_payout_credits,
    (SELECT count(*) FROM outstanding WHERE obligation_kind = 'buyer_refund')
        AS pending_refund_count,
    (SELECT COALESCE(sum(amount), 0)::bigint FROM outstanding WHERE obligation_kind = 'buyer_refund')
        AS pending_refund_credits,
    (SELECT count(*) FROM public.deliveries d
        WHERE d.status IN ('prepared', 'pending', 'claimed', 'sending')
          AND d.payload_operation = 'send'
          AND COALESCE(d.payload_world, 'main') = $1) AS pending_outbound_mail_count,
    health.balance_credits,
    health.balance_observed_at,
    health.balance_status,
    health.evidence_status,
    EXTRACT(EPOCH FROM (clock_timestamp() - health.balance_observed_at))::bigint
        AS balance_age_seconds
FROM (SELECT 1) AS one
LEFT JOIN public.settlement_recovery_health health ON health.world_id = $1
"#;

/// 最近一份追加报告；每种实体只返回一行，缺失时为空。
pub(super) const LATEST_REPORTS: &str = r#"
SELECT DISTINCT ON (entity_type) entity_type, occurred_at, metadata
FROM public.audit_records
WHERE entity_type IN ('custody_report', 'solvency_report')
ORDER BY entity_type, occurred_at DESC, id DESC
"#;
