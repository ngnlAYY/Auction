-- operations alert 批次是持久化重试状态机。锁定四张关联表后统一 fresh Rust schema 与
-- Auction-BF v28 接管 schema 的约束，禁止两套数据库对同一 Worker 写入给出不同结果。
LOCK TABLE
    public.deliveries,
    public.operations_alert_signals,
    public.operations_alert_batches,
    public.operations_alert_batch_items
IN SHARE ROW EXCLUSIVE MODE;

-- 压缩迁移遗漏了旧版已有的 signal identity 与时间关系。先移除来源不同、名称不稳定的
-- inline CHECK，再用显式名称建立当前应用闭集；已有非法事实会在 ADD CONSTRAINT 时失败关闭。
DO $$
DECLARE
    constraint_name text;
BEGIN
    FOR constraint_name IN
        SELECT constraint.conname
        FROM pg_catalog.pg_constraint AS constraint
        WHERE constraint.conrelid = 'public.operations_alert_signals'::regclass
          AND constraint.contype = 'c'
        ORDER BY constraint.conname
    LOOP
        EXECUTE format(
            'ALTER TABLE public.operations_alert_signals DROP CONSTRAINT %I',
            constraint_name
        );
    END LOOP;
END
$$;

ALTER TABLE public.operations_alert_signals
    ADD CONSTRAINT operations_alert_signals_fingerprint_check
        CHECK (octet_length(fingerprint) = 32),
    ADD CONSTRAINT operations_alert_signals_kind_check CHECK (
        kind IN (
            'service_missing', 'service_degraded', 'audit_failure', 'source_unavailable',
            'delivery_pending', 'delivery_manual_reconcile', 'fulfillment_backlog',
            'fulfillment_failed', 'fulfillment_manual_reconcile', 'recovery_payment_overdue',
            'recovery_relist_pending', 'recovery_failed', 'recovery_manual_reconcile',
            'balance_low', 'evidence_stale'
        )
    ),
    ADD CONSTRAINT operations_alert_signals_scope_check CHECK (
        scope IN (
            'public', 'admin', 'worker', 'audit', 'process', 'delivery',
            'auction', 'fulfillment', 'recovery', 'main', 'isekai'
        )
    ),
    ADD CONSTRAINT operations_alert_signals_source_check
        CHECK (source IN ('process', 'delivery', 'fulfillment', 'recovery')),
    ADD CONSTRAINT operations_alert_signals_count_check CHECK (count_value >= 0),
    ADD CONSTRAINT operations_alert_signals_age_check CHECK (age_seconds >= 0),
    ADD CONSTRAINT operations_alert_signals_version_check CHECK (state_version > 0),
    ADD CONSTRAINT operations_alert_signals_seen_order_check
        CHECK (last_seen_at >= first_seen_at),
    ADD CONSTRAINT operations_alert_signals_resolved_order_check
        CHECK (resolved_at IS NULL OR resolved_at >= first_seen_at),
    ADD CONSTRAINT operations_alert_signals_notified_order_check
        CHECK (last_notified_at IS NULL OR last_notified_at >= first_seen_at),
    ADD CONSTRAINT operations_alert_signals_next_notify_order_check
        CHECK (next_notify_at IS NULL OR next_notify_at >= first_seen_at),
    ADD CONSTRAINT operations_alert_signals_active_shape_check CHECK (
        (active AND resolved_at IS NULL AND next_notify_at IS NOT NULL)
        OR (NOT active AND resolved_at IS NOT NULL AND next_notify_at IS NULL)
    ),
    ADD CONSTRAINT operations_alert_signals_notification_order_check CHECK (
        last_notified_at IS NULL
        OR next_notify_at IS NULL
        OR next_notify_at >= last_notified_at
    ),
    ADD CONSTRAINT operations_alert_signals_identity_check CHECK (
        (kind IN ('service_missing', 'service_degraded')
            AND source = 'process' AND scope IN ('public', 'admin', 'worker'))
        OR (kind = 'audit_failure' AND source = 'process' AND scope = 'audit')
        OR (kind = 'source_unavailable' AND source = scope)
        OR (kind IN ('delivery_pending', 'delivery_manual_reconcile')
            AND source = 'delivery' AND scope = 'delivery')
        OR (kind IN (
                'fulfillment_backlog', 'fulfillment_failed',
                'fulfillment_manual_reconcile'
            ) AND source = 'fulfillment' AND scope = 'auction')
        OR (kind IN (
                'recovery_payment_overdue', 'recovery_relist_pending',
                'recovery_failed', 'recovery_manual_reconcile'
            ) AND source = 'recovery' AND scope = 'auction')
        OR (kind IN ('balance_low', 'evidence_stale')
            AND source = 'recovery' AND scope IN ('main', 'isekai'))
    );

