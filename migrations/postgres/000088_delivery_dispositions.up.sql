-- W2 统一处置事实模型（决策 46）：一个只追加事实表收口投递异常终态。
-- owner: auction-domain（处置种类与准入闭集）、auction-storage（唯一 SQL 写入与读取）
--
-- 背景：legacy 自寄、装备损失、Admin 判失败、预检退役与人工 Sent 证明此前各有专用表或
--   元数据谓词，仪表盘、告警与 Admin 列表分别拼接排除条件，口径容易漂移（现状基线
--   「处置语义」）。本迁移建立单一只追加事实表，并由运行期写入方在原事务内追加处置。
--
-- 不变量：
--   1. 处置事实只追加：BEFORE UPDATE/DELETE 触发器与权限层同时拒绝改写；保留政策为永久。
--   2. 每种处置恰好一个来源引用，且与种类一一对应（CHECK）：自动 Sent 绑定、人工 Sent
--      证明命令、Admin 账本命令、装备损失命令、历史自寄修复命令、操作者确认审计。
--      预检退役以投递行自身（id、fence、失败码）为证，因此不带外部引用。
--   3. `resolves_alert` 是写入时固定的事实列，不是读侧派生：只有它解除该 fence 上的失败
--      计数。当前沿用迁移前的排除口径——预检退役中仅 `lot_watch_invalidated` 解除，改名与
--      赛季退役仍计入通知失败；放开属有意改变判定，须用户批准（决策 72）并新增迁移。
--   4. `operator_acknowledged` 由操作者命令写入：原因必填，且只允许没有资金或资产影响的
--      历史失败（领域准入，事务内重新读取后判定）；该行自身保存幂等命令 ID 与摘要。
--   5. 处置不改变投递状态、资金、履约或资产事实，也不构成发送、领取或收款证明。
--   6. 读侧只按 `resolves_alert` 与当前 fence 过滤“未处置”，不再按来源拆开排除谓词。
--   7. 运行账号按 000081/000083 的最小权限模型只获得 SELECT 与 INSERT；写入口唯一。
--
-- 等级：compatible。纯新增表、索引与初始回填，旧程序读取既有表结果不变（它不读取本表），
--   因此可在常规停机门禁下与其它迁移同批执行。
--
-- 回填：只回填能由既有不可变事实精确证明的行，其余保持未处置。回填比较沿用 W3.7 的时区
--   无关形式（legacy 自寄谓词只比较 UTC 瞬间，不依赖会话 TimeZone）。

