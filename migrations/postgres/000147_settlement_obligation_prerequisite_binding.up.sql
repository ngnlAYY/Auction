-- 结算义务的前置义务必须属于同一来源身份，且前置链不得成环或无限增长。
-- owner: auction-storage
--
-- 不变量：
--   1. 一致性检查器以 `auction_id`/`lot_id`/`equipment_id`/`contract_id` 判定前置身份；
--      000002 与 000135 只声明可空外键，没有把该身份写入数据库层。
--   2. 前置链最深 64 层；超过边界或回到自身时，检查器分别报告 `obligation_cycle` 与
--      `obligation_chain_depth_limit`。
--   3. 延迟约束触发器在提交前校验；既有行不回填，新写入立即受约束。
--
-- 等级：breaking。新增会拒绝旧程序可能写入的跨身份或成环前置关系，须与同版程序一起切换。

CREATE FUNCTION public.validate_settlement_obligation_prerequisite_000147()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, public
AS $function$
DECLARE
    parent_auction_id uuid;
    parent_lot_id uuid;
    parent_equipment_id uuid;
    parent_contract_id uuid;
    cursor_id uuid;
    depth integer := 0;
BEGIN
    IF NEW.prerequisite_obligation_id IS NULL THEN
        RETURN NULL;
    END IF;

    SELECT auction_id, lot_id, equipment_id, contract_id
    INTO parent_auction_id, parent_lot_id, parent_equipment_id, parent_contract_id
    FROM public.settlement_obligations
    WHERE id = NEW.prerequisite_obligation_id;
    IF NOT FOUND THEN
        RETURN NULL;
    END IF;

    IF parent_auction_id IS DISTINCT FROM NEW.auction_id
       OR parent_lot_id IS DISTINCT FROM NEW.lot_id
       OR parent_equipment_id IS DISTINCT FROM NEW.equipment_id
       OR parent_contract_id IS DISTINCT FROM NEW.contract_id THEN
        RAISE EXCEPTION '结算义务的前置义务身份不一致'
            USING ERRCODE = '23514';
    END IF;

    cursor_id := NEW.prerequisite_obligation_id;
    WHILE cursor_id IS NOT NULL LOOP
        IF cursor_id = NEW.id THEN
            RAISE EXCEPTION '结算义务的前置链成环'
                USING ERRCODE = '23514';
        END IF;
        depth := depth + 1;
        IF depth > 64 THEN
            RAISE EXCEPTION '结算义务的前置链超过 64 层'
                USING ERRCODE = '23514';
        END IF;
        SELECT prerequisite_obligation_id
        INTO cursor_id
        FROM public.settlement_obligations
        WHERE id = cursor_id;
    END LOOP;
    RETURN NULL;
END
$function$;
REVOKE ALL ON FUNCTION public.validate_settlement_obligation_prerequisite_000147()
    FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.validate_settlement_obligation_prerequisite_000147()
    TO auction_runtime;

CREATE CONSTRAINT TRIGGER settlement_obligations_prerequisite_000147
AFTER INSERT OR UPDATE ON public.settlement_obligations
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION public.validate_settlement_obligation_prerequisite_000147();

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (147, 'breaking');
