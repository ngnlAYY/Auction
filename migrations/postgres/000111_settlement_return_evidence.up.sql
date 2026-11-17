-- 结算退件证据：恢复流程不处理的结算退件的消费事实（只追加）。
-- owner: auction-application（系统退件来源认证与处置闭集）、auction-storage（唯一 SQL 写入与读取）
--
-- 背景：只有系统发件人 `MoogleMail` 以 `[RTS] ` + canonical 结算主题退回的邮件是结算退件。恢复
--   流程只消费其中绑定恢复记录付款或交付义务的退件；货款、退款、流拍/撤拍寄回此前没有消费方，
--   带资产的退件还可能被当作未知邮件处理。无前缀的兼容入口已撤掉：义务 ID 可由公开 ID 推出，
--   它留下的历史记录不能证明来源。
--
-- 不变量：
--   1. 每封入站退件（`inbound_mail.id`）最多消费一次：本表以 `return_mail_id` 为主键，并与恢复流程的
--      `settlement_recovery_return_consumptions` 按“来源可证明且绑定恢复付款/交付义务”互斥分区。
--   2. 事实只追加：触发器拒绝 UPDATE/DELETE/TRUNCATE，运行账号只获得 SELECT 与 INSERT。
--   3. 只记证据：`manual_reconcile` 表示同事务把对应未终结履约转人工对账（原因码
--      `settlement_mail_returned`）；其余状态不改写任何业务事实。不领取、不退回、不重发，不写
--      `settlement_funding_returns`，不改变资金。
--   4. 来源无法证明的记录只隔离为 `untrusted`，不计入恢复退回次数，也不改变恢复流程。
--   5. 已绑定的两种状态必须同时记录原义务、原投递与消费前的履约状态；原因码只校验形状，
--      闭集由应用层 `SettlementReturnReason` 拥有。
--
-- 兼容等级 compatible：纯新增表、索引、触发器与运行账号只追加授权，并为 `inbound_mail` 增加一个
--   普通部分索引；旧程序不读取本表，读写结果与迁移前相同。回退到旧程序后它仍按旧规则（含无
--   前缀兼容入口）处理退件、不认本表的消费事实，这正是迁移前的行为，不会损坏本表。

CREATE TABLE public.settlement_return_evidence (
    return_mail_id uuid PRIMARY KEY REFERENCES public.inbound_mail(id) ON DELETE RESTRICT,
    match_state text NOT NULL CHECK (
        match_state IN ('untrusted', 'unbound', 'fulfillment_terminal', 'manual_reconcile')
    ),
    reason_code text NOT NULL CHECK (reason_code ~ '^[a-z][a-z0-9_]{0,63}$'),
    -- 证据义务 ID 对应的结算义务；不存在时为空。
    obligation_id uuid REFERENCES public.settlement_obligations(id) ON DELETE RESTRICT,
    -- 被退回的原出站投递；只在已绑定时记录。
    delivery_id uuid REFERENCES public.deliveries(id) ON DELETE RESTRICT,
    -- 消费前的履约执行状态；只在已绑定时记录。
    previous_execution_state text CHECK (
        previous_execution_state IS NULL OR previous_execution_state IN (
            'pending', 'prepared', 'enqueued', 'satisfied', 'failed', 'manual_reconcile'
        )
    ),
    recorded_at timestamptz NOT NULL CHECK (isfinite(recorded_at)),
    CHECK (
        (match_state IN ('fulfillment_terminal', 'manual_reconcile')
            AND obligation_id IS NOT NULL AND delivery_id IS NOT NULL
            AND previous_execution_state IS NOT NULL)
        OR (match_state IN ('untrusted', 'unbound')
            AND delivery_id IS NULL AND previous_execution_state IS NULL)
    ),
    CHECK (
        match_state <> 'manual_reconcile'
        OR previous_execution_state IN ('pending', 'prepared', 'enqueued')
    ),
    CHECK (
        match_state <> 'fulfillment_terminal'
        OR previous_execution_state IN ('satisfied', 'failed', 'manual_reconcile')
    )
);

COMMENT ON TABLE public.settlement_return_evidence IS
    '恢复流程不处理的结算退件的消费事实（只追加）；只留证与转人工对账，不领取、不退回、不重发、不改变资金';

CREATE INDEX settlement_return_evidence_obligation_idx
    ON public.settlement_return_evidence (obligation_id)
    WHERE obligation_id IS NOT NULL;