CREATE TABLE public.delivery_dispositions (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    delivery_id uuid NOT NULL REFERENCES public.deliveries(id) ON DELETE RESTRICT,
    -- 该处置作用到的投递 fence；读侧只匹配当前 fence，人工重排后旧处置自动失效。
    fencing_version bigint NOT NULL CHECK (fencing_version >= 1),
    kind text NOT NULL CHECK (kind IN (
        'auto_sent_proof', 'operator_sent_confirmation', 'admin_failed', 'equipment_loss',
        'legacy_self', 'preflight_retired', 'operator_acknowledged'
    )),
    -- 操作者确认的幂等身份；其余种类由来源引用提供身份，为空。
    command_id uuid UNIQUE CHECK (
        command_id IS NULL OR command_id <> '00000000-0000-0000-0000-000000000000'
    ),
    request_digest bytea CHECK (request_digest IS NULL OR octet_length(request_digest) = 32),
    sent_confirmation_id uuid
        REFERENCES public.delivery_sent_confirmations(command_id) ON DELETE RESTRICT,
    admin_command_id uuid
        REFERENCES public.delivery_admin_commands(command_id) ON DELETE RESTRICT,
    equipment_loss_id uuid
        REFERENCES public.settlement_equipment_losses(command_id) ON DELETE RESTRICT,
    legacy_self_reconciliation_id uuid
        REFERENCES public.settlement_legacy_self_reconciliations(command_id) ON DELETE RESTRICT,
    sent_binding_world_id text CHECK (
        sent_binding_world_id IS NULL OR sent_binding_world_id IN ('main', 'isekai')
    ),
    sent_binding_external_mail_id text CHECK (
        sent_binding_external_mail_id IS NULL
        OR (length(sent_binding_external_mail_id) BETWEEN 1 AND 64
            AND sent_binding_external_mail_id ~ '^[1-9][0-9]*$')
    ),
    source_audit_id uuid REFERENCES public.audit_records(id) ON DELETE RESTRICT,
    -- 操作者来源如实标注（决策 17）：Admin 为共享身份，CLI 附带系统用户，Worker 与回填
    -- 分别标注 worker / system。
    actor_source text NOT NULL CHECK (actor_source IN ('admin', 'cli', 'worker', 'system')),
    actor_id text NOT NULL CHECK (
        length(actor_id) BETWEEN 1 AND 128 AND actor_id = btrim(actor_id)
        AND actor_id !~ '[[:cntrl:]]'
    ),
    reason text CHECK (
        reason IS NULL OR (length(reason) BETWEEN 1 AND 2048 AND reason = btrim(reason)
                           AND reason !~ '[[:cntrl:]]')
    ),
    reason_code text CHECK (reason_code IS NULL OR reason_code ~ '^[a-z0-9_.:-]{1,64}$'),
    resolves_alert boolean NOT NULL,
    detail jsonb NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(detail) = 'object'),
    occurred_at timestamptz NOT NULL DEFAULT clock_timestamp() CHECK (isfinite(occurred_at)),
    -- 同一投递、fence 与种类只有一条处置；重复写入幂等忽略。
    UNIQUE (delivery_id, kind, fencing_version),
    FOREIGN KEY (sent_binding_world_id, sent_binding_external_mail_id)
        REFERENCES public.delivery_sent_bindings(world_id, external_mail_id) ON DELETE RESTRICT,
    CHECK ((sent_binding_world_id IS NULL) = (sent_binding_external_mail_id IS NULL)),
    -- 恰好一个来源：预检退役以投递行为证，因此为 0。
    CHECK (num_nonnulls(sent_confirmation_id, admin_command_id, equipment_loss_id,
                        legacy_self_reconciliation_id, source_audit_id, sent_binding_world_id)
           = CASE WHEN kind = 'preflight_retired' THEN 0 ELSE 1 END),
    -- 来源与种类一一对应。
    CHECK (
        (kind = 'auto_sent_proof' AND sent_binding_world_id IS NOT NULL)
        OR (kind = 'operator_sent_confirmation' AND sent_confirmation_id IS NOT NULL)
        OR (kind = 'admin_failed' AND admin_command_id IS NOT NULL)
        OR (kind = 'equipment_loss' AND equipment_loss_id IS NOT NULL)
        OR (kind = 'legacy_self' AND legacy_self_reconciliation_id IS NOT NULL)
        OR (kind = 'operator_acknowledged' AND source_audit_id IS NOT NULL)
        OR kind = 'preflight_retired'
    ),
    CHECK ((kind = 'operator_acknowledged') = (command_id IS NOT NULL)),
    CHECK ((command_id IS NULL) = (request_digest IS NULL)),
    CHECK (kind <> 'operator_acknowledged' OR reason IS NOT NULL),
    CHECK (kind <> 'preflight_retired' OR (
        reason_code IS NOT NULL AND reason_code IN (
            'lot_watch_invalidated', 'recipient_renamed',
            'auction_notice_invalidated', 'season_notice_invalidated'
        )
    )),
    -- 解除判定与领域闭集一致：只有预检退役可带“记录但不解除”，且只有关注提醒失效解除。
    CHECK (
        (kind IN ('auto_sent_proof', 'operator_sent_confirmation', 'admin_failed',
                  'equipment_loss', 'legacy_self', 'operator_acknowledged') AND resolves_alert)
        OR (kind = 'preflight_retired'
            AND resolves_alert = (reason_code = 'lot_watch_invalidated'))
    )
);

COMMENT ON TABLE public.delivery_dispositions IS
    '投递异常终态的统一处置事实（只追加）；不改变投递状态、资金、履约或资产，也不构成发送、领取或收款证明';

CREATE FUNCTION public.reject_delivery_disposition_mutation() RETURNS trigger
LANGUAGE plpgsql AS $function$
BEGIN
    RAISE EXCEPTION 'delivery dispositions are append only' USING ERRCODE = '55000';
END;
$function$;

