-- 结算退件案件：每封退件证据一个可变案件投影，处理命令、事件与领回事实只追加，持久 Take 以
-- 受限来源 `settlement_return_action` 经 delivery owner 执行。
-- owner: auction-domain（案件状态、动作准入与补寄上限）、auction-application（端口与证据判定）、
--   auction-storage（唯一 SQL 写入与读取）
--
-- 不变量：
--   1. 案件以入站证据唯一：`settlement_return_cases` 以 `return_mail_id` 为主键并引用
--      `settlement_return_evidence`（包括 untrusted/unbound/fulfillment_terminal）；本迁移为已有证据
--      补建案件与开案事件，不改写证据。
--   2. 命令按 `command_id` 幂等：同 ID 同摘要返回原结果，同 ID 不同摘要冲突；命令、事件与领回事实
--      只追加，触发器拒绝 UPDATE/DELETE/TRUNCATE，运行账号只获 INSERT。案件与 Take 动作是投影，
--      按 `case_version`/`state_version` 乐观并发。
--   3. 每个原派发投递最多被一个案件绑定；每个案件最多一个 Take，每封退件邮件与每个原派发最多
--      一条领回事实；Take 投递与案件一一对应。
--   4. 补寄上限在建案时冻结进案件行（`resend_limit`，领域常量 3，不含首寄）。
--   5. 新 delivery 来源 `settlement_return_action` 只做 Take；本地先提交动作与审计，外部 Take 由
--      投递 Worker 在租约与发送围栏后执行；结果未知不自动重试。
--
-- 兼容等级 breaking：旧程序不认识新来源 `settlement_return_action`，领取到这类 Take 投递会按未知
--   来源失败关闭；旧 Worker 也不参与新的同邮件互斥与案件准入。回退旧程序前须先确认没有未终结的
--   `settlement_return_action` 投递（见 ops/deploy/RECOVERY.md 的回退要求）。

LOCK TABLE public.deliveries IN SHARE ROW EXCLUSIVE MODE;

ALTER TABLE public.deliveries DROP CONSTRAINT deliveries_origin_kind_check;
ALTER TABLE public.deliveries ADD CONSTRAINT deliveries_origin_kind_check CHECK (
    origin_kind IN (
        'settlement',
        'settlement_recovery_action',
        'settlement_return_action',
        'user_key',
        'manual',
        'mail_intake',
        'operations_alert',
        'lot_watch',
        'seller_summary',
        'season_reminder'
    )
);

CREATE TABLE public.settlement_return_cases (
    return_mail_id uuid PRIMARY KEY
        REFERENCES public.settlement_return_evidence(return_mail_id) ON DELETE RESTRICT,
    world_id text NOT NULL REFERENCES public.worlds(id) ON DELETE RESTRICT,
    -- 证据义务；未知义务时为空。
    obligation_id uuid REFERENCES public.settlement_obligations(id) ON DELETE RESTRICT,
    -- 已绑定的原派发投递与派发编号；隔离记录为空。
    delivery_id uuid REFERENCES public.deliveries(id) ON DELETE RESTRICT,
    dispatch_no integer CHECK (dispatch_no IS NULL OR dispatch_no >= 1),
    status text NOT NULL CHECK (
        status IN (
            'quarantined', 'recheck_pending', 'awaiting_decision', 'take_pending',
            'take_unknown', 'resend_pending', 'held', 'resent', 'inbox_closed', 'reviewed'
        )
    ),
    -- 当前判定原因码：建案时取自证据，重新核验改绑时更新；闭集由应用层 `SettlementReturnReason`
    -- 拥有，这里只校验形状。
    reason_code text NOT NULL CHECK (reason_code ~ '^[a-z][a-z0-9_]{0,63}$'),
    plan text CHECK (plan IS NULL OR plan IN ('resend', 'hold')),
    resend_limit smallint NOT NULL CHECK (resend_limit BETWEEN 0 AND 100),
    -- 本案授权的补寄派发编号。
    resend_dispatch_no integer CHECK (resend_dispatch_no IS NULL OR resend_dispatch_no >= 2),
    case_version bigint NOT NULL DEFAULT 1 CHECK (case_version > 0),
    recheck_claim_token uuid,
    recheck_claim_expires_at timestamptz,
    recheck_count integer NOT NULL DEFAULT 0 CHECK (recheck_count >= 0),
    created_at timestamptz NOT NULL CHECK (isfinite(created_at)),
    updated_at timestamptz NOT NULL CHECK (isfinite(updated_at)),
    closed_at timestamptz,
    CHECK (updated_at >= created_at),
    CHECK ((delivery_id IS NULL) = (dispatch_no IS NULL)),
    CHECK (delivery_id IS NULL OR obligation_id IS NOT NULL),
    CHECK (
        (status IN ('quarantined', 'recheck_pending', 'reviewed') AND delivery_id IS NULL)
        OR (status NOT IN ('quarantined', 'recheck_pending', 'reviewed')
            AND delivery_id IS NOT NULL)
    ),
    CHECK (
        (status IN ('held', 'resent', 'inbox_closed', 'reviewed')) = (closed_at IS NOT NULL)
    ),
    CHECK (closed_at IS NULL OR closed_at BETWEEN created_at AND updated_at),
    CHECK (
        (status IN ('take_pending', 'take_unknown', 'resend_pending', 'held', 'resent'))
            = (plan IS NOT NULL)
    ),
    CHECK ((status IN ('resend_pending', 'resent')) = (resend_dispatch_no IS NOT NULL)),
    CHECK ((recheck_claim_token IS NULL) = (recheck_claim_expires_at IS NULL)),
    CHECK (recheck_claim_token IS NULL OR status = 'recheck_pending')
);

