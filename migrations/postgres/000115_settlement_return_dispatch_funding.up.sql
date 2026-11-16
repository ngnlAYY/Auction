-- 结算退件补寄的派发历史与资金返还：原义务上的新派发按 `(义务, 派发编号)` 只追加登记，
-- 资金投递绑定保留历史，`settlement_funding_returns` 改为带入站/Take 绑定的只追加事实。
-- owner: auction-domain（派发编号与补寄身份）、auction-storage（派发登记、资金返还与历史绑定）
--
-- 不变量：
--   1. 履约行新增当前派发编号 `dispatch_no`（首寄 1，既有行默认 1）；派发历史
--      `settlement_fulfillment_dispatches` 以 `(obligation_id, dispatch_no)` 为主键、投递唯一，补寄
--      必须引用授权它的退件案件（每案最多一次）。既有的已准备履约补登为首寄派发。
--   2. 资金投递绑定只追加：`settlement_funding_delivery_bindings` 记录每个出站投递曾占用的分配；
--      分配指针在返还后换代，旧派发仍按历史绑定定位原账户并受资金保护，迟到回调只能重放旧结论。
--      既有分配的当前绑定补登为历史。
--   3. 资金返还只凭精确领回证明写一次：`settlement_funding_returns` 以原投递为主键，新行必须同时
--      绑定退件案件、Take 投递与 `returned_delivery` 来源（各自唯一）；表改为只追加，运行账号只获
--      INSERT（此前位于可变授权列表）。返还把原分配由 spent 恢复为 available，金额仍为原额。
--
-- 兼容等级 breaking：旧程序按 `allocation.delivery_id` 单指针定位资金，看不到历史绑定，补寄换代
--   后会把旧派发判为无资金关联；也不认识返还来源的额度口径。回退旧程序前须先确认没有补寄派发与
--   资金返还事实（见 ops/deploy/RECOVERY.md）。

LOCK TABLE public.settlement_obligation_fulfillments, public.settlement_funding_allocations,
    public.settlement_funding_returns IN SHARE ROW EXCLUSIVE MODE;

ALTER TABLE public.settlement_obligation_fulfillments
    ADD COLUMN dispatch_no integer NOT NULL DEFAULT 1 CHECK (dispatch_no >= 1);

CREATE TABLE public.settlement_fulfillment_dispatches (
    obligation_id uuid NOT NULL
        REFERENCES public.settlement_obligations(id) ON DELETE RESTRICT,
    dispatch_no integer NOT NULL CHECK (dispatch_no >= 1),
    delivery_id uuid NOT NULL UNIQUE REFERENCES public.deliveries(id) ON DELETE RESTRICT,
    -- 授权补寄的退件案件；首寄为空。
    authorized_by uuid UNIQUE
        REFERENCES public.settlement_return_cases(return_mail_id) ON DELETE RESTRICT,
    recorded_at timestamptz NOT NULL CHECK (isfinite(recorded_at)),
    PRIMARY KEY (obligation_id, dispatch_no),
    CHECK ((dispatch_no = 1) = (authorized_by IS NULL))
);

COMMENT ON TABLE public.settlement_fulfillment_dispatches IS
    '结算义务的派发历史（只追加）：首寄与经退件案件授权的补寄，旧派发换代后仍可追溯与绑定退件';

INSERT INTO public.settlement_fulfillment_dispatches
    (obligation_id, dispatch_no, delivery_id, authorized_by, recorded_at)
SELECT fulfillment.obligation_id, 1, fulfillment.delivery_id, NULL, delivery.created_at
FROM public.settlement_obligation_fulfillments AS fulfillment
JOIN public.deliveries AS delivery
    ON delivery.id = fulfillment.delivery_id
   AND delivery.origin_kind = 'settlement'
   AND delivery.origin_id = fulfillment.obligation_id::text
WHERE fulfillment.delivery_id IS NOT NULL;

CREATE TABLE public.settlement_funding_delivery_bindings (
    delivery_id uuid PRIMARY KEY REFERENCES public.deliveries(id) ON DELETE RESTRICT,
    allocation_id uuid NOT NULL
        REFERENCES public.settlement_funding_allocations(id) ON DELETE RESTRICT,
    uid text NOT NULL REFERENCES public.settlement_funding_accounts(uid) ON DELETE RESTRICT,
    recorded_at timestamptz NOT NULL DEFAULT clock_timestamp() CHECK (isfinite(recorded_at))
);

CREATE INDEX settlement_funding_delivery_bindings_allocation_idx
    ON public.settlement_funding_delivery_bindings (allocation_id);