CREATE TRIGGER delivery_dispositions_append_only
BEFORE UPDATE OR DELETE ON public.delivery_dispositions
FOR EACH ROW EXECUTE FUNCTION public.reject_delivery_disposition_mutation();

-- 读侧只按 (delivery_id, 当前 fence, resolves_alert) 过滤，部分索引与谓词一致。
CREATE INDEX delivery_dispositions_resolving_idx
    ON public.delivery_dispositions (delivery_id, fencing_version)
    WHERE resolves_alert;

CREATE INDEX delivery_dispositions_kind_idx ON public.delivery_dispositions (kind);

-- ── 回填：只回填能由既有不可变事实精确证明的行 ──────────────────────────────────────

-- 1) 历史自寄核对：只接受与原投递完整身份相互匹配的修复记录（W3.7 时区无关比较）。
INSERT INTO public.delivery_dispositions
    (delivery_id, fencing_version, kind, legacy_self_reconciliation_id, actor_source, actor_id,
     reason, reason_code, resolves_alert, detail, occurred_at)
SELECT r.delivery_id, d.fencing_version, 'legacy_self', r.command_id, 'system', 'migration-000088',
       r.reason, 'legacy_self_reconciliation', true,
       jsonb_build_object('backfilled', true,
           'derived_from', 'settlement_legacy_self_reconciliations', 'action', r.action),
       r.created_at
