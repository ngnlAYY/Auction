-- W5.5：补齐页数 80% 与换季未完成义务的运维告警来源。
-- owner: auction-application（告警种类、评估与阈值）、auction-storage（只读水位与页数持久化）
--
-- 不变量：
--   1. 只扩展 operations_alert_signals 的 kind 与 identity 闭集；不新增告警来源。
--   2. Sent 页数只记录最近一次完整枚举实际扫描的页数与当时生效上限；前缀并非完整列表，不能冒充。
--   3. 论坛页数只记录远端主题按 `st` 偏移发现的总页数与当时生效上限；本地目录固定帖页数不是该值。
--   4. 换季告警只统计 isekai 已冻结 `intake_opens_at` 前七天内的未完成义务，不从当前排期推测。
--   5. 所有新增值都只影响告警观察，不改变竞价、结算、投递、资金或冻结场次。
--
-- 等级：breaking。旧程序无法解析新增 kind，读取该行时闭集解析失败；回退须按
-- RECOVERY.md 处理含 breaking 迁移的版本。
LOCK TABLE public.mail_scan_checkpoints IN SHARE ROW EXCLUSIVE MODE;
LOCK TABLE public.auction_forum_sync IN SHARE ROW EXCLUSIVE MODE;
LOCK TABLE public.operations_alert_signals IN SHARE ROW EXCLUSIVE MODE;

ALTER TABLE public.mail_scan_checkpoints
    ADD COLUMN last_full_pages_scanned integer
        CHECK (last_full_pages_scanned IS NULL
               OR (last_full_pages_scanned >= 0 AND last_full_pages_scanned <= 1000)),
    ADD COLUMN last_full_pages_observed_at timestamptz
        CHECK (last_full_pages_observed_at IS NULL
               OR isfinite(last_full_pages_observed_at)),
    ADD COLUMN last_full_page_limit integer
        CHECK (last_full_page_limit IS NULL
               OR (last_full_page_limit >= 1 AND last_full_page_limit <= 1000)),
    ADD CONSTRAINT mail_scan_checkpoints_page_observation_complete CHECK (
        (last_full_pages_scanned IS NULL)
            = (last_full_pages_observed_at IS NULL)
        AND (last_full_pages_scanned IS NULL) = (last_full_page_limit IS NULL)
    );

ALTER TABLE public.auction_forum_sync
    ADD COLUMN remote_page_count integer
        CHECK (remote_page_count IS NULL OR remote_page_count >= 1),
    ADD COLUMN remote_page_count_observed_at timestamptz
        CHECK (remote_page_count_observed_at IS NULL
               OR isfinite(remote_page_count_observed_at)),
    ADD COLUMN remote_page_limit integer
        CHECK (remote_page_limit IS NULL
               OR (remote_page_limit >= 1 AND remote_page_limit <= 1000)),
    ADD CONSTRAINT auction_forum_sync_page_observation_complete CHECK (
        (remote_page_count IS NULL) = (remote_page_count_observed_at IS NULL)
        AND (remote_page_count IS NULL) = (remote_page_limit IS NULL)
    );

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
            'disk_capacity_low', 'backup_failed', 'wal_upload_stalled',
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
    gen_random_uuid(), 'system', 'migration-000101', 'system.migration', 'migration',
    '000101', 'success', 'system',
    jsonb_build_object(
        'summary', 'operations alert page and season sources',
        'compatibility', 'breaking')
);

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (101, 'breaking');
