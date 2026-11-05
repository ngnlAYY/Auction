-- W5.5 补齐运维告警来源（决策 66 分级不变）：收件轮询停滞、凭据失效、生命周期与慢速代理
--                           逾期、队列延迟、论坛发布与私信失败、非结算通知失败（含邮资不足）、
--                           外部账号持续忙。
-- owner: auction-application（告警来源闭集）、auction-storage（SQL 与持久化）
-- 不变量：
--   1. kind/scope/source/identity 仍是数据库层闭集；未登记的取值在写入时失败关闭，
--      应用层 `OperationsAlertKind::ALL`/`OperationsAlertSource::ALL` 与本文件逐项对应。
--   2. 只放宽 CHECK，不改写任何既有行、列或批次状态；已存在的 signal、批次与通知证据保持原样。
--   3. 新来源各自绑定唯一 scope，`source_unavailable` 仍要求 scope 等于来源名称，因此新增
--      来源名本身必须出现在 scope 闭集中。
--   4. 新增来源复用既有表：`mail_poll_state`、`credential_keys`、`auctions`、`lots`、
--      `auction_forum_sync`、`lot_watch_pm_deliveries`、`deliveries`、
--      `settlement_recovery_health`；本迁移不新建表，因此运行账号授权面不变。
-- 等级：breaking。放宽 CHECK 后新程序会写入旧程序不认识的新 kind/source；旧程序读取这些
--   行时按闭集解析失败，无法回退到旧版本继续扫描。停机顺序：与其它迁移相同，在停机门禁下
--   先停止 Worker 再迁移，避免旧 Worker 与新闭集同时写入。
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
            'notification_postage_insufficient', 'account_busy'
        )
    ),
    ADD CONSTRAINT operations_alert_signals_scope_check CHECK (
        scope IN (
            'public', 'admin', 'worker', 'audit', 'process', 'delivery',
            'auction', 'fulfillment', 'recovery', 'main', 'isekai',
            'mail', 'credential', 'lifecycle', 'forum', 'notification',
            'mail_cookie', 'external_api'
        )
    ),
    ADD CONSTRAINT operations_alert_signals_source_check CHECK (
        source IN (
            'process', 'delivery', 'fulfillment', 'recovery',
            'mail', 'credential', 'lifecycle', 'forum', 'notification'
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
    );

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (85, 'breaking');
