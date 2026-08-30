-- MoogleMail 收件人不存在时，退信资产需要人工取回。保留既有 delivery 人工对账总量 signal，
-- 只扩展持久化 kind/identity 闭集，使 Worker 可以追加独立、可操作的退信提醒。
LOCK TABLE public.operations_alert_signals IN SHARE ROW EXCLUSIVE MODE;

ALTER TABLE public.operations_alert_signals
    DROP CONSTRAINT operations_alert_signals_kind_check,
    DROP CONSTRAINT operations_alert_signals_identity_check,
    ADD CONSTRAINT operations_alert_signals_kind_check CHECK (
        kind IN (
            'service_missing', 'service_degraded', 'audit_failure', 'source_unavailable',
            'delivery_pending', 'delivery_returned_mail', 'delivery_manual_reconcile',
            'fulfillment_backlog', 'fulfillment_failed', 'fulfillment_manual_reconcile',
            'recovery_payment_overdue', 'recovery_relist_pending', 'recovery_failed',
            'recovery_manual_reconcile', 'balance_low', 'evidence_stale'
        )
    ),
    ADD CONSTRAINT operations_alert_signals_identity_check CHECK (
        (kind IN ('service_missing', 'service_degraded')
            AND source = 'process' AND scope IN ('public', 'admin', 'worker'))
        OR (kind = 'audit_failure' AND source = 'process' AND scope = 'audit')
        OR (kind = 'source_unavailable' AND source = scope)
        OR (kind IN (
                'delivery_pending', 'delivery_returned_mail', 'delivery_manual_reconcile'
            ) AND source = 'delivery' AND scope = 'delivery')
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
