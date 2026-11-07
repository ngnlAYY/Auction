-- W14.6 / D9：付款期限已过，但付款投递没有自动或人工发送证明时产生独立运维告警。
-- owner: auction-application（告警种类闭集与评估）、auction-storage（只读水位查询）
--
-- 不变量：
--   1. 只扩展 operations_alert_signals 的 kind 闭集；不新增表、不改写既有 signal、批次、
--      通知、确认、恢复或投递事实。
--   2. 新 signal 固定 source = 'recovery'、scope = 'auction'；来源与 scope 闭集不变。
--   3. 该告警只用于提示人工核对付款投递，不改变 payment_deadline_at、恢复阶段、资金或
--      发送结果；未知投递仍按既有 manual_reconcile 流程处理。
--   4. awaiting_payment 在付款期限后也可能暂时保留（W14.6 只延后提醒），因此数据库闭集
--      不能要求该信号同时存在 payment_overdue 行。
--
-- 等级：breaking。旧程序无法解析新增 kind，读取该行时闭集解析失败；回退须按
-- RECOVERY.md 处理含 breaking 迁移的版本。
LOCK TABLE public.operations_alert_signals IN SHARE ROW EXCLUSIVE MODE;

ALTER TABLE public.operations_alert_signals
    DROP CONSTRAINT operations_alert_signals_kind_check,
    DROP CONSTRAINT operations_alert_signals_scope_check,
    DROP CONSTRAINT operations_alert_signals_source_check,
    DROP CONSTRAINT operations_alert_signals_identity_check,
    ADD CONSTRAINT operations_alert_signals_kind_check CHECK (
        kind IN (
            'service_missing', 'service_degraded', 'audit_failure', 'source_unavailable',
            'delivery_pending', 'delivery_returned_mail', 'delivery_manual_reconcile',
            'fulfillment_backlog', 'fulfillment_failed', 'fulfillment_manual_reconcile',
            'recovery_payment_overdue', 'recovery_payment_mail_unsent',
            'recovery_relist_pending', 'recovery_failed', 'recovery_manual_reconcile',
            'balance_low', 'evidence_stale',
            'mail_poll_stalled', 'credential_invalid', 'auction_phase_overdue',
            'proxy_expiry_overdue', 'queue_latency', 'forum_sync_failed',
            'forum_message_failed', 'notification_delivery_failed',
            'notification_postage_insufficient', 'account_busy',
            'disk_capacity_low', 'backup_failed', 'wal_upload_stalled',
            'custody_drift', 'solvency_shortfall', 'reconciliation_stale'
        )
    ),
    ADD CONSTRAINT operations_alert_signals_scope_check CHECK (
        scope IN (
            'public', 'admin', 'worker', 'audit', 'process', 'delivery',
            'auction', 'fulfillment', 'recovery', 'main', 'isekai',
            'mail', 'credential', 'lifecycle', 'forum', 'notification',
            'mail_cookie', 'external_api', 'backup', 'wal',
            'custody', 'solvency'
        )
    ),
    ADD CONSTRAINT operations_alert_signals_source_check CHECK (
        source IN (
            'process', 'delivery', 'fulfillment', 'recovery',
            'mail', 'credential', 'lifecycle', 'forum', 'notification',
            'backup', 'wal', 'reconciliation'
        )
    ),
    ADD CONSTRAINT operations_alert_signals_identity_check CHECK (
        (kind IN ('service_missing', 'service_degraded')
            AND source = 'process' AND scope IN ('public', 'admin', 'worker'))
        OR (kind = 'audit_failure' AND source = 'process' AND scope = 'audit')
        OR (kind = 'source_unavailable' AND source = scope)
        OR (kind IN (
                'delivery_pending', 'delivery_returned_mail', 'delivery_manual_reconcile',
                'queue_latency'
            ) AND source = 'delivery' AND scope = 'delivery')
        OR (kind IN (
                'fulfillment_backlog', 'fulfillment_failed',
                'fulfillment_manual_reconcile'
            ) AND source = 'fulfillment' AND scope = 'auction')
        OR (kind IN (
                'recovery_payment_overdue', 'recovery_payment_mail_unsent',
                'recovery_relist_pending', 'recovery_failed', 'recovery_manual_reconcile'
            ) AND source = 'recovery' AND scope = 'auction')
        OR (kind IN ('balance_low', 'evidence_stale', 'account_busy')
            AND source = 'recovery' AND scope IN ('main', 'isekai'))
        OR (kind = 'mail_poll_stalled' AND source = 'mail' AND scope = 'mail')
        OR (kind = 'credential_invalid' AND source = 'credential'
            AND scope IN ('mail_cookie', 'external_api'))
        OR (kind IN ('auction_phase_overdue', 'proxy_expiry_overdue')
            AND source = 'lifecycle' AND scope = 'auction')
        OR (kind IN ('forum_sync_failed', 'forum_message_failed')
            AND source = 'forum' AND scope = 'forum')
        OR (kind IN ('notification_delivery_failed', 'notification_postage_insufficient')
            AND source = 'notification' AND scope = 'notification')
        OR (kind IN ('disk_capacity_low', 'backup_failed')
            AND source = 'backup' AND scope = 'backup')
        OR (kind = 'wal_upload_stalled' AND source = 'wal' AND scope = 'wal')
        OR (kind IN ('custody_drift', 'solvency_shortfall')
            AND source = 'reconciliation' AND scope IN ('main', 'isekai'))
        OR (kind = 'reconciliation_stale'
            AND source = 'reconciliation' AND scope IN ('custody', 'solvency'))
    );

INSERT INTO public.audit_records
    (id, actor_type, actor_id, action, entity_type, entity_id, result, source, metadata)
VALUES (
    gen_random_uuid(), 'system', 'migration-000099', 'system.migration', 'migration',
    '000099', 'success', 'system',
    jsonb_build_object(
        'summary', 'payment mail unsent alert kind',
        'compatibility', 'breaking')
);

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (99, 'breaking');