COMMENT ON TABLE public.settlement_return_cases IS
    '结算退件案件投影（每封退件证据一个）；终结只表示本次退件处理完成，不消灭原权利人的权益';

-- 一个原派发最多被一个案件绑定；一封邮件只会被退回一次。
CREATE UNIQUE INDEX settlement_return_cases_delivery_once
    ON public.settlement_return_cases (delivery_id)
    WHERE delivery_id IS NOT NULL;

CREATE INDEX settlement_return_cases_obligation_idx
    ON public.settlement_return_cases (obligation_id)
    WHERE obligation_id IS NOT NULL;

-- Worker 扫描与 Admin 待办只读未结案案件。
CREATE INDEX settlement_return_cases_open_idx
    ON public.settlement_return_cases (status, updated_at, return_mail_id)
    WHERE closed_at IS NULL;

CREATE TABLE public.settlement_return_commands (
    command_id uuid PRIMARY KEY
        CHECK (command_id <> '00000000-0000-0000-0000-000000000000'),
    return_mail_id uuid NOT NULL
        REFERENCES public.settlement_return_cases(return_mail_id) ON DELETE RESTRICT,
    action text NOT NULL CHECK (
        action IN (
            'reclaim_and_resend', 'reclaim_and_hold', 'resend_from_custody', 'close_inbox',
            'acknowledge', 'recheck'
        )
    ),
    request_digest bytea NOT NULL CHECK (octet_length(request_digest) = 32),
    actor_id text NOT NULL CHECK (
        actor_id = btrim(actor_id) AND actor_id <> '' AND octet_length(actor_id) <= 128
    ),
    expected_version bigint NOT NULL CHECK (expected_version > 0),
    result_version bigint NOT NULL CHECK (result_version > expected_version),
    result_status text NOT NULL CHECK (
        result_status IN (
            'quarantined', 'recheck_pending', 'awaiting_decision', 'take_pending',
            'take_unknown', 'resend_pending', 'held', 'resent', 'inbox_closed', 'reviewed'
        )
    ),
    created_at timestamptz NOT NULL CHECK (isfinite(created_at))
);

CREATE TABLE public.settlement_return_events (
    return_mail_id uuid NOT NULL
        REFERENCES public.settlement_return_cases(return_mail_id) ON DELETE RESTRICT,
    case_version bigint NOT NULL CHECK (case_version > 0),
    event_kind text NOT NULL CHECK (
        event_kind IN (
            'opened', 'acknowledged', 'recheck_requested', 'recheck_rebound',
            'recheck_unchanged', 'take_requested', 'take_activated', 'take_proven',
            'take_unknown', 'held', 'resend_authorized', 'resent', 'inbox_closed'
        )
    ),
    -- 事件之后的案件状态。
    status text NOT NULL CHECK (
        status IN (
            'quarantined', 'recheck_pending', 'awaiting_decision', 'take_pending',
            'take_unknown', 'resend_pending', 'held', 'resent', 'inbox_closed', 'reviewed'
        )
    ),
    command_id uuid REFERENCES public.settlement_return_commands(command_id) ON DELETE RESTRICT,
    actor_id text CHECK (
        actor_id IS NULL
        OR (actor_id = btrim(actor_id) AND actor_id <> '' AND octet_length(actor_id) <= 128)
    ),
    details jsonb NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(details) = 'object'),
    occurred_at timestamptz NOT NULL CHECK (isfinite(occurred_at)),
    PRIMARY KEY (return_mail_id, case_version),
    CHECK ((command_id IS NULL) = (actor_id IS NULL))
);

