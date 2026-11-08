-- W5.1：Webhook 告警通道使用独立持久投递账本，不占用 MoogleMail 的 delivery/冷却状态。
-- owner: auction-application（通道模型与结果分类）、auction-storage（账本与领取/终态）
--
-- 不变量：
--   1. webhook 投递只能使用创建时冻结的 JSON；发送时不得按当前配置重算。
--   2. 每次投递有唯一 command_id，并以 lease/fencing 防止并发重复发送。
--   3. 发送结果未知时进入 manual_reconcile，绝不自动重发。
--   4. webhook 的 next_webhook_at 与 MoogleMail 的 next_notify_at 完全分离。
-- 等级：compatible。仅新增表和可空列，旧程序忽略它们即可继续运行。
LOCK TABLE public.operations_alert_signals IN SHARE ROW EXCLUSIVE MODE;

ALTER TABLE public.operations_alert_signals
    ADD COLUMN next_webhook_at timestamptz
        CHECK (next_webhook_at IS NULL OR isfinite(next_webhook_at));

UPDATE public.operations_alert_signals
SET next_webhook_at = transaction_timestamp()
WHERE active AND next_webhook_at IS NULL;

CREATE INDEX operations_alert_signals_next_webhook_idx
    ON public.operations_alert_signals (next_webhook_at, fingerprint)
    WHERE active AND next_webhook_at IS NOT NULL;

CREATE TABLE public.operations_alert_webhook_deliveries (
    id uuid PRIMARY KEY,
    command_id uuid NOT NULL UNIQUE,
    fingerprint bytea NOT NULL
        REFERENCES public.operations_alert_signals(fingerprint) ON DELETE RESTRICT,
    signal_version bigint NOT NULL CHECK (signal_version > 0),
    request_hash bytea NOT NULL CHECK (octet_length(request_hash) = 32),
    payload jsonb NOT NULL CHECK (jsonb_typeof(payload) = 'object'),
    status text NOT NULL CHECK (
        status IN ('pending', 'sending', 'succeeded', 'failed', 'manual_reconcile')
    ),
    claim_token uuid,
    claim_expires_at timestamptz,
    claim_fencing bigint NOT NULL DEFAULT 0 CHECK (claim_fencing >= 0),
    attempt_count integer NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    next_attempt_at timestamptz,
    sending_at timestamptz,
    completed_at timestamptz,
    failure_code text,
    result_metadata jsonb NOT NULL DEFAULT '{}'::jsonb
        CHECK (jsonb_typeof(result_metadata) = 'object'),
    created_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    updated_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    CHECK (updated_at >= created_at),
    CHECK (
        (status = 'sending' AND claim_token IS NOT NULL AND claim_expires_at IS NOT NULL
            AND sending_at IS NOT NULL AND completed_at IS NULL)
        OR (status = 'pending' AND claim_token IS NULL AND claim_expires_at IS NULL
            AND next_attempt_at IS NOT NULL AND completed_at IS NULL)
        OR (status IN ('succeeded', 'failed', 'manual_reconcile')
            AND claim_token IS NULL AND claim_expires_at IS NULL AND completed_at IS NOT NULL)
    ),
    CHECK (status <> 'succeeded' OR failure_code IS NULL),
    CHECK (status NOT IN ('failed', 'manual_reconcile') OR failure_code IS NOT NULL)
);

CREATE UNIQUE INDEX operations_alert_webhook_open_fingerprint_idx
    ON public.operations_alert_webhook_deliveries (fingerprint)
    WHERE status IN ('pending', 'sending', 'manual_reconcile');

CREATE INDEX operations_alert_webhook_due_idx
    ON public.operations_alert_webhook_deliveries (status, next_attempt_at, created_at, id);

GRANT SELECT, INSERT, UPDATE, DELETE
    ON public.operations_alert_webhook_deliveries TO auction_runtime;

-- 同步备份恢复使用的完整授权函数；否则 `pg_restore --no-owner` 后运行账号只能读新表。
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

SELECT public.apply_auction_runtime_privileges();

INSERT INTO public.audit_records
    (id, actor_type, actor_id, action, entity_type, entity_id, result, source, metadata)
VALUES (
    gen_random_uuid(), 'system', 'migration-000102', 'system.migration', 'migration',
    '000102', 'success', 'system',
    jsonb_build_object('summary', 'operations alert webhook channel', 'compatibility', 'compatible')
);

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (102, 'compatible');
