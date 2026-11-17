-- 结算退件告警：已绑定待处理的退件案件、隔离待核对的退件案件与 Take 结果未知的退件案件各自产生
-- 运维告警；结果未知按具体原因分作用域，通知据此带出原因类别。
-- owner: auction-application（告警种类与评估）、auction-storage（只读案件水位查询）
--
-- 不变量：
--   1. 只扩展 operations_alert_signals 的 kind、scope 与 identity 闭集，不新增表或来源；新种类沿用
--      履约告警来源 source = 'fulfillment'。待处理与隔离两种的 scope 为 'auction'；
--      `settlement_return_take_unknown` 的 scope 是 Take 结果未知的原因闭集（与领域
--      `SettlementReturnTakeUnknownReason` 逐项一致），同一原因一个信号。
--   2. 数量按未结案案件计，不要求存在义务（隔离记录也计入）；告警确认或静音不改变案件，案件结案
--      后由下一轮扫描关闭；读取失败产生来源不可用，不当作零件自动关闭。
--   3. 已由退件案件接管的履约（退件转人工对账、代管或留箱终结）改按退件待办计数，不再重复计入
--      履约人工对账与失败水位；这是读侧口径，不改写任何业务事实。
--
-- 等级：breaking。旧程序无法解析新增 kind，读取该行时闭集解析失败。回退须按 RECOVERY.md 处理含
-- breaking 迁移的版本。
LOCK TABLE public.operations_alert_signals IN SHARE ROW EXCLUSIVE MODE;

ALTER TABLE public.operations_alert_signals
    DROP CONSTRAINT operations_alert_signals_kind_check,
    DROP CONSTRAINT operations_alert_signals_scope_check,
    DROP CONSTRAINT operations_alert_signals_identity_check,
    ADD CONSTRAINT operations_alert_signals_kind_check CHECK (
        kind IN (
            'service_missing', 'service_degraded', 'audit_failure', 'source_unavailable',
            'delivery_pending', 'delivery_returned_mail', 'delivery_manual_reconcile',
            'fulfillment_backlog', 'fulfillment_failed', 'fulfillment_manual_reconcile',
            'fulfillment_collection_overdue',
            'settlement_return_pending', 'settlement_return_quarantined',
            'settlement_return_take_unknown',
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
    ADD CONSTRAINT operations_alert_signals_scope_check CHECK (
        scope IN (
            'public', 'admin', 'worker', 'audit', 'process', 'delivery',
            'auction', 'fulfillment', 'recovery', 'main', 'isekai',
            'mail', 'credential', 'lifecycle', 'forum', 'notification',
            'mail_cookie', 'external_api', 'backup', 'wal',
            'custody', 'solvency', 'reconciliation',
            'assets_claimable', 'assets_unclaimable', 'assets_changed', 'mail_missing',
            'receipt_unproven', 'session_changed', 'take_unverifiable', 'disposition_conflict',
            'awaiting_observation'
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
                'fulfillment_manual_reconcile', 'fulfillment_collection_overdue',
                'settlement_return_pending', 'settlement_return_quarantined'
            ) AND source = 'fulfillment' AND scope = 'auction')
        OR (kind = 'settlement_return_take_unknown' AND source = 'fulfillment'
            AND scope IN (
                'assets_claimable', 'assets_unclaimable', 'assets_changed', 'mail_missing',
                'receipt_unproven', 'session_changed', 'take_unverifiable',
                'disposition_conflict', 'awaiting_observation'
            ))
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
    gen_random_uuid(), 'system', 'migration-000116', 'system.migration', 'migration',
    '000116', 'success', 'system',
    jsonb_build_object(
        'summary', 'operations alert settlement return pending and quarantined kinds',
        'compatibility', 'breaking')
);

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (116, 'breaking');
