-- W9.9 数据库最小权限（决策 35）：迁移 owner 只供 CLI 建库、迁移、重建与恢复使用；
-- Public、Admin 与 Worker 共用一个无 DDL、无 TRUNCATE 的运行账号 auction_runtime。
-- owner: auction-storage
-- 不变量：
--   1. 运行账号只有 public schema 的 USAGE，没有 CREATE，因此不能建表、索引、视图、触发器或函数。
--   2. 运行账号对任何表都没有 TRUNCATE（含 _sqlx_migrations）；只追加表（触发器拒绝 UPDATE/DELETE）
--      只获得 INSERT，冻结证据表只获得 SELECT。
--   3. 序列只授予 USAGE/SELECT，不授予 UPDATE，运行账号不能 setval 绕过分配。
--   4. 未来由迁移 owner 新建的表与序列默认只授予 SELECT（序列另加 USAGE），新增写入面必须在
--      对应迁移里显式 GRANT，未授权即失败关闭。
--   5. 授权对象是固定角色名 auction_runtime；运行账号缺失、拥有对象、可继承 owner/superuser
--      或具有 DDL/降权属性时以明确错误终止，不猜测或静默跳过。
-- 前置条件（部署时由用户执行）：先在集群创建 LOGIN 角色 auction_runtime（非 superuser、
-- 非 CREATEDB、非 CREATEROLE、非 BYPASSRLS），把三角色敏感 TOML 的 [database].username 改为它，
-- 并把应用数据库 owner 保持为迁移 owner。首次设置与合并部署同样先建运行账号再迁移。
-- 停机顺序：本迁移只改授权，不改写业务事实；旧程序以 owner 身份运行时不受影响，可在停机门禁下
-- 与其它迁移同批执行。等级为 compatible：既有数据行、列与约束不变，旧程序读写结果不变。
-- 授权步骤同时固化为 public.apply_auction_runtime_privileges()：备份恢复以 --no-owner
-- --no-privileges 执行，ACL 与默认权限都不在 dump 中，恢复后必须以 owner 重新执行该函数
-- （auction-cli db grants 直接调用同名函数），因此授权只有这一份定义，不会与 CLI 漂移。
-- 是否再按角色拆分运行账号交 W8.2 评估。

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
        compensation_ledger, compensation_pool_reversals, delivery_send_attempts,
        delivery_sent_bindings, delivery_sent_confirmations, delivery_take_attempts,
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
    gen_random_uuid(), 'system', 'migration-000081', 'system.migration', 'migration',
    '000081', 'success', 'system',
    jsonb_build_object(
        'summary', 'database least privilege runtime role',
        'runtime_role', 'auction_runtime',
        'compatibility', 'compatible'
    )
);

-- 只改授权、不改写既有事实；旧程序以 owner 身份运行时读写结果不变，因此登记 compatible。
INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (81, 'compatible');