-- 旧版在调用 delivery Create 前就冻结批次，因此历史 claim 失败可能没有 deliveries 行；Rust
-- 无法从这种批次恢复目标配置。将仍可领取的孤儿批次确定性终结，保留其历史条目并允许
-- signal 重新形成新批次。
UPDATE public.operations_alert_batches AS batch
SET status = 'failed',
    claim_token = NULL,
    claimed_at = NULL,
    claim_expires_at = NULL,
    next_attempt_at = NULL,
    delivery_error_code = 'retry_exhausted',
    updated_at = GREATEST(statement_timestamp(), batch.updated_at, batch.created_at),
    completed_at = GREATEST(statement_timestamp(), batch.updated_at, batch.created_at)
WHERE batch.status IN ('pending', 'claimed')
  AND NOT EXISTS (
      SELECT 1
      FROM public.deliveries AS delivery
      WHERE delivery.id = batch.delivery_id
  );

-- 普通退避不持久化瞬时错误；终态只保存稳定类别。先兼容当前 Rust 草案产生的旧别名，再
-- 以 delivery 的权威终态归一化投影；没有 terminal delivery 的历史终态只能来自 Create 阶段。
UPDATE public.operations_alert_batches
SET delivery_error_code = ''
WHERE status IN ('pending', 'claimed', 'enqueued', 'succeeded')
  AND delivery_error_code <> '';

UPDATE public.operations_alert_batches
SET status = 'failed',
    delivery_error_code = 'dependency_mismatch'
WHERE status IN ('failed', 'manual_reconcile')
  AND delivery_error_code IN ('dependency_mismatch', 'delivery_identity_conflict');

UPDATE public.operations_alert_batches AS batch
SET status = delivery.status,
    delivery_error_code = CASE delivery.status
        WHEN 'succeeded' THEN ''
        WHEN 'failed' THEN 'delivery_failed'
        WHEN 'manual_reconcile' THEN 'delivery_manual_reconcile'
    END
FROM public.deliveries AS delivery
WHERE batch.delivery_id = delivery.id
  AND batch.status IN ('failed', 'manual_reconcile')
  AND batch.delivery_error_code <> 'dependency_mismatch'
  AND delivery.status IN ('succeeded', 'failed', 'manual_reconcile');

UPDATE public.operations_alert_batches AS batch
SET status = 'failed',
    delivery_error_code = 'retry_exhausted'
WHERE batch.status IN ('failed', 'manual_reconcile')
  AND batch.delivery_error_code <> 'dependency_mismatch'
  AND NOT EXISTS (
      SELECT 1
      FROM public.deliveries AS delivery
      WHERE delivery.id = batch.delivery_id
        AND delivery.status IN ('succeeded', 'failed', 'manual_reconcile')
  );

-- enqueued/succeeded 理论上都发生在 delivery Create 成功之后；缺失关联无法安全修复。
DO $$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM public.operations_alert_batches AS batch
        LEFT JOIN public.deliveries AS delivery ON delivery.id = batch.delivery_id
        WHERE batch.status IN ('enqueued', 'succeeded')
          AND delivery.id IS NULL
    ) THEN
        RAISE EXCEPTION
            'operations alert enqueued/succeeded batch is missing its delivery';
    END IF;
END
$$;

-- Worker 创建批次时已经持有第一次 claim，因此 fencing/attempt 从 1 开始。pending 只表示
-- 一次失败后的持久化退避且错误码为空；失败类别仅保存在 failed/manual_reconcile 终态。
DO $$
DECLARE
    constraint_name text;
BEGIN
    FOR constraint_name IN
        SELECT constraint.conname
        FROM pg_catalog.pg_constraint AS constraint
        WHERE constraint.conrelid = 'public.operations_alert_batches'::regclass
          AND constraint.contype = 'c'
        ORDER BY constraint.conname
    LOOP
        EXECUTE format(
            'ALTER TABLE public.operations_alert_batches DROP CONSTRAINT %I',
            constraint_name
        );
    END LOOP;
END
$$;

