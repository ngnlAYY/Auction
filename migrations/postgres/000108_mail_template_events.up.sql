-- 出站通知模板的修订事实（只追加，同时是命令账本）。模板正文原先写在 runtime 配置文件里，
-- 文件里固化的旧正文会一直盖住程序内置的默认正文；现在运营改过的正文保存在数据库，
-- 没有改过的模板跟随内置默认正文。
-- owner: auction-application（模板键闭集、占位符规则、命令摘要/重放/版本 CAS）、
--        auction-storage（唯一 SQL 写入与读取）
--
-- 不变量：
--   1. 每个模板键以 revision 最高的一条事实为准：set 给出运营正文，reset 表示回到内置默认正文；
--      从未有事实的模板键使用内置默认正文。本表不保存默认正文。
--   2. command_id 是主键：同 ID 同请求返回原事实，同 ID 不同请求冲突；(template_key, revision)
--      唯一且 revision = expected_revision + 1，过期版本无法写入。
--   3. 事实只追加，触发器拒绝 UPDATE/DELETE/TRUNCATE；运行账号只获得 SELECT 与 INSERT。
--      每条事实同事务写入一条审计记录（audit_id）。
--   4. 数据库只检查模板键的形状，闭集与占位符规则由应用层在写入前校验；旧程序不认识的键
--      由读取方忽略。正文只是通知措辞，不决定金额、资格、幂等、锁、会话或审计。
--   5. 已创建的通知不受影响：被超提醒的正文在创建时冻结，其余通知在发送时渲染一次。
--
-- 兼容等级 compatible：纯新增表、索引、触发器和运行账号只追加授权；旧程序不读取本表，
-- 继续使用自身配置里的模板，行为不变。

CREATE TABLE public.mail_template_events (
    command_id uuid PRIMARY KEY CHECK (command_id <> '00000000-0000-0000-0000-000000000000'),
    digest_version smallint NOT NULL CHECK (digest_version = 1),
    request_digest bytea NOT NULL CHECK (octet_length(request_digest) = 32),
    template_key text NOT NULL CHECK (octet_length(template_key) BETWEEN 1 AND 64
        AND template_key ~ '^[a-z][a-z0-9_]*$'),
    revision bigint NOT NULL CHECK (revision > 0),
    expected_revision bigint NOT NULL
        CHECK (expected_revision >= 0 AND revision = expected_revision + 1),
    operation text NOT NULL CHECK (operation IN ('set', 'reset')),
    -- set 的运营正文；上限与单封 MoogleMail 正文一致。reset 没有正文。
    body text,
    actor_id text NOT NULL CHECK (octet_length(actor_id) BETWEEN 1 AND 256
        AND actor_id = btrim(actor_id)),
    audit_id uuid NOT NULL UNIQUE REFERENCES public.audit_records(id) ON DELETE RESTRICT,
    result_payload jsonb NOT NULL CHECK (jsonb_typeof(result_payload) = 'object'),
    recorded_at timestamptz NOT NULL CHECK (isfinite(recorded_at)),
    CHECK ((operation = 'set' AND body IS NOT NULL AND octet_length(body) BETWEEN 1 AND 65536)
        OR (operation = 'reset' AND body IS NULL)),
    UNIQUE (template_key, revision)
);

COMMENT ON TABLE public.mail_template_events IS
    '出站通知模板的修订事实兼命令账本（只追加）；每个模板键以最高 revision 为准，reset 表示使用内置默认正文';

CREATE INDEX mail_template_events_latest_idx
    ON public.mail_template_events (template_key, revision DESC);

CREATE FUNCTION public.reject_mail_template_event_mutation()
RETURNS trigger LANGUAGE plpgsql AS $function$
BEGIN
    RAISE EXCEPTION 'mail template events are append-only' USING ERRCODE = '55000';
END;
$function$;

CREATE TRIGGER mail_template_events_append_only
BEFORE UPDATE OR DELETE ON public.mail_template_events
FOR EACH ROW EXECUTE FUNCTION public.reject_mail_template_event_mutation();

CREATE TRIGGER mail_template_events_no_truncate
BEFORE TRUNCATE ON public.mail_template_events
FOR EACH STATEMENT EXECUTE FUNCTION public.reject_mail_template_event_mutation();

-- 运行账号授权：在只追加事实列表里加入 mail_template_events，其余与 000102 的定义逐字一致。
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

SELECT public.apply_auction_runtime_privileges();

INSERT INTO public.audit_records
    (id, actor_type, actor_id, action, entity_type, entity_id, result, source, metadata)
VALUES (
    gen_random_uuid(), 'system', 'migration-000108', 'system.migration', 'migration',
    '000108', 'success', 'system',
    jsonb_build_object(
        'summary', 'mail template revisions move from runtime configuration to the database',
        'table', 'mail_template_events',
        'compatibility', 'compatible'
    )
);

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (108, 'compatible');
