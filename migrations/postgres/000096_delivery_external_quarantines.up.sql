-- W10.9 恢复后外部核对：Sent 精确匹配的投递隔离事实（只追加）。
-- owner: auction-application（匹配与隔离准入）、auction-storage（唯一 SQL 写入与读取）
--
-- 不变量：
--   1. 一个 (delivery_id, fencing_version) 只有一条隔离事实；同一 Sent MID 也只能证明一辆投递。
--   2. 事实只追加，触发器拒绝 UPDATE/DELETE；运行账号只获得 SELECT 与 INSERT。
--   3. 隔离只把 pending/claimed 投递改为 manual_reconcile 并推进 fence；不重发、不领取、
--      不改写资金、结算、履约或资产事实，也不构成发送/收款证明。
--   4. sent_at/observed_at 来自 Worker 暂停状态下的只读详情读取；scan_id 绑定同一次 CLI 回执。
--   5. 恢复库没有该表以外的依赖；旧程序不读取本表，行为不变。
--
-- 兼容等级 compatible：纯新增表、索引、触发器和运行账号只追加授权。

CREATE TABLE public.delivery_external_quarantines (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    -- 同一次恢复核对回执的命令身份；不是业务 command_id。
    scan_id uuid NOT NULL CHECK (scan_id <> '00000000-0000-0000-0000-000000000000'),
    delivery_id uuid NOT NULL REFERENCES public.deliveries(id) ON DELETE RESTRICT,
    -- 被隔离时的投递 fence；投递随后推进 fence，旧事实保持原值。
    fencing_version bigint NOT NULL CHECK (fencing_version >= 1),
    world_id text NOT NULL CHECK (world_id IN ('main', 'isekai')),
    external_mail_id text NOT NULL CHECK (
        length(external_mail_id) BETWEEN 1 AND 64 AND external_mail_id ~ '^[1-9][0-9]*$'
    ),
    sent_at timestamptz NOT NULL CHECK (isfinite(sent_at)),
    recipient text NOT NULL CHECK (
        length(recipient) BETWEEN 1 AND 256 AND recipient = btrim(recipient)
        AND recipient !~ '[[:cntrl:]]'
    ),
    subject text NOT NULL CHECK (
        length(subject) BETWEEN 1 AND 512 AND subject = btrim(subject)
        AND subject !~ '[[:cntrl:]]'
    ),
    collected boolean NOT NULL,
    observed_at timestamptz NOT NULL CHECK (isfinite(observed_at)),
    occurred_at timestamptz NOT NULL DEFAULT clock_timestamp() CHECK (isfinite(occurred_at)),
    UNIQUE (delivery_id, fencing_version),
    UNIQUE (world_id, external_mail_id)
);

COMMENT ON TABLE public.delivery_external_quarantines IS
    '恢复后与 Sent 精确匹配的投递隔离事实（只追加）；不证明发送、领取或收款';

CREATE FUNCTION public.reject_delivery_external_quarantine_mutation() RETURNS trigger
LANGUAGE plpgsql AS $function$
BEGIN
    RAISE EXCEPTION 'delivery external quarantines are append only' USING ERRCODE = '55000';
END;
$function$;

CREATE TRIGGER delivery_external_quarantines_append_only
BEFORE UPDATE OR DELETE ON public.delivery_external_quarantines
FOR EACH ROW EXECUTE FUNCTION public.reject_delivery_external_quarantine_mutation();

CREATE INDEX delivery_external_quarantines_scan_idx
    ON public.delivery_external_quarantines (scan_id, delivery_id);

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
        lot_watch_events,
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
        delivery_external_quarantines, operations_alert_acknowledgements,
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

SELECT public.apply_auction_runtime_privileges();

INSERT INTO public.audit_records
    (id, actor_type, actor_id, action, entity_type, entity_id, result, source, metadata)
VALUES (
    gen_random_uuid(), 'system', 'migration-000096', 'system.migration', 'migration',
    '000096', 'success', 'system',
    jsonb_build_object(
        'summary', 'post-restore external reconciliation quarantine facts',
        'table', 'delivery_external_quarantines',
        'compatibility', 'compatible'
    )
);

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (96, 'compatible');