COMMENT ON TABLE public.settlement_funding_delivery_bindings IS
    '出站投递曾占用的资金分配（只追加）；分配指针换代后旧派发仍受资金保护';

INSERT INTO public.settlement_funding_delivery_bindings (delivery_id, allocation_id, uid)
SELECT allocation.delivery_id, allocation.id, allocation.uid
FROM public.settlement_funding_allocations AS allocation
WHERE allocation.delivery_id IS NOT NULL;

ALTER TABLE public.settlement_funding_returns
    ADD COLUMN return_mail_id uuid UNIQUE
        REFERENCES public.settlement_return_cases(return_mail_id) ON DELETE RESTRICT,
    ADD COLUMN take_delivery_id uuid UNIQUE
        REFERENCES public.deliveries(id) ON DELETE RESTRICT,
    ADD COLUMN source_id uuid UNIQUE
        REFERENCES public.settlement_funding_sources(id) ON DELETE RESTRICT;

-- 新返还必须带齐入站/Take/来源绑定；NOT VALID 只约束新行，迁移前的历史行（如有）原样保留。
ALTER TABLE public.settlement_funding_returns
    ADD CONSTRAINT settlement_funding_returns_take_binding_check CHECK (
        return_mail_id IS NOT NULL AND take_delivery_id IS NOT NULL AND source_id IS NOT NULL
    ) NOT VALID;

CREATE FUNCTION public.reject_settlement_dispatch_funding_mutation()
RETURNS trigger LANGUAGE plpgsql AS $function$
BEGIN
    RAISE EXCEPTION 'settlement dispatch and funding return facts are append-only'
        USING ERRCODE = '55000';
END;
$function$;

CREATE TRIGGER settlement_fulfillment_dispatches_append_only
BEFORE UPDATE OR DELETE ON public.settlement_fulfillment_dispatches
FOR EACH ROW EXECUTE FUNCTION public.reject_settlement_dispatch_funding_mutation();
CREATE TRIGGER settlement_fulfillment_dispatches_no_truncate
BEFORE TRUNCATE ON public.settlement_fulfillment_dispatches
FOR EACH STATEMENT EXECUTE FUNCTION public.reject_settlement_dispatch_funding_mutation();

CREATE TRIGGER settlement_funding_delivery_bindings_append_only
BEFORE UPDATE OR DELETE ON public.settlement_funding_delivery_bindings
FOR EACH ROW EXECUTE FUNCTION public.reject_settlement_dispatch_funding_mutation();
CREATE TRIGGER settlement_funding_delivery_bindings_no_truncate
BEFORE TRUNCATE ON public.settlement_funding_delivery_bindings
FOR EACH STATEMENT EXECUTE FUNCTION public.reject_settlement_dispatch_funding_mutation();

CREATE TRIGGER settlement_funding_returns_append_only
BEFORE UPDATE OR DELETE ON public.settlement_funding_returns
FOR EACH ROW EXECUTE FUNCTION public.reject_settlement_dispatch_funding_mutation();
CREATE TRIGGER settlement_funding_returns_no_truncate
BEFORE TRUNCATE ON public.settlement_funding_returns
FOR EACH STATEMENT EXECUTE FUNCTION public.reject_settlement_dispatch_funding_mutation();

-- 运行账号授权：settlement_funding_returns 从可变列表移入只追加列表，新增
-- settlement_funding_delivery_bindings 与 settlement_fulfillment_dispatches 只授 INSERT；其余与
-- 000114 的定义逐字一致。
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
        settlement_funding_scope, settlement_funding_sources,
        settlement_internal_commands, settlement_obligation_fulfillments, settlement_obligations,
        settlement_recoveries, settlement_recovery_actions, settlement_recovery_commands,
        settlement_recovery_health, settlement_return_actions, settlement_return_cases,
        user_admin_commands, user_blacklist,
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
        settlement_funding_delivery_bindings, settlement_funding_returns,
        settlement_fulfillment_dispatches,
        settlement_equipment_losses,
        settlement_legacy_bot_seller_claims, settlement_legacy_self_reconciliations,
        settlement_recovery_return_consumptions, settlement_return_commands,
        settlement_return_events, settlement_return_evidence, settlement_return_takes,
        settlement_return_verifications,
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
    gen_random_uuid(), 'system', 'migration-000115', 'system.migration', 'migration',
    '000115', 'success', 'system',
    jsonb_build_object(
        'summary', 'settlement dispatch history, funding delivery bindings and append-only funding returns',
        'tables', jsonb_build_array(
            'settlement_fulfillment_dispatches', 'settlement_funding_delivery_bindings',
            'settlement_funding_returns'
        ),
        'compatibility', 'breaking'
    )
);

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (115, 'breaking');
