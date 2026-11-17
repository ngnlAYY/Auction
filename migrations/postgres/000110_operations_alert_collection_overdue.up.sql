-- 长期未领取告警：已寄出、只等收件人在游戏内领取的 Send 履约，寄出时长超过运行配置阈值时
-- 产生运维告警；阈值经 Admin 运行配置保存，命令账本随之按原始请求记录命令身份。
-- owner: auction-application（告警种类、阈值与评估，运行配置命令摘要）、auction-storage（只读
-- 水位查询，运行配置命令账本）
--
-- 不变量：
--   1. 只扩展 operations_alert_signals 的 kind 与 identity 闭集；不新增表、来源或 scope，
--      新种类与既有履约告警同一身份：source = 'fulfillment'、scope = 'auction'。
--   2. 告警只提示人工跟进，不改变义务、投递、资金或资产状态，也不触发重发、撤回或任何
--      自动动作；等待领取全部收口或寄出时长回落到阈值以下后，由下一轮扫描关闭。
--   3. 阈值只来自运行配置（Worker 与 Admin runtime 的同名字段），不进入数据库约束。
--   4. 同时补上 000095 漏登记的 scope `reconciliation`：`source_unavailable` 要求 scope 等于
--      来源名，资产对账报告缺失或读取失败时该信号此前违反 scope CHECK，使整轮 Worker 扫描事务
--      回滚、所有告警停止更新。只放宽 scope 闭集，identity 规则不变。
--   5. 新增部分索引 settlement_obligation_fulfillments_open_idx（谓词
--      execution_state <> 'satisfied' AND NOT dormant）与 settlement_obligations_blocked_idx（谓词
--      business_state = 'blocked'）：告警扫描与 Admin 看板的履约水位只读尚未满足、且不属于从未
--      激活的或有义务的履约，以及缺少履约行的 blocked 义务；已满足的历史与从未激活的或有买方
--      退款都不进入扫描，入口规模不随成交 lot 增长。只提供候选入口，不改变计数口径、金额、状态
--      迁移、锁序或审计。
--      `dormant` 标记从未激活的或有义务的履约（只有买方退款可以是 contingent）：结算写入或有
--      义务时置真；recovery 激活退款、装备损失把退款履约直接收口为 failed 时同事务置假。CHECK
--      保证标记只留在初始 pending（无投递、无领取）的行上，把履约推出 pending 而不清标记的写入
--      当即失败；延迟约束触发器保证义务离开 contingent 时其履约已清标记、带标记的履约只属于
--      contingent 义务，漏清标记的写入在提交时失败关闭，已激活的退款不会从活动入口与计数中
--      消失。既有行按同一谓词回填（contingent 义务的初始 pending 履约），只写标记，不改执行
--      状态、状态版本或更新时间。
--   6. runtime_config_admin_commands 的 digest_version 闭集放宽为 1、2。版本 2 的 request_digest
--      是原始请求的摘要：运行配置新阈值省略与显式写出（含显式写出默认值）是不同请求，落成候选
--      只经 new_fingerprint 绑定，同一 command_id 只有原样重试才返回原结果（命令幂等）。升级前
--      写入的版本 1 行不改写、不回填，摘要仍按落成候选计算，同 ID 重试按原口径判定。
--
-- 等级：breaking。旧程序无法解析新增 kind，读取该行时闭集解析失败；版本 2 的运行配置命令行
-- 在旧程序里同样校验失败；旧程序激活带标记的或有退款时不清标记，提交被延迟约束触发器拒绝。
-- 回退须按 RECOVERY.md 处理含 breaking 迁移的版本。
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
            'custody', 'solvency', 'reconciliation'
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
                'fulfillment_manual_reconcile', 'fulfillment_collection_overdue'
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

-- 从未激活的或有义务：标记只能留在初始 pending（无投递、无领取）的履约上。
LOCK TABLE public.settlement_obligations, public.settlement_obligation_fulfillments
    IN SHARE ROW EXCLUSIVE MODE;

