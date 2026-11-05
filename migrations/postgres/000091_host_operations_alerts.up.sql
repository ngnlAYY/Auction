-- W5.5 主机侧运维告警来源：备份目录容量、定时备份失败与 WAL 归档积压。
-- owner: auction-application（告警来源/种类闭集与阈值）、auction-storage（SQL 与权限）、
--        auction-cli（主机事实采集入口 `auction-cli alert report`）
--
-- 不变量：
--   1. 只放宽 `operations_alert_signals` 的 kind/scope/source/identity CHECK，不改写任何既有
--      signal、批次、通知或确认事实；已存在的行与历史周期保持原样。
--   2. 新增 `backup` 与 `wal` 两个来源，各自绑定唯一 scope：`backup` 只承载
--      `disk_capacity_low`/`backup_failed`，`wal` 只承载 `wal_upload_stalled`；
--      `source_unavailable` 仍要求 scope 等于来源名，因此两者的 scope 闭集必须同时扩展。
--   3. 主机侧来源不在 Worker 的扫描闭集内：Worker 无法读取主机文件系统，不得伪造
--      “来源不可用”。它们只由 `auction-cli alert report` 提交，写入路径复用同一
--      `operations_alert_signals` 水位与 fingerprint（kind + scope），CLI 重试是 upsert，
--      不会产生第二条 signal，也不会重置已排定的下一次通知时间。
--   4. 不新建表、不新增写入面：运行账号对 `operations_alert_signals` 的现有授权已覆盖
--      主机侧写入，因此本迁移不重新执行授权函数。
-- 等级：breaking。放宽 CHECK 后新程序会写入旧程序不认识的新 kind/source；旧程序读取这些
--   行时按闭集解析失败，无法回退到旧版本继续扫描与展示。停机顺序与其它迁移相同，在停机
--   门禁下执行。
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
            'disk_capacity_low', 'backup_failed', 'wal_upload_stalled'
        )
    ),
    ADD CONSTRAINT operations_alert_signals_scope_check CHECK (
        scope IN (
            'public', 'admin', 'worker', 'audit', 'process', 'delivery',
            'auction', 'fulfillment', 'recovery', 'main', 'isekai',
            'mail', 'credential', 'lifecycle', 'forum', 'notification',
            'mail_cookie', 'external_api', 'backup', 'wal'
        )
    ),
    ADD CONSTRAINT operations_alert_signals_source_check CHECK (
        source IN (
            'process', 'delivery', 'fulfillment', 'recovery',
            'mail', 'credential', 'lifecycle', 'forum', 'notification',
            'backup', 'wal'
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
    );

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (91, 'breaking');
