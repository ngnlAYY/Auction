-- settlement owner：原子追踪资金来源累计发行并绑定补偿冲正的世界身份。
-- 不变量：分录保持只追加；同来源可拆分 issue，但累计 issue/return 不超过来源额度；使用后额度冻结；
--          冲正的 world、auction、lot 必须与 recovery 及 auction 的世界一致。
-- Breaking：新增累计约束、来源额度冻结与复合外键；升级前须停止全部写入角色。
LOCK TABLE
    public.auctions,
    public.compensation_pool_reversals,
    public.settlement_funding_entries,
    public.settlement_funding_sources,
    public.settlement_recoveries
IN SHARE ROW EXCLUSIVE MODE;

DO $block$
BEGIN
    IF EXISTS (
        SELECT source.id
        FROM public.settlement_funding_sources AS source
        LEFT JOIN public.settlement_funding_entries AS entry ON entry.source_id = source.id
        GROUP BY source.id, source.amount
        HAVING COALESCE(
            sum(entry.amount::numeric) FILTER (WHERE entry.kind IN ('issue', 'return')),
            0
        ) > source.amount::numeric
    ) THEN
        RAISE EXCEPTION '既有资金来源累计 issue/return 超过额度；须先人工核对'
            USING ERRCODE = '23514';
    END IF;

    IF EXISTS (
        SELECT 1
        FROM public.compensation_pool_reversals AS reversal
        LEFT JOIN public.settlement_recoveries AS recovery
          ON recovery.id = reversal.recovery_id
        LEFT JOIN public.auctions AS auction
          ON auction.id = reversal.auction_id
        WHERE recovery.id IS NULL
           OR auction.id IS NULL
           OR recovery.world_id IS DISTINCT FROM reversal.world_id
           OR auction.world_id IS DISTINCT FROM reversal.world_id
           OR recovery.auction_id IS DISTINCT FROM reversal.auction_id
           OR recovery.lot_id IS DISTINCT FROM reversal.lot_id
    ) THEN
        RAISE EXCEPTION '既有补偿池冲正与 recovery/auction 世界身份不一致；须先人工核对'
            USING ERRCODE = '23514';
    END IF;
END
$block$;

ALTER TABLE public.settlement_funding_sources
    ADD COLUMN issued_amount bigint NOT NULL DEFAULT 0,
    ADD CONSTRAINT settlement_funding_sources_issued_amount_check_000131
        CHECK (issued_amount >= 0 AND issued_amount <= amount);

UPDATE public.settlement_funding_sources AS source
SET issued_amount = totals.issued_amount
FROM (
    SELECT source.id,
           COALESCE(
               sum(entry.amount::numeric) FILTER (WHERE entry.kind IN ('issue', 'return')),
               0
           )::bigint AS issued_amount
    FROM public.settlement_funding_sources AS source
    LEFT JOIN public.settlement_funding_entries AS entry ON entry.source_id = source.id
    GROUP BY source.id
) AS totals
WHERE totals.id = source.id;

CREATE FUNCTION public.freeze_used_settlement_funding_source_amount_000131()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, public
AS $function$
BEGIN
    IF NEW.amount IS DISTINCT FROM OLD.amount
       AND (
           OLD.issued_amount > 0
           OR EXISTS (
               SELECT 1
               FROM public.settlement_funding_entries AS entry
               WHERE entry.source_id = OLD.id
                 AND entry.kind IN ('issue', 'return')
           )
       ) THEN
        RAISE EXCEPTION '已使用的资金来源额度不可改写'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END
$function$;
CREATE TRIGGER settlement_funding_source_amount_frozen_000131
BEFORE UPDATE OF amount ON public.settlement_funding_sources
FOR EACH ROW EXECUTE FUNCTION public.freeze_used_settlement_funding_source_amount_000131();

CREATE OR REPLACE FUNCTION public.enforce_settlement_funding_issue_limit_000120()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $function$
DECLARE
    source_uid text;
    source_kind text;
    source_key uuid;
    source_amount bigint;
    source_proof_digest bytea;
    counted_source uuid;
