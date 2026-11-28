-- lot 的两个父对象都是世界内的，且慢速代理授权必须属于本 lot；在写入时补数据库层守卫。
-- owner: auction-storage
--
-- 不变量：
--   1. `lots` 只引用同世界的 `auctions` 与 `equipment`（两者都有 `world_id`）。此前只校验
--      各自存在，跨世界装备可被挂到拍品上，污染按世界聚合的竞价与结算。
--   2. `lots.slow_authorization_id` 指向的 `bid_proxy_authorizations` 必须属于本 lot 且
--      `mode = 'slow'`。此前外键只验证授权存在，可把别的 lot 的（甚至 fast 模式）授权记为
--      本 lot 的慢速授权。
--   3. 两个守卫只拒绝新写入/改绑；既有行保持原样，不猜测修复。迁移不改写任何业务行。
--   4. 慢速授权绑定用延迟约束触发器，与既有 `DEFERRABLE INITIALLY DEFERRED` 外键同一时机
--      校验，允许同一事务先更新 lot、后落授权行。
--
-- 等级：breaking。新增会拒绝旧程序可能写入的跨世界/跨 lot 绑定，须在停机门禁下与同版程序切换。

CREATE FUNCTION public.enforce_lot_world_consistency_000140()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, public
AS $function$
DECLARE
    auction_world text;
    equipment_world text;
BEGIN
    IF TG_OP = 'UPDATE'
       AND NEW.auction_id IS NOT DISTINCT FROM OLD.auction_id
       AND NEW.equipment_id IS NOT DISTINCT FROM OLD.equipment_id THEN
        RETURN NEW;
    END IF;

    SELECT auction.world_id INTO auction_world
    FROM public.auctions AS auction
    WHERE auction.id = NEW.auction_id;
    SELECT equipment.world_id INTO equipment_world
    FROM public.equipment AS equipment
    WHERE equipment.id = NEW.equipment_id;

    -- 缺失父行由外键失败关闭，这里只比较两个真实存在的世界。
    IF auction_world IS NOT NULL
       AND equipment_world IS NOT NULL
       AND auction_world IS DISTINCT FROM equipment_world THEN
        RAISE EXCEPTION 'lot auction and equipment must belong to the same world'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END
$function$;

CREATE TRIGGER lots_world_consistency_000140
BEFORE INSERT OR UPDATE OF auction_id, equipment_id ON public.lots
FOR EACH ROW EXECUTE FUNCTION public.enforce_lot_world_consistency_000140();

CREATE FUNCTION public.enforce_lot_slow_authorization_owner_000140()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, public
AS $function$
DECLARE
    authorization_lot_id uuid;
    authorization_mode text;
BEGIN
    IF NEW.slow_authorization_id IS NULL THEN
        RETURN NULL;
    END IF;
    IF TG_OP = 'UPDATE'
       AND NEW.slow_authorization_id IS NOT DISTINCT FROM OLD.slow_authorization_id THEN
        RETURN NULL;
    END IF;

    -- `authorization` 是 SQL 保留字，别名不能直接用它。
    SELECT proxy.lot_id, proxy.mode
    INTO authorization_lot_id, authorization_mode
    FROM public.bid_proxy_authorizations AS proxy
    WHERE proxy.authorization_id = NEW.slow_authorization_id;
    IF NOT FOUND THEN
        -- 外键在提交时失败关闭；这里不重复报告。
        RETURN NULL;
    END IF;

    IF authorization_lot_id IS DISTINCT FROM NEW.id
       OR authorization_mode IS DISTINCT FROM 'slow' THEN
        RAISE EXCEPTION 'lot slow authorization must belong to the lot with slow mode'
            USING ERRCODE = '23514';
    END IF;
    RETURN NULL;
END
$function$;

CREATE CONSTRAINT TRIGGER lots_slow_authorization_owner_000140
AFTER INSERT OR UPDATE OF slow_authorization_id ON public.lots
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION public.enforce_lot_slow_authorization_owner_000140();

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (140, 'breaking');