-- 消费方按收件顺序扫描待消费的结算退件；部分索引只覆盖结算退件证据。
CREATE INDEX inbound_mail_settlement_return_idx
    ON public.inbound_mail (received_at, id)
    WHERE mail_kind = 'delivery'
      AND status = 'accepted'
      AND decision_code = 'settlement_recovery_returned';

CREATE FUNCTION public.reject_settlement_return_evidence_mutation()
RETURNS trigger LANGUAGE plpgsql AS $function$
BEGIN
    RAISE EXCEPTION 'settlement return evidence is append-only' USING ERRCODE = '55000';
END;
$function$;

CREATE TRIGGER settlement_return_evidence_append_only
BEFORE UPDATE OR DELETE ON public.settlement_return_evidence
FOR EACH ROW EXECUTE FUNCTION public.reject_settlement_return_evidence_mutation();

CREATE TRIGGER settlement_return_evidence_no_truncate
BEFORE TRUNCATE ON public.settlement_return_evidence
FOR EACH STATEMENT EXECUTE FUNCTION public.reject_settlement_return_evidence_mutation();

-- 运行账号授权：在只追加事实列表里加入 settlement_return_evidence，其余与 000108 的定义逐字一致。
CREATE OR REPLACE FUNCTION public.apply_auction_runtime_privileges() RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $function$
DECLARE
    runtime_role name := 'auction_runtime';
    runtime_oid oid;