CREATE TABLE public.settlement_return_actions (
    return_mail_id uuid PRIMARY KEY
        REFERENCES public.settlement_return_cases(return_mail_id) ON DELETE RESTRICT,
    world_id text NOT NULL REFERENCES public.worlds(id) ON DELETE RESTRICT,
    source_external_mail_id text NOT NULL
        CHECK (source_external_mail_id ~ '^[1-9][0-9]{0,63}$'),
    expected_credits bigint NOT NULL CHECK (expected_credits >= 0),
    expected_cod bigint NOT NULL CHECK (expected_cod >= 0),
    expected_attachment_ids text[] NOT NULL
        CHECK (cardinality(expected_attachment_ids) <= 100),
    execution_state text NOT NULL CHECK (
        execution_state IN ('pending', 'enqueued', 'satisfied', 'manual_reconcile')
    ),
    delivery_id uuid UNIQUE REFERENCES public.deliveries(id) ON DELETE RESTRICT,
    prepare_command_id uuid UNIQUE,
    prepare_request_digest bytea CHECK (
        prepare_request_digest IS NULL OR octet_length(prepare_request_digest) = 32
    ),
    activate_command_id uuid UNIQUE,
    activate_request_digest bytea CHECK (
        activate_request_digest IS NULL OR octet_length(activate_request_digest) = 32
    ),
    failure_code text CHECK (failure_code IS NULL OR failure_code ~ '^[a-z][a-z0-9_]{0,63}$'),
    state_version bigint NOT NULL DEFAULT 1 CHECK (state_version > 0),
    created_at timestamptz NOT NULL CHECK (isfinite(created_at)),
    updated_at timestamptz NOT NULL CHECK (isfinite(updated_at)),
    completed_at timestamptz,
    CHECK (updated_at >= created_at),
    CHECK ((execution_state = 'pending') = (delivery_id IS NULL)),
    CHECK (
        (delivery_id IS NULL AND prepare_command_id IS NULL AND prepare_request_digest IS NULL
            AND activate_command_id IS NULL AND activate_request_digest IS NULL)
        OR (delivery_id IS NOT NULL AND prepare_command_id IS NOT NULL
            AND prepare_request_digest IS NOT NULL AND activate_command_id IS NOT NULL
            AND activate_request_digest IS NOT NULL)
    ),
    CHECK ((execution_state IN ('satisfied', 'manual_reconcile')) = (completed_at IS NOT NULL)),
    CHECK ((execution_state = 'manual_reconcile') = (failure_code IS NOT NULL))
);

-- Worker 只扫描未终结的 Take。
CREATE INDEX settlement_return_actions_open_idx
    ON public.settlement_return_actions (execution_state, updated_at, return_mail_id)
    WHERE execution_state IN ('pending', 'enqueued');

CREATE TABLE public.settlement_return_takes (
    return_mail_id uuid PRIMARY KEY
        REFERENCES public.settlement_return_cases(return_mail_id) ON DELETE RESTRICT,
    take_delivery_id uuid NOT NULL UNIQUE REFERENCES public.deliveries(id) ON DELETE RESTRICT,
    -- 被退回的原派发：同一派发的资产只能领回一次。
    original_delivery_id uuid NOT NULL UNIQUE
        REFERENCES public.deliveries(id) ON DELETE RESTRICT,
    obligation_id uuid NOT NULL REFERENCES public.settlement_obligations(id) ON DELETE RESTRICT,
    world_id text NOT NULL REFERENCES public.worlds(id) ON DELETE RESTRICT,
    external_mail_id text NOT NULL CHECK (external_mail_id ~ '^[1-9][0-9]{0,63}$'),
    credits bigint NOT NULL CHECK (credits >= 0),
    cod bigint NOT NULL CHECK (cod >= 0),
    attachment_ids text[] NOT NULL CHECK (cardinality(attachment_ids) <= 100),
    proof_digest bytea NOT NULL CHECK (octet_length(proof_digest) = 32),
    proven_at timestamptz NOT NULL CHECK (isfinite(proven_at)),
    UNIQUE (world_id, external_mail_id)
);

COMMENT ON TABLE public.settlement_return_takes IS
    '结算退件精确领回证明的消费事实（只追加）；资金返还与代管责任都以它为准，只消费一次';

