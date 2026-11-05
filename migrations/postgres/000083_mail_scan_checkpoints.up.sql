-- storage owner：W1.4 Sent / Read Mail 增量扫描检查点持久化。
--
-- 不变量：
--   * 检查点只保存“已完整扫描到的最高 MID + 列表时间”和最近完整校验时间；
--     它不是发送、收取、已读、领取或资金证明，Inbox 仍以 mail_poll_state.cursor_value 为权威游标。
--   * 作用域由 (credential_id, world_id, stream) 定位，并绑定凭据 state_version 与远端 endpoint origin；
--     任一不匹配、行损坏或读取失败都必须回退完整扫描。
--   * 只允许 sent / read_mail 两个承担优化职责的流；Inbox 检查点不进入数据库。
--   * 覆盖写入不追加历史证据；该表不参与审计、对账或任何完成判定。
--
-- 兼容等级 compatible：新增表与运行账号写入授权，旧程序不读取也不依赖；既有数据行为不变。
-- 依赖 000081：未来由迁移 owner 新建的表默认只读，因此本迁移把新表加入可变表清单，
-- 并重新执行 public.apply_auction_runtime_privileges()，保证备份恢复路径与运行时授权一致。

CREATE TABLE public.mail_scan_checkpoints (
    credential_id uuid NOT NULL REFERENCES public.credential_keys(id) ON DELETE CASCADE,
    world_id text NOT NULL CHECK (world_id IN ('main', 'isekai')),
    stream text NOT NULL CHECK (stream IN ('sent', 'read_mail')),
    credential_version bigint NOT NULL CHECK (credential_version > 0),
    endpoint_scope text NOT NULL
        CHECK (endpoint_scope = btrim(endpoint_scope)
               AND octet_length(endpoint_scope) BETWEEN 1 AND 512
               AND endpoint_scope !~ '[[:cntrl:]]'),
    watermark_mail_id text
        CHECK (watermark_mail_id IS NULL OR (
            watermark_mail_id ~ '^(0|[1-9][0-9]{0,38})$'
        )),
    watermark_observed_at timestamptz
        CHECK (watermark_observed_at IS NULL OR isfinite(watermark_observed_at)),
    last_full_scan_at timestamptz
        CHECK (last_full_scan_at IS NULL OR isfinite(last_full_scan_at)),
    updated_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    PRIMARY KEY (credential_id, world_id, stream),
    CHECK ((watermark_mail_id IS NULL) = (watermark_observed_at IS NULL))
);

COMMENT ON TABLE public.mail_scan_checkpoints IS
    'Sent/Read Mail 增量扫描检查点；不是发送、已读或领取证明，作用域不匹配时回退完整扫描';

-- 同一份授权定义继续由 public.apply_auction_runtime_privileges() 拥有；本迁移只把新表
-- 加入可变表清单，其余语句与 000081 完全一致。
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
    gen_random_uuid(), 'system', 'migration-000083', 'system.migration', 'migration',
    '000083', 'success', 'system',
    jsonb_build_object(
        'summary', 'persist Sent and Read Mail scan checkpoints',
        'compatibility', 'compatible'
    )
);

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (83, 'compatible');