ALTER TABLE public.operations_alert_batches
    ALTER COLUMN claim_fencing DROP DEFAULT,
    ALTER COLUMN attempt_count DROP DEFAULT,
    ADD CONSTRAINT operations_alert_batches_request_digest_check
        CHECK (octet_length(request_digest) = 32),
    ADD CONSTRAINT operations_alert_batches_status_check CHECK (
        status IN ('pending', 'claimed', 'enqueued', 'succeeded', 'failed', 'manual_reconcile')
    ),
    ADD CONSTRAINT operations_alert_batches_claim_fencing_check CHECK (claim_fencing > 0),
    ADD CONSTRAINT operations_alert_batches_attempt_count_check
        CHECK (attempt_count BETWEEN 1 AND 3),
    ADD CONSTRAINT operations_alert_batches_item_count_check
        CHECK (item_count BETWEEN 1 AND 64),
    ADD CONSTRAINT operations_alert_batches_body_digest_check
        CHECK (octet_length(body_digest) = 32),
    ADD CONSTRAINT operations_alert_batches_error_code_check CHECK (
        delivery_error_code IN (
            '', 'timeout', 'canceled', 'persistence', 'dependency_mismatch',
            'delivery_failed', 'delivery_manual_reconcile', 'retry_exhausted'
        )
    ),
    ADD CONSTRAINT operations_alert_batches_updated_order_check
        CHECK (updated_at >= created_at),
    ADD CONSTRAINT operations_alert_batches_completed_order_check
        CHECK (completed_at IS NULL OR completed_at BETWEEN created_at AND updated_at),
    ADD CONSTRAINT operations_alert_batches_claim_shape_check CHECK (
        (
            status = 'claimed'
            AND claim_token IS NOT NULL
            AND claimed_at IS NOT NULL
            AND claim_expires_at IS NOT NULL
            AND claim_expires_at > claimed_at
            AND next_attempt_at IS NULL
        )
        OR (
            status <> 'claimed'
            AND claim_token IS NULL
            AND claimed_at IS NULL
            AND claim_expires_at IS NULL
        )
    ),
    ADD CONSTRAINT operations_alert_batches_retry_shape_check
        CHECK ((status = 'pending') = (next_attempt_at IS NOT NULL)),
    ADD CONSTRAINT operations_alert_batches_terminal_shape_check CHECK (
        (status IN ('succeeded', 'failed', 'manual_reconcile'))
        = (completed_at IS NOT NULL)
    ),
    ADD CONSTRAINT operations_alert_batches_error_shape_check CHECK (
        (status IN ('failed', 'manual_reconcile')) = (delivery_error_code <> '')
    );

-- 旧 v28 的 Create 前批次允许没有 delivery。NOT VALID 使历史终态孤儿可审计地保留，同时
-- 约束迁移后的所有新写入；目标没有孤儿时立即验证成与 fresh Rust schema 相同的完整 FK。
DO $$
DECLARE
    delivery_constraint_name name;
    delivery_constraint_validated boolean;
BEGIN
    SELECT constraint.conname, constraint.convalidated
    INTO delivery_constraint_name, delivery_constraint_validated
    FROM pg_catalog.pg_constraint AS constraint
    WHERE constraint.conrelid = 'public.operations_alert_batches'::regclass
      AND constraint.contype = 'f'
      AND constraint.confrelid = 'public.deliveries'::regclass
      AND array_length(constraint.conkey, 1) = 1
      AND (
          SELECT attribute.attname
          FROM pg_catalog.pg_attribute AS attribute
          WHERE attribute.attrelid = constraint.conrelid
            AND attribute.attnum = constraint.conkey[1]
      ) = 'delivery_id'
      AND (
          SELECT attribute.attname
          FROM pg_catalog.pg_attribute AS attribute
          WHERE attribute.attrelid = constraint.confrelid
            AND attribute.attnum = constraint.confkey[1]
      ) = 'id'
    ORDER BY constraint.conname
    LIMIT 1;

    IF delivery_constraint_name IS NULL THEN
        ALTER TABLE public.operations_alert_batches
            ADD CONSTRAINT operations_alert_batches_delivery_fkey
            FOREIGN KEY (delivery_id)
            REFERENCES public.deliveries(id)
            ON DELETE RESTRICT
            NOT VALID;
        delivery_constraint_name := 'operations_alert_batches_delivery_fkey';
        delivery_constraint_validated := false;
    END IF;

    IF NOT delivery_constraint_validated
       AND NOT EXISTS (
           SELECT 1
           FROM public.operations_alert_batches AS batch
           LEFT JOIN public.deliveries AS delivery ON delivery.id = batch.delivery_id
           WHERE delivery.id IS NULL
       ) THEN
        EXECUTE format(
            'ALTER TABLE public.operations_alert_batches VALIDATE CONSTRAINT %I',
            delivery_constraint_name
        );
    END IF;
END
$$;

-- 领取查询按 fingerprint 反查未完成批次；恢复旧版已有而压缩迁移遗漏的反向索引。
CREATE INDEX IF NOT EXISTS operations_alert_batch_items_fingerprint_batch_idx
    ON public.operations_alert_batch_items (fingerprint, batch_id);