CREATE FUNCTION public.reject_settlement_return_fact_mutation()
RETURNS trigger LANGUAGE plpgsql AS $function$
BEGIN
    RAISE EXCEPTION 'settlement return facts are append-only' USING ERRCODE = '55000';
END;
$function$;

CREATE TRIGGER settlement_return_commands_append_only
BEFORE UPDATE OR DELETE ON public.settlement_return_commands
FOR EACH ROW EXECUTE FUNCTION public.reject_settlement_return_fact_mutation();
CREATE TRIGGER settlement_return_commands_no_truncate
BEFORE TRUNCATE ON public.settlement_return_commands
FOR EACH STATEMENT EXECUTE FUNCTION public.reject_settlement_return_fact_mutation();

CREATE TRIGGER settlement_return_events_append_only
BEFORE UPDATE OR DELETE ON public.settlement_return_events
FOR EACH ROW EXECUTE FUNCTION public.reject_settlement_return_fact_mutation();
CREATE TRIGGER settlement_return_events_no_truncate
BEFORE TRUNCATE ON public.settlement_return_events
FOR EACH STATEMENT EXECUTE FUNCTION public.reject_settlement_return_fact_mutation();

CREATE TRIGGER settlement_return_takes_append_only
BEFORE UPDATE OR DELETE ON public.settlement_return_takes
FOR EACH ROW EXECUTE FUNCTION public.reject_settlement_return_fact_mutation();
CREATE TRIGGER settlement_return_takes_no_truncate
BEFORE TRUNCATE ON public.settlement_return_takes
FOR EACH STATEMENT EXECUTE FUNCTION public.reject_settlement_return_fact_mutation();

-- 为已有证据补建案件：已绑定的历史证据都是首寄（派发编号 1），同一原派发只保留最早一封的
-- 绑定，其余按未绑定隔离；不改写证据。
INSERT INTO public.settlement_return_cases
    (return_mail_id, world_id, obligation_id, delivery_id, dispatch_no, status, reason_code,
     resend_limit, created_at, updated_at)
SELECT evidence.return_mail_id,
       inbound.world_id,
       evidence.obligation_id,
       CASE WHEN bound.first_return THEN evidence.delivery_id END,
       CASE WHEN bound.first_return THEN 1 END,
       CASE WHEN bound.first_return THEN 'awaiting_decision' ELSE 'quarantined' END,
       CASE WHEN evidence.delivery_id IS NULL OR bound.first_return THEN evidence.reason_code
            ELSE 'dispatch_already_returned' END,
       3,
       evidence.recorded_at,
       evidence.recorded_at
FROM public.settlement_return_evidence AS evidence
JOIN public.inbound_mail AS inbound ON inbound.id = evidence.return_mail_id
CROSS JOIN LATERAL (
    SELECT evidence.delivery_id IS NOT NULL
           AND NOT EXISTS (
               SELECT 1 FROM public.settlement_return_evidence AS earlier
               WHERE earlier.delivery_id = evidence.delivery_id
                 AND (earlier.recorded_at, earlier.return_mail_id)
                     < (evidence.recorded_at, evidence.return_mail_id)
           ) AS first_return
) AS bound;

INSERT INTO public.settlement_return_events
    (return_mail_id, case_version, event_kind, status, details, occurred_at)
SELECT return_mail_id, 1, 'opened', status, jsonb_build_object('backfilled', true), created_at
FROM public.settlement_return_cases;

-- 运行账号授权：可变投影加入 settlement_return_cases、settlement_return_actions，只追加事实加入
-- settlement_return_commands、settlement_return_events、settlement_return_takes；其余与 000111 的定义逐字一致。
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
        settlement_equipment_losses,
        settlement_legacy_bot_seller_claims, settlement_legacy_self_reconciliations,
        settlement_recovery_return_consumptions, settlement_return_commands,
        settlement_return_events, settlement_return_evidence, settlement_return_takes,
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
    gen_random_uuid(), 'system', 'migration-000114', 'system.migration', 'migration',
    '000114', 'success', 'system',
    jsonb_build_object(
        'summary', 'settlement return cases, append-only facts and the settlement_return_action origin',
        'tables', jsonb_build_array(
            'settlement_return_cases', 'settlement_return_commands', 'settlement_return_events',
            'settlement_return_actions', 'settlement_return_takes'
        ),
        'compatibility', 'breaking'
    )
);

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (114, 'breaking');
