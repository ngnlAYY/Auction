-- W12.1 残留：把被超提醒与自动标记写入移出 Public 出价的 Serializable 事务。
--
-- 背景：出价事务此前在同一 Serializable 窗口内写 `lot_watches`（自动标记）、
-- `lot_watch_leadership`（领先周期）、`lot_watch_notifications` / `lot_watch_pm_deliveries`
-- 与 `deliveries`（提醒入队），并触发 `notify_delivery_ready` 的 `pg_notify`。这些写入跨多个
-- 共享行/索引页与读谓词，使 Final 阶段不同 lot 的重叠出价形成 SSI 读写反依赖，重试耗尽后
-- 玩家收到 503。
--
-- 不变量：
--   1. 竞价事实（`bids`）、实时事件、lot 最终状态、命令账本与命令/竞价审计仍与出价同一
--      Serializable 事务原子提交；本迁移不移动也不改写这些事实。
--   2. 提醒与自动标记只由已提交事实派生：出价事务只向本表追加一个身份确定的待处理事件，
--      事件行不更新任何既有表、不触发投递唤醒；提醒/标记由提交后的同进程处理或 Worker 扫描
--      在独立事务中应用。
--   3. 每个竞价事实、每份新建代理至多一条事件（唯一索引），事件成功后状态单向进入 `done`；
--      处理事务与完成标记同一事务提交，崩溃回滚即可重试，不会丢失也不会重复提醒。
--   4. 同一 lot 的竞价事件按 `state_version` 顺序处理：更早的事件未完成时，后续事件不得领取，
--      因而领先周期、被超提醒与 `lot_watch_notifications` 的身份/周期仍由既有的追加式事实
--      决定，不建立第二套业务状态机。
--   5. 本表只承载派生的处理进度（pending/processing/done）与租约字段；不是业务状态来源，
--      历史提醒、投递、审计与自由文本都不在此复制。
--
-- 兼容等级 compatible：纯新增表、索引与运行权限；旧程序不读该表，仍会在出价事务内写提醒。
-- 新旧程序混跑只会让新程序追加的事件在新 Worker 启动前保持待处理，不会产生重复提醒；
-- 停机门禁下同版切换仍按部署流程执行。

CREATE TABLE public.lot_watch_events (
    event_id uuid PRIMARY KEY,
    event_kind text NOT NULL CHECK (event_kind IN ('bid_accepted', 'proxy_created')),
    lot_id uuid NOT NULL REFERENCES public.lots(id) ON DELETE RESTRICT,
    user_id uuid NOT NULL REFERENCES public.users(id) ON DELETE RESTRICT,
    bid_id uuid REFERENCES public.bids(id) ON DELETE RESTRICT,
    authorization_id uuid REFERENCES public.bid_proxy_authorizations(authorization_id)
        ON DELETE RESTRICT,
    state_version bigint NOT NULL CHECK (state_version > 0),
    occurred_at timestamptz NOT NULL,
    status text NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'processing', 'done')),
    attempts integer NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    next_attempt_at timestamptz NOT NULL,
    lease_owner text,
    lease_expires_at timestamptz,
    last_error_code text,
    completed_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    CHECK ((event_kind = 'bid_accepted') = (bid_id IS NOT NULL)),
    CHECK ((event_kind = 'proxy_created') = (authorization_id IS NOT NULL)),
    CHECK (status <> 'processing'
        OR (lease_owner IS NOT NULL AND lease_expires_at IS NOT NULL)),
    CHECK ((status = 'done') = (completed_at IS NOT NULL))
);

-- 每个事实最多一条事件；身份由应用层按同一规则生成。
CREATE UNIQUE INDEX lot_watch_events_bid_idx
    ON public.lot_watch_events (bid_id)
    WHERE bid_id IS NOT NULL;
CREATE UNIQUE INDEX lot_watch_events_authorization_idx
    ON public.lot_watch_events (authorization_id)
    WHERE authorization_id IS NOT NULL;

-- 领取候选：只扫描未完成事件，按可用时刻与发生顺序排列。
CREATE INDEX lot_watch_events_due_idx
    ON public.lot_watch_events (next_attempt_at, occurred_at, event_id)
    WHERE status IN ('pending', 'processing');

-- 同 lot 竞价事件的顺序屏障：更早未完成事件必须先行。
CREATE INDEX lot_watch_events_lot_order_idx
    ON public.lot_watch_events (lot_id, state_version, event_id)
    WHERE event_kind = 'bid_accepted' AND status <> 'done';

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
    gen_random_uuid(), 'system', 'migration-000092', 'system.migration', 'migration',
    '000092', 'success', 'system',
    jsonb_build_object(
        'summary', 'lot watch reminder/mark durable post-commit outbox',
        'compatibility', 'compatible')
);

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (92, 'compatible');
