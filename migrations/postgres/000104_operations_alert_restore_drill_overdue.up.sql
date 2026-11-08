-- 决策 31 / W10.3：月度恢复演练逾期的主机侧告警种类。
-- owner: auction-application（告警种类、阈值与评估）、auction-cli（主机证据采集）
--
-- 不变量：
--   1. 只扩展 operations_alert_signals 的 kind 与 identity 闭集；不新增告警来源，新种类归属
--      既有 `backup` 来源与 `backup` scope。
--   2. 演练证据只来自运维留存的成功恢复回执与通过的迁移演练回执；本迁移不记录、不推断演练。
--   3. 新值只影响告警观察，不改变备份、恢复、竞价、结算、投递或资金事实。
--
-- 等级：breaking。旧程序无法解析新增 kind，读取该行时闭集解析失败；回退须按
-- RECOVERY.md 处理含 breaking 迁移的版本。
LOCK TABLE public.operations_alert_signals IN SHARE ROW EXCLUSIVE MODE;

ALTER TABLE public.operations_alert_signals
    DROP CONSTRAINT operations_alert_signals_kind_check,
    DROP CONSTRAINT operations_alert_signals_identity_check,
    ADD CONSTRAINT operations_alert_signals_kind_check CHECK (
        kind IN (
            'service_missing', 'service_degraded', 'audit_failure', 'source_unavailable',
            'delivery_pending', 'delivery_returned_mail', 'delivery_manual_reconcile',
            'fulfillment_backlog', 'fulfillment_failed', 'fulfillment_manual_reconcile',
            'recovery_payment_overdue', 'recovery_payment_mail_unsent',
            'recovery_relist_pending', 'recovery_failed', 'recovery_manual_reconcile',
            'balance_low', 'evidence_stale',
            'mail_poll_stalled', 'mail_sent_page_limit_approaching',
            'credential_invalid', 'auction_phase_overdue', 'proxy_expiry_overdue',
            'queue_latency', 'forum_sync_failed', 'forum_message_failed',
            'forum_topic_page_limit_approaching', 'notification_delivery_failed',
            'notification_postage_insufficient', 'account_busy',
            'disk_capacity_low', 'backup_failed', 'restore_drill_overdue', 'wal_upload_stalled',
            'custody_drift', 'solvency_shortfall', 'reconciliation_stale',
            'season_obligations_pending'
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
        OR (kind IN ('mail_poll_stalled', 'mail_sent_page_limit_approaching')
            AND source = 'mail' AND scope = 'mail')
        OR (kind = 'credential_invalid' AND source = 'credential'
            AND scope IN ('mail_cookie', 'external_api'))
        OR (kind IN ('auction_phase_overdue', 'proxy_expiry_overdue')
            AND source = 'lifecycle' AND scope = 'auction')
        OR (kind = 'season_obligations_pending'
            AND source = 'lifecycle' AND scope = 'isekai')
        OR (kind IN (
                'forum_sync_failed', 'forum_message_failed',
                'forum_topic_page_limit_approaching'
            ) AND source = 'forum' AND scope = 'forum')
        OR (kind IN ('notification_delivery_failed', 'notification_postage_insufficient')
            AND source = 'notification' AND scope = 'notification')
        OR (kind IN ('disk_capacity_low', 'backup_failed', 'restore_drill_overdue')
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
    gen_random_uuid(), 'system', 'migration-000104', 'system.migration', 'migration',
    '000104', 'success', 'system',
    jsonb_build_object(
        'summary', 'operations alert restore drill overdue kind',
        'compatibility', 'breaking')
);

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (104, 'breaking');
