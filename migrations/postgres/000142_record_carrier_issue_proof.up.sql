-- record_carrier 的 1-credit 发行必须复用冻结来源的证明摘要，与其他 record-only 来源一致。
-- owner: auction-storage
--
-- 不变量：
--   1. `settlement_funding_entries.proof_digest` 记录分录来源证明；`record_opening`、
--      `record_internal_purchase`、`record_operator_equipment_loss`、`record_compensation`
--      分支都要求 `NEW.proof_digest = source.proof_digest`（000120）。
--   2. `record_carrier` 分支只校验来源形状（isekai 付款义务 → main 冻结派发，或 000136 的寄售合同），
--      未绑定来源摘要，因此可用任意 32 字节摘要对合法来源“伪造证据”发行。
--   3. 本迁移以独立 `BEFORE INSERT` 触发器补齐该等式，不改写既有函数、历史分录或来源行；
--      存储层 `ledger.rs::issue` 以来源摘要构造分录，因此新写入天然满足。
--
-- 等级：breaking。新增会拒绝摘要不匹配的 carrier 发行，须与同版程序在停机门禁下切换。

CREATE FUNCTION public.enforce_record_carrier_issue_proof_000142()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, public
AS $function$
DECLARE
    source_kind text;
    source_digest bytea;
BEGIN
    IF NEW.kind <> 'issue' OR NEW.source_id IS NULL THEN
        RETURN NEW;
    END IF;
    SELECT source.kind, source.proof_digest
    INTO source_kind, source_digest
    FROM public.settlement_funding_sources AS source
    WHERE source.id = NEW.source_id;
    IF source_kind = 'record_carrier'
       AND NEW.proof_digest IS DISTINCT FROM source_digest THEN
        RAISE EXCEPTION 'record_carrier 发行必须引用来源证明摘要'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END
$function$;

CREATE TRIGGER settlement_funding_issue_carrier_proof_000142
BEFORE INSERT ON public.settlement_funding_entries
FOR EACH ROW EXECUTE FUNCTION public.enforce_record_carrier_issue_proof_000142();

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (142, 'breaking');
