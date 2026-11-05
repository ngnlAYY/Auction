-- W5.8 告警台（决策 66）：活动/历史列表、确认与带到期的静默、每日摘要与导航待办计数。
-- owner: auction-application（告警台投影与命令边界）、auction-storage（唯一 SQL 与只追加事实）
--
-- 不变量：
--   1. 告警的活动/恢复状态仍只由 `operations_alert_signals` 水位拥有；本迁移不建立第二套
--      状态机，也不改写任何既有 signal、批次或通知证据。
--   2. 确认与静默事实只追加：`operations_alert_acknowledgements` 由 BEFORE UPDATE/DELETE
--      触发器与权限层拒绝改写，保留政策为永久；`command_id` 唯一承载幂等身份。
--   3. 静默只把 `next_notify_at` 推到到期时间；kind/scope/source/count/age/active 与阈值
--      评估不变，到期后重复通知自然恢复。确认只记录当前活动周期内的操作者事实。
--   4. signal 行上的 `acknowledged_at/acknowledged_by/silenced_until` 是当前活动周期的快速
--      投影；观察事务在 signal 从已恢复重新变为活动时清空它们，历史仍由事实表完整保存。
--   5. `resolution_notified_at` 只用于把已恢复的每日摘要类信号附带进下一封摘要；它不代表
--      恢复通知单独发送，也不影响任何业务事实。
--   6. 运行账号按 000081/000083/000088 的最小权限模型只获得 SELECT 与 INSERT；写入口唯一。
--
-- 等级：compatible。新增列允许旧程序省略、新表旧程序不读取；旧程序回退后仅停止识别
--   确认/静默（重复通知恢复），不会读到未知闭集或无法解析的既有行。
-- 停机顺序：与其它迁移相同，在停机门禁下执行；旧 Worker 不需要提前停止。

ALTER TABLE public.operations_alert_signals
    ADD COLUMN acknowledged_at timestamptz,
    ADD COLUMN acknowledged_by text CHECK (
        acknowledged_by IS NULL OR (
            length(acknowledged_by) BETWEEN 1 AND 256
            AND acknowledged_by = btrim(acknowledged_by)
            AND acknowledged_by !~ '[[:cntrl:]]'
        )
    ),
    ADD COLUMN silenced_until timestamptz,
    ADD COLUMN resolution_notified_at timestamptz;

CREATE TABLE public.operations_alert_acknowledgements (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    fingerprint bytea NOT NULL
        REFERENCES public.operations_alert_signals(fingerprint) ON DELETE RESTRICT
        CHECK (octet_length(fingerprint) = 32),
    action text NOT NULL CHECK (action IN ('acknowledge', 'silence')),
    command_id uuid NOT NULL UNIQUE
        CHECK (command_id <> '00000000-0000-0000-0000-000000000000'),
    request_digest bytea NOT NULL CHECK (octet_length(request_digest) = 32),
    actor_id text NOT NULL CHECK (
        length(actor_id) BETWEEN 1 AND 256
        AND actor_id = btrim(actor_id)
        AND actor_id !~ '[[:cntrl:]]'
    ),
    reason text NOT NULL DEFAULT '' CHECK (
        octet_length(reason) <= 2048
        AND reason = btrim(reason)
        AND reason !~ '[[:cntrl:]]'
    ),
    silence_seconds integer CHECK (silence_seconds BETWEEN 60 AND 604800),
    silenced_until timestamptz,
    occurred_at timestamptz NOT NULL DEFAULT clock_timestamp() CHECK (isfinite(occurred_at)),
    CHECK (
        (action = 'acknowledge' AND silence_seconds IS NULL AND silenced_until IS NULL)
        OR (action = 'silence' AND silence_seconds IS NOT NULL AND silenced_until IS NOT NULL
            AND silenced_until > occurred_at)
    )
);

CREATE INDEX operations_alert_acknowledgements_fingerprint_idx
    ON public.operations_alert_acknowledgements (fingerprint, occurred_at DESC, id);

COMMENT ON TABLE public.operations_alert_acknowledgements IS
    '告警台确认与静默的只追加操作者事实；不改变 signal 状态、阈值或业务数据';

CREATE FUNCTION public.reject_operations_alert_acknowledgement_mutation() RETURNS trigger
LANGUAGE plpgsql AS $function$
BEGIN
    RAISE EXCEPTION 'operations alert acknowledgements are append only' USING ERRCODE = '55000';
END;
$function$;

CREATE TRIGGER operations_alert_acknowledgements_append_only
    BEFORE UPDATE OR DELETE ON public.operations_alert_acknowledgements
    FOR EACH ROW EXECUTE FUNCTION public.reject_operations_alert_acknowledgement_mutation();

-- ── 运行账号授权：新表只有 SELECT 与 INSERT ─────────────────────────────────────────

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
        auction_forum_catalog_pages, auction_forum_reply_cursors, auction_forum_sync,
        auction_intake_targets, auction_preview_items, auction_schema_identity,
        auction_submissions, auctions, bid_proxy_attempts, bid_proxy_authorizations,
        bid_proxy_outbid_uses, compensation_pool, credential_admin_commands, credential_keys,
        deliveries, delivery_admin_commands, delivery_send_cadence, equipment, equipment_secrets,
        forum_bidder_identities, forum_identity_history, forum_sync_admin_commands, inbound_mail,
        lot_watch_leadership, lot_watch_notifications, lot_watch_pending_notifications,
        lot_watch_pm_deliveries, lot_watches, lots, mail_admin_commands, mail_commands,
        mail_poll_pending_reads, mail_poll_state, mail_runtime_generation, mailbox_sweep_cursors,
        mail_scan_checkpoints,
        operations_alert_batch_items, operations_alert_batches, operations_alert_runtime_state,
        operations_alert_signals, public_sessions, realtime_consumer_offsets, realtime_events,
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
        operations_alert_acknowledgements,
        delivery_send_attempts, delivery_sent_bindings, delivery_sent_confirmations,
        delivery_take_attempts,
        lot_settlements, runtime_switch_events, settlement_equipment_losses,
        settlement_legacy_bot_seller_claims, settlement_legacy_self_reconciliations,
        settlement_recovery_return_consumptions, user_blacklist_history
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

REVOKE ALL ON FUNCTION public.apply_auction_runtime_privileges() FROM PUBLIC;

-- 迁移时立即应用；角色前置条件不满足时以明确错误终止整个迁移。
SELECT public.apply_auction_runtime_privileges();

INSERT INTO public.audit_records
    (id, actor_type, actor_id, action, entity_type, entity_id, result, source, metadata)
VALUES (
    gen_random_uuid(), 'system', 'migration-000089', 'system.migration', 'migration',
    '000089', 'success', 'system',
    jsonb_build_object(
        'summary', 'operations alert console acknowledge and silence facts',
        'table', 'operations_alert_acknowledgements',
        'actions', jsonb_build_array('acknowledge', 'silence'),
        'compatibility', 'compatible'
    )
);

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (89, 'compatible');