FROM public.settlement_legacy_self_reconciliations AS r
JOIN public.deliveries AS d ON d.id = r.delivery_id
WHERE 
    r.delivery_id=d.id AND d.status='failed'
    AND d.origin_kind='settlement' AND d.origin_id=r.obligation_id::text
    AND d.fencing_version=r.expected_delivery_fencing+1
    AND d.command_id::text=r.original_snapshot#>>'{delivery,command_id}'
    AND to_jsonb(d.request_digest)=r.original_snapshot#>'{delivery,request_digest}'
    AND coalesce(to_jsonb(d.activation_command_id),'null'::jsonb)=r.original_snapshot#>'{delivery,activation_command_id}'
    AND coalesce(to_jsonb(d.activation_request_digest),'null'::jsonb)=r.original_snapshot#>'{delivery,activation_request_digest}'
    AND d.payload=r.original_snapshot#>'{delivery,payload}'
    AND d.recipient=r.original_snapshot#>>'{delivery,recipient}'
    AND d.failure_code=r.original_snapshot#>>'{delivery,failure_code}'
    AND d.external_reference IS NULL
    -- 会话时区无关的整行比较：时间列统一按 UTC 瞬间渲染。
    AND (r.original_snapshot->'delivery') ?& ARRAY[
        'lease_expires_at','claimed_at','sending_at','completed_at','activated_at','created_at']
    AND ((to_jsonb(d) || jsonb_build_object(
            'lease_expires_at',coalesce(to_jsonb(d.lease_expires_at AT TIME ZONE 'UTC'),'null'::jsonb),
            'claimed_at',coalesce(to_jsonb(d.claimed_at AT TIME ZONE 'UTC'),'null'::jsonb),
            'sending_at',coalesce(to_jsonb(d.sending_at AT TIME ZONE 'UTC'),'null'::jsonb),
            'completed_at',coalesce(to_jsonb(d.completed_at AT TIME ZONE 'UTC'),'null'::jsonb),
            'activated_at',coalesce(to_jsonb(d.activated_at AT TIME ZONE 'UTC'),'null'::jsonb),
            'created_at',coalesce(to_jsonb(d.created_at AT TIME ZONE 'UTC'),'null'::jsonb)))
        -'status'-'fencing_version'-'updated_at'-'result_metadata')=
        ((r.original_snapshot->'delivery' || jsonb_build_object(
            'lease_expires_at',coalesce(to_jsonb((r.original_snapshot#>>'{delivery,lease_expires_at}')::timestamptz AT TIME ZONE 'UTC'),'null'::jsonb),
            'claimed_at',coalesce(to_jsonb((r.original_snapshot#>>'{delivery,claimed_at}')::timestamptz AT TIME ZONE 'UTC'),'null'::jsonb),
            'sending_at',coalesce(to_jsonb((r.original_snapshot#>>'{delivery,sending_at}')::timestamptz AT TIME ZONE 'UTC'),'null'::jsonb),
            'completed_at',coalesce(to_jsonb((r.original_snapshot#>>'{delivery,completed_at}')::timestamptz AT TIME ZONE 'UTC'),'null'::jsonb),
            'activated_at',coalesce(to_jsonb((r.original_snapshot#>>'{delivery,activated_at}')::timestamptz AT TIME ZONE 'UTC'),'null'::jsonb),
            'created_at',coalesce(to_jsonb((r.original_snapshot#>>'{delivery,created_at}')::timestamptz AT TIME ZONE 'UTC'),'null'::jsonb)))
        -'status'-'fencing_version'-'updated_at'-'result_metadata')
    AND (d.result_metadata-'legacy_self_reconciliation')=r.original_snapshot#>'{delivery,result_metadata}'
    AND d.result_metadata->'legacy_self_reconciliation'->>'command_id'=r.command_id::text
    AND r.evidence->'identity'->>'uid'=r.uid
    AND r.evidence->'identity'->>'player_name'=d.recipient
    AND r.original_snapshot#>>'{obligation,id}'=r.obligation_id::text
    AND r.original_snapshot#>>'{fulfillment,obligation_id}'=r.obligation_id::text
    AND r.original_snapshot#>>'{fulfillment,delivery_id}'=r.delivery_id::text
    AND r.original_snapshot#>>'{fulfillment,execution_state}'='manual_reconcile'
    AND r.original_snapshot#>>'{delivery,status}'='manual_reconcile'
    AND (r.action='keep_unsold' OR (r.action='reopen_payment'
        AND d.failure_code='recipient_is_sender'
        AND r.original_snapshot#>>'{delivery,result_metadata,compose_discard,outcome}'='discarded'))
;

-- 2) Admin 对账判失败：命令账本中产出当前 fence 的失败命令。
INSERT INTO public.delivery_dispositions
    (delivery_id, fencing_version, kind, admin_command_id, actor_source, actor_id,
     reason_code, resolves_alert, detail, occurred_at)
SELECT DISTINCT ON (d.id, d.fencing_version)
       d.id, d.fencing_version, 'admin_failed', admin.command_id, 'system', 'migration-000088',
       NULL, true,
       jsonb_build_object('backfilled', true, 'derived_from', 'delivery_admin_commands',
           'command_kind', admin.command_kind),
       admin.created_at
FROM public.deliveries AS d
JOIN public.delivery_admin_commands AS admin ON admin.delivery_id = d.id
WHERE d.status = 'failed'
  AND admin.command_kind = 'reconcile'
  AND admin.result_payload ->> 'Status' = 'failed'
  AND admin.result_payload -> 'FencingVersion' = to_jsonb(d.fencing_version)
ORDER BY d.id, d.fencing_version, admin.created_at, admin.command_id;

-- 3) 装备损失：原投递被永久退役，失败码与损失事实必须同时成立。
INSERT INTO public.delivery_dispositions
    (delivery_id, fencing_version, kind, equipment_loss_id, actor_source, actor_id,
     reason, reason_code, resolves_alert, detail, occurred_at)
SELECT loss.delivery_id, d.fencing_version, 'equipment_loss', loss.command_id, 'system',
       'migration-000088', loss.reason, 'equipment_destroyed', true,
       jsonb_build_object('backfilled', true, 'derived_from', 'settlement_equipment_losses',
           'lot_id', loss.lot_id, 'recovery_id', loss.recovery_id),
       loss.created_at
FROM public.settlement_equipment_losses AS loss
JOIN public.deliveries AS d ON d.id = loss.delivery_id
WHERE d.status = 'failed' AND d.failure_code = 'equipment_destroyed';

-- 4a) 预检退役且沿用迁移前的失败排除口径：关注提醒在发送围栏前失去资格。
INSERT INTO public.delivery_dispositions
    (delivery_id, fencing_version, kind, actor_source, actor_id, reason_code, resolves_alert,
     detail, occurred_at)
SELECT d.id, d.fencing_version, 'preflight_retired', 'system', 'migration-000088',
       'lot_watch_invalidated', true,
       jsonb_build_object('backfilled', true, 'derived_from', 'deliveries',
           'reason_code', d.result_metadata ->> 'reason_code'),
       COALESCE(d.completed_at, d.updated_at)
FROM public.deliveries AS d
WHERE d.status = 'failed'
  AND d.origin_kind = 'lot_watch'
  AND d.failure_code = 'lot_watch_invalidated'
  AND d.sending_at IS NULL
  AND d.result_metadata @>
      '{"outcome_unknown":false,"preflight_blocked":true,"reason_code":"lot_watch_invalidated"}'::jsonb;

-- 4b) 其余预检退役（改名、投稿场次已开拍、赛季边界已过）：记录事实但不解除失败计数，
--     保持迁移前通知失败口径不变。
INSERT INTO public.delivery_dispositions
    (delivery_id, fencing_version, kind, actor_source, actor_id, reason_code, resolves_alert,
     detail, occurred_at)
SELECT d.id, d.fencing_version, 'preflight_retired', 'system', 'migration-000088',
       d.failure_code, false,
       jsonb_build_object('backfilled', true, 'derived_from', 'deliveries',
           'reason_code', d.result_metadata ->> 'reason_code'),
       COALESCE(d.completed_at, d.updated_at)
FROM public.deliveries AS d
WHERE d.status = 'failed'
  AND d.sending_at IS NULL
  AND d.result_metadata @> '{"preflight_blocked":true}'::jsonb
  AND d.failure_code IN ('recipient_renamed', 'auction_notice_invalidated',
                         'season_notice_invalidated');

-- 5) 人工 Sent 证明：只回填仍停在原 fence 且已成功的确认。
INSERT INTO public.delivery_dispositions
    (delivery_id, fencing_version, kind, sent_confirmation_id, actor_source, actor_id,
     reason, reason_code, resolves_alert, detail, occurred_at)
SELECT confirmation.delivery_id, confirmation.expected_fencing_version,
       'operator_sent_confirmation', confirmation.command_id, 'system', 'migration-000088',
       confirmation.reason, 'sent_operator_confirmed_v1', true,
       jsonb_build_object('backfilled', true, 'derived_from', 'delivery_sent_confirmations',
           'sending_fencing_version', confirmation.sending_fencing_version),
       confirmation.confirmed_at
FROM public.delivery_sent_confirmations AS confirmation
JOIN public.deliveries AS d
  ON d.id = confirmation.delivery_id
 AND d.fencing_version = confirmation.expected_fencing_version
WHERE d.status = 'succeeded';

-- 6) 自动 Sent 恢复：只回填自身携带恢复证明并与远端绑定一致的已成功投递。
INSERT INTO public.delivery_dispositions
    (delivery_id, fencing_version, kind, sent_binding_world_id, sent_binding_external_mail_id,
     actor_source, actor_id, reason_code, resolves_alert, detail, occurred_at)
SELECT DISTINCT ON (binding.delivery_id, delivery.fencing_version)
       binding.delivery_id, delivery.fencing_version, 'auto_sent_proof', binding.world_id,
       binding.external_mail_id, 'system', 'migration-000088',
       delivery.result_metadata ->> 'proof', true,
       jsonb_build_object('backfilled', true, 'derived_from', 'delivery_sent_bindings'),
       COALESCE(binding.proven_at, delivery.completed_at)
FROM public.delivery_sent_bindings AS binding
JOIN public.deliveries AS delivery ON delivery.id = binding.delivery_id
WHERE delivery.status = 'succeeded'
  AND delivery.result_metadata ->> 'proof' IN
      ('sent_recovered_frozen_v1', 'sent_recovered_reference_v1', 'sent_recovered_notice_v1')
ORDER BY binding.delivery_id, delivery.fencing_version, binding.proven_at DESC;

-- ── 运行账号授权：只追加表只有 SELECT 与 INSERT ─────────────────────────────────────

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
    gen_random_uuid(), 'system', 'migration-000088', 'system.migration', 'migration',
    '000088', 'success', 'system',
    jsonb_build_object(
        'summary', 'unified delivery disposition facts',
        'table', 'delivery_dispositions',
        'kinds', jsonb_build_array(
            'auto_sent_proof', 'operator_sent_confirmation', 'admin_failed', 'equipment_loss',
            'legacy_self', 'preflight_retired', 'operator_acknowledged'),
        'compatibility', 'compatible'
    )
);

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (88, 'compatible');
