//! W11.1–W11.3 只读对账查询；参数固定为 world、观察到的外部装备 ID 与样本上限。

/// 应持装备：未满足的装备类义务，以及尚未结束且携带装备附件的出站投递。
/// 义务 `execution_state = 'satisfied'` 表示外部证明已成立，不再计入应持集合；
/// `failed`/`manual_reconcile` 仍可能持有实物，因此继续计入并要求人工解释。
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
        )) AS unexpected_count
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
    WHERE e.id IS NULL
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
/// 才从应付合计中移除。
pub(super) const SOLVENCY_WORLD: &str = r#"
WITH outstanding AS (
    SELECT o.obligation_kind, o.amount
    FROM public.settlement_obligations o
    LEFT JOIN public.settlement_obligation_fulfillments f ON f.obligation_id = o.id
    WHERE o.world_id = $1
      AND o.obligation_kind IN ('seller_payout', 'buyer_refund')
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
    health.evidence_status
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