BEGIN
    SELECT role.oid INTO runtime_oid
    FROM pg_catalog.pg_roles AS role
    WHERE role.rolname = runtime_role;

    IF runtime_oid IS NULL THEN
        RAISE EXCEPTION
            '运行时角色 % 不存在：请先创建该 LOGIN 账号并改为三角色 [database].username 后重试',
            runtime_role;
    END IF;
    IF runtime_role = current_user THEN
        RAISE EXCEPTION
            '迁移 owner 不能同时是运行时角色 %：服务与迁移必须使用不同账号', runtime_role;
    END IF;
    IF EXISTS (
        SELECT 1 FROM pg_catalog.pg_roles AS role
        WHERE role.oid = runtime_oid
          AND (role.rolsuper OR role.rolcreatedb OR role.rolcreaterole
               OR role.rolbypassrls OR role.rolreplication)
    ) THEN
        RAISE EXCEPTION
            '运行时角色 % 不得具有 superuser、CREATEDB、CREATEROLE、BYPASSRLS 或 REPLICATION 属性',
            runtime_role;
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM pg_catalog.pg_roles AS role
        WHERE role.oid = runtime_oid AND role.rolcanlogin
    ) THEN
        RAISE EXCEPTION '运行时角色 % 必须允许登录（LOGIN）', runtime_role;
    END IF;
    -- 对象 owner 天然拥有全部 DDL 与 TRUNCATE 权限，不能靠 GRANT/REVOKE 收口。
    IF EXISTS (
        SELECT 1 FROM pg_catalog.pg_class AS relation
        WHERE relation.relowner = runtime_oid
          AND relation.relnamespace = 'public'::pg_catalog.regnamespace
    ) OR EXISTS (
        SELECT 1 FROM pg_catalog.pg_namespace AS namespace
        WHERE namespace.nspname = 'public' AND namespace.nspowner = runtime_oid
    ) THEN
        RAISE EXCEPTION '运行时角色 % 不得拥有 public schema 或其对象', runtime_role;
    END IF;
    -- 继承 owner 或 superuser 同样得到 DDL；仅检查成员关系，不改动角色定义。
    IF EXISTS (
        SELECT 1
        FROM pg_catalog.pg_roles AS granted_role
        WHERE granted_role.oid <> runtime_oid
          AND pg_catalog.pg_has_role(runtime_oid, granted_role.oid, 'MEMBER')
          AND (
              granted_role.rolsuper
              OR granted_role.oid = (SELECT datdba FROM pg_catalog.pg_database
                                     WHERE datname = current_database())
              OR EXISTS (
                  SELECT 1 FROM pg_catalog.pg_class AS relation
                  WHERE relation.relowner = granted_role.oid
                    AND relation.relnamespace = 'public'::pg_catalog.regnamespace
              )
          )
    ) THEN
        RAISE EXCEPTION
            '运行时角色 % 不得继承迁移 owner、数据库 owner 或 superuser 角色', runtime_role;
    END IF;

    -- schema 面：只读使用，不允许运行账号建立任何 schema 对象。
    REVOKE ALL ON SCHEMA public FROM auction_runtime;
    GRANT USAGE ON SCHEMA public TO auction_runtime;

    -- 从空权限集合开始重建：先撤销该 owner 曾经授予的全部对象权限（含 TRUNCATE）。
    REVOKE ALL ON ALL TABLES IN SCHEMA public FROM auction_runtime;
    REVOKE ALL ON ALL SEQUENCES IN SCHEMA public FROM auction_runtime;

    -- 默认读取面：普通表、只追加表、视图与 _sqlx_migrations 统一只读（后续不加任何写入授权）。
    GRANT SELECT ON ALL TABLES IN SCHEMA public TO auction_runtime;

    -- 可变表：完整读写，但仍无 TRUNCATE 与 DDL。
    GRANT INSERT, UPDATE, DELETE ON TABLE
        admin_sessions, auction_commands, auction_forum_bid_commands,
        auction_forum_catalog_pages, auction_forum_sync, auction_intake_targets,
        auction_preview_items, auction_schema_identity, auction_submissions, auctions,
        bid_proxy_authorizations, compensation_pool, credential_admin_commands, credential_keys,
        deliveries, delivery_admin_commands, delivery_send_cadence, equipment, equipment_secrets,
        forum_bidder_identities, forum_identity_history, forum_sync_admin_commands, inbound_mail,
        lot_watch_events,
        lot_watch_leadership, lot_watch_notifications, lot_watch_pending_notifications,
        lot_watch_pm_deliveries, lot_watches, lots, mail_admin_commands, mail_commands,
        mail_poll_pending_reads, mail_poll_state, mail_runtime_generation, mailbox_sweep_cursors,
        mail_scan_checkpoints,
        operations_alert_batch_items, operations_alert_batches, operations_alert_runtime_state,
        operations_alert_signals, operations_alert_webhook_deliveries, public_sessions,
        realtime_consumer_offsets, realtime_events,
        realtime_failures, realtime_streams, rules_admin_commands, rules_documents,
        runtime_config_admin_commands, scheduled_rounds, service_heartbeats,
        settlement_funding_accounts, settlement_funding_allocations, settlement_funding_commands,
        settlement_funding_entries, settlement_funding_lots, settlement_funding_receipts,
        settlement_funding_returns, settlement_funding_scope, settlement_funding_sources,
        settlement_internal_commands, settlement_obligation_fulfillments, settlement_obligations,
        settlement_recoveries, settlement_recovery_actions, settlement_recovery_commands,
        settlement_recovery_health, user_admin_commands, user_blacklist,
        user_game_identity_history, user_login_key_envelopes, user_mail_commands, user_renames,
        users, worlds
    TO auction_runtime;

    -- 只追加事实：触发器已拒绝 UPDATE/DELETE，权限层同时只给 INSERT。
    GRANT INSERT ON TABLE
        auction_settlements, audit_records, bid_proxy_authorization_transitions, bids,
        compensation_ledger, compensation_pool_reversals, delivery_dispositions,
        delivery_external_quarantines, operations_alert_acknowledgements,
        delivery_send_attempts, delivery_sent_bindings, delivery_sent_confirmations,
        delivery_take_attempts,
        lot_settlements, mail_template_events, runtime_switch_events,
        settlement_equipment_losses,
        settlement_legacy_bot_seller_claims, settlement_legacy_self_reconciliations,
        settlement_recovery_return_consumptions, settlement_return_evidence,
        user_blacklist_history
    TO auction_runtime;

    -- 冻结证据与迁移登记只读：settlement_pools（071 冻结）、schema_migration_compatibility。
    -- （SELECT 已在上面统一授予，这里不追加任何写入权限。）

    -- 序列只允许取号，不允许 setval。
    GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA public TO auction_runtime;

    -- 默认权限：未来由本 owner 新建的对象仍只授予读取面，写入面必须由新迁移显式 GRANT。
    ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT SELECT ON TABLES TO auction_runtime;
    ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT USAGE, SELECT ON SEQUENCES TO auction_runtime;

    -- 该函数只应由迁移 owner 执行；恢复后默认 ACL 会被重置，函数自身再次收口。
    REVOKE ALL ON FUNCTION public.apply_auction_runtime_privileges() FROM PUBLIC;
END
$function$;

SELECT public.apply_auction_runtime_privileges();

INSERT INTO public.audit_records
    (id, actor_type, actor_id, action, entity_type, entity_id, result, source, metadata)
VALUES (
    gen_random_uuid(), 'system', 'migration-000111', 'system.migration', 'migration',
    '000111', 'success', 'system',
    jsonb_build_object(
        'summary', 'settlement return evidence for returns outside the recovery flow',
        'table', 'settlement_return_evidence',
        'compatibility', 'compatible'
    )
);

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (111, 'compatible');
