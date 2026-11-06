-- W11.1–W11.3 资产对账与偿付能力报告：追加式报告事实、告警来源与读取索引。
-- owner: auction-storage（SQL、报告查询与追加事实）、auction-application（告警来源/种类闭集）、
--        auction-integrations（Armory/余额只读观察）
--
-- 不变量：
--   1. 报告只追加到 `audit_records`；本迁移不新建可变报告表，也不改写任何既有审计、
--      告警 signal、批次、通知或确认事实。既有 append-only 触发器继续保护全部报告行。
--   2. 新增 `reconciliation` 告警来源，只承载三类信号：
--      `custody_drift`（按 world 的装备差异）、`solvency_shortfall`（按 world 的偿付缺口）、
--      `reconciliation_stale`（报告缺失或过期的 scope 为 `custody`/`solvency`）。
--      `source_unavailable` 仍要求 scope 等于来源名，因此 scope 闭集同时扩展。
--   3. 报告事实不授权任何资金或资产动作：`asset_actions_authorized` 恒为 false，
--      读取方不得据此转移余额、重发邮件或改写义务。
-- 等级：breaking。放宽 CHECK 后新程序会写入旧程序不认识的 kind/source；旧程序读取这些
--   行时按闭集解析失败，无法回退到旧版本继续扫描与展示。停机顺序与其它迁移相同。
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
            'recovery_payment_overdue', 'recovery_relist_pending', 'recovery_failed',
            'recovery_manual_reconcile', 'balance_low', 'evidence_stale',
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
                'recovery_payment_overdue', 'recovery_relist_pending',
                'recovery_failed', 'recovery_manual_reconcile'
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

-- 报告读取只需要最新一条 custody/solvency 事实；实体索引按 entity_id 过滤，无法服务按动作
-- 倒序的“最近报告”查询，因此补充只覆盖这两类实体的部分索引，不改写既有索引。
CREATE INDEX audit_records_asset_report_idx
    ON public.audit_records (entity_type, occurred_at DESC)
    WHERE entity_type IN ('custody_report', 'solvency_report');

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (95, 'breaking');