ALTER TABLE public.settlement_obligation_fulfillments
    ADD COLUMN dormant boolean NOT NULL DEFAULT false,
    ADD CONSTRAINT settlement_obligation_fulfillments_dormant_check CHECK (
        NOT dormant
        OR (execution_state = 'pending' AND delivery_id IS NULL AND claim_token IS NULL)
    );

COMMENT ON COLUMN public.settlement_obligation_fulfillments.dormant IS
    '所属义务是从未激活的或有义务（contingent），履约停在初始 pending，不进入活动入口';

UPDATE public.settlement_obligation_fulfillments AS fulfillment
SET dormant = true
FROM public.settlement_obligations AS obligation
WHERE obligation.id = fulfillment.obligation_id
  AND obligation.business_state = 'contingent'
  AND fulfillment.execution_state = 'pending'
  AND fulfillment.delivery_id IS NULL
  AND fulfillment.claim_token IS NULL;

-- 标记与义务状态的跨表一致性：在提交时复核，写入方须在同一事务内同时改义务与标记。
CREATE FUNCTION public.validate_settlement_fulfillment_dormant()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    checked_obligation uuid;
BEGIN
    IF TG_TABLE_NAME = 'settlement_obligations' THEN
        checked_obligation := NEW.id;
    ELSE
        checked_obligation := NEW.obligation_id;
    END IF;
    IF EXISTS (
        SELECT 1
        FROM public.settlement_obligation_fulfillments AS fulfillment
        JOIN public.settlement_obligations AS obligation
          ON obligation.id = fulfillment.obligation_id
        WHERE fulfillment.obligation_id = checked_obligation
          AND fulfillment.dormant
          AND obligation.business_state <> 'contingent'
    ) THEN
        RAISE EXCEPTION '结算履约 % 带或有未激活标记，但义务已不是 contingent', checked_obligation
            USING ERRCODE = '23514';
    END IF;
    RETURN NULL;
END;
$$;

CREATE CONSTRAINT TRIGGER settlement_obligations_dormant_consistent
AFTER UPDATE OF business_state ON public.settlement_obligations
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW
WHEN (OLD.business_state = 'contingent' AND NEW.business_state <> 'contingent')
EXECUTE FUNCTION public.validate_settlement_fulfillment_dormant();

CREATE CONSTRAINT TRIGGER settlement_obligation_fulfillments_dormant_consistent
AFTER INSERT OR UPDATE OF dormant ON public.settlement_obligation_fulfillments
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW
WHEN (NEW.dormant)
EXECUTE FUNCTION public.validate_settlement_fulfillment_dormant();

-- 履约水位的候选入口：只含尚未满足、且不属于从未激活或有义务的履约；履约以义务为主键，索引
-- 只存义务标识。
CREATE INDEX settlement_obligation_fulfillments_open_idx
    ON public.settlement_obligation_fulfillments (obligation_id)
    WHERE execution_state <> 'satisfied' AND NOT dormant;

-- 履约水位的另一入口：缺少履约行的 blocked 义务只能从 blocked 义务里找。
CREATE INDEX settlement_obligations_blocked_idx
    ON public.settlement_obligations (id)
    WHERE business_state = 'blocked';

-- 运行配置命令账本：新命令以版本 2 记录原始请求摘要；既有版本 1 行原样保留并继续通过约束。
ALTER TABLE public.runtime_config_admin_commands
    DROP CONSTRAINT runtime_config_admin_commands_digest_version_check,
    ADD CONSTRAINT runtime_config_admin_commands_digest_version_check
        CHECK (digest_version IN (1, 2));

INSERT INTO public.audit_records
    (id, actor_type, actor_id, action, entity_type, entity_id, result, source, metadata)
VALUES (
    gen_random_uuid(), 'system', 'migration-000110', 'system.migration', 'migration',
    '000110', 'success', 'system',
    jsonb_build_object(
        'summary', 'operations alert fulfillment collection overdue kind; '
            || 'dormant contingent fulfillments; runtime command digest v2',
        'compatibility', 'breaking')
);

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (110, 'breaking');