BEGIN
    IF NEW.kind NOT IN ('issue', 'return') THEN
        RETURN NEW;
    END IF;
    SELECT source.uid, source.kind, source.source_key, source.amount, source.proof_digest
    INTO source_uid, source_kind, source_key, source_amount, source_proof_digest
    FROM public.settlement_funding_sources AS source
    WHERE source.id = NEW.source_id
    FOR UPDATE;
    IF NOT FOUND OR source_uid IS DISTINCT FROM NEW.uid THEN
        RAISE EXCEPTION '资金分录来源不存在或 UID 不匹配'
            USING ERRCODE = '23514';
    END IF;
    IF NEW.kind = 'issue'
       AND source_kind NOT IN ('reconciliation', 'collected_payment')
       AND NOT (
           (source_kind = 'record_carrier'
            AND NEW.amount = 1
            AND public.settlement_record_carrier_source_valid_000120(NEW.source_id, true))
           OR (source_kind = 'record_opening'
               AND NEW.command_id = source_key
               AND NEW.proof_digest = source_proof_digest
               AND public.settlement_record_opening_credit_valid_000120(NEW.uid, NEW.credit_id))
           OR (
               source_kind IN (
                   'record_internal_purchase',
                   'record_operator_equipment_loss',
                   'record_compensation'
               )
               AND NEW.proof_digest = source_proof_digest
               AND public.settlement_record_only_source_valid_000120(
                   NEW.source_id, NEW.credit_id, true
               )
           )
       ) THEN
        RAISE EXCEPTION 'issue 来源必须匹配冻结结算与 record_only 责任事实'
            USING ERRCODE = '23514';
    END IF;
    IF NEW.kind = 'return' THEN
        IF source_kind <> 'returned_delivery' OR NEW.amount <> source_amount
           OR EXISTS (
               SELECT 1 FROM public.settlement_funding_entries AS entry
               WHERE entry.source_id = NEW.source_id
           ) THEN
            RAISE EXCEPTION 'returned_delivery 来源必须唯一发行一笔完整 return'
                USING ERRCODE = '23514';
        END IF;
    END IF;
    UPDATE public.settlement_funding_sources AS source
    SET issued_amount = source.issued_amount + NEW.amount
    WHERE source.id = NEW.source_id
      AND source.uid = NEW.uid
      AND source.issued_amount::numeric + NEW.amount::numeric <= source.amount::numeric
    RETURNING source.id INTO counted_source;
    IF counted_source IS NULL THEN
        RAISE EXCEPTION '资金来源分录超过冻结额度'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END
$function$;
REVOKE ALL ON FUNCTION public.enforce_settlement_funding_issue_limit_000120() FROM PUBLIC, auction_runtime;
REVOKE ALL ON FUNCTION public.freeze_used_settlement_funding_source_amount_000131() FROM PUBLIC, auction_runtime;

ALTER TABLE public.settlement_recoveries
    ADD CONSTRAINT settlement_recoveries_identity_world_unique_000131
        UNIQUE (id, world_id, auction_id, lot_id);
ALTER TABLE public.compensation_pool_reversals
    ADD CONSTRAINT compensation_pool_reversals_auction_world_fkey_000131
        FOREIGN KEY (auction_id, world_id)
        REFERENCES public.auctions (id, world_id) ON DELETE RESTRICT,
    ADD CONSTRAINT compensation_pool_reversals_recovery_identity_fkey_000131
        FOREIGN KEY (recovery_id, world_id, auction_id, lot_id)
        REFERENCES public.settlement_recoveries (id, world_id, auction_id, lot_id)
        ON DELETE RESTRICT;

-- 000081/000122 的授权入口原先把 funding sources 列为完整可变表，允许直接伪造计数器。
-- 保留完整授权校验与其它表矩阵，再由唯一公开入口收紧来源写入面，防止 grants 重放恢复表级 UPDATE。
ALTER FUNCTION public.apply_auction_runtime_privileges()
    RENAME TO apply_auction_runtime_privileges_000122;
CREATE FUNCTION public.apply_auction_runtime_privileges()
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $function$
DECLARE
    function_owner oid;
    grantee record;
BEGIN
    PERFORM public.apply_auction_runtime_privileges_000122();
    REVOKE INSERT, UPDATE, DELETE ON TABLE public.settlement_funding_sources FROM auction_runtime;
    REVOKE INSERT (issued_amount), UPDATE (issued_amount)
        ON TABLE public.settlement_funding_sources FROM auction_runtime;
    GRANT INSERT (id, uid, kind, source_key, amount, proof_digest)
        ON TABLE public.settlement_funding_sources TO auction_runtime;

    -- 不变量：运行账号不能执行授权链上的任何函数。`pg_restore --no-privileges` 恢复默认 ACL 后，
    -- 重放本入口必须重新收口整条链；否则运行账号可执行旧 SECURITY DEFINER 入口，
    -- 重新获得收紧前的 settlement_funding_sources 写权限。
    SELECT procedure.proowner INTO function_owner
    FROM pg_catalog.pg_proc AS procedure
    WHERE procedure.oid = 'public.apply_auction_runtime_privileges()'::pg_catalog.regprocedure;
    REVOKE ALL ON FUNCTION public.apply_auction_runtime_privileges() FROM PUBLIC;
    REVOKE ALL ON FUNCTION public.apply_auction_runtime_privileges_000122() FROM PUBLIC;
    FOR grantee IN
        SELECT role.rolname
        FROM pg_catalog.pg_roles AS role
        WHERE role.oid <> function_owner
    LOOP
        EXECUTE format(
            'REVOKE ALL ON FUNCTION public.apply_auction_runtime_privileges() FROM %I',
            grantee.rolname
        );
        EXECUTE format(
            'REVOKE ALL ON FUNCTION public.apply_auction_runtime_privileges_000122() FROM %I',
            grantee.rolname
        );
    END LOOP;
END
$function$;
REVOKE ALL ON FUNCTION public.apply_auction_runtime_privileges() FROM PUBLIC, auction_runtime;
REVOKE ALL ON FUNCTION public.apply_auction_runtime_privileges_000122() FROM PUBLIC, auction_runtime;
SELECT public.apply_auction_runtime_privileges();

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (131, 'breaking');
