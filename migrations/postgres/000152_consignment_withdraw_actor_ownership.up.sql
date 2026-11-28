-- 账户挂单的撤回事件必须由卖家本人发起。
-- owner: auction-storage
--
-- 不变量：
--   1. 000138 的 `withdrawal_requested` 与 `closed` 事件门只要求 `seller_user_id IS NOT NULL`，
--      没有把 `consignment_commands.actor_ref` 与挂单卖家比较；任何 public_user 都能写入撤回事件。
--   2. 匿名挂单（seller_user_id 为空）沿用 000138 的同名账户门，本触发器不改变其语义。
--   3. 只对 `withdraw` 命令的 public_user 事件追加窄守卫；不改写既有事件或命令。
--
-- 等级：breaking。新增会拒绝旧程序可能写入的越权撤回事件，须与同版程序一起切换。

CREATE FUNCTION public.enforce_consignment_withdraw_owner_000152()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, public
AS $function$
DECLARE
    listing_seller_id uuid;
    command_actor_ref text;
    command_actor_kind text;
    command_operation text;
BEGIN
    IF NEW.event_kind NOT IN ('withdrawal_requested', 'closed') THEN
        RETURN NEW;
    END IF;

    SELECT seller_user_id
    INTO listing_seller_id
    FROM public.consignment_listings
    WHERE id = NEW.listing_id;
    IF NOT FOUND OR listing_seller_id IS NULL THEN
        RETURN NEW;
    END IF;

    SELECT actor_ref, actor_kind, operation
    INTO command_actor_ref, command_actor_kind, command_operation
    FROM public.consignment_commands
    WHERE command_id = NEW.command_id;
    IF NOT FOUND THEN
        RETURN NEW;
    END IF;

    IF command_operation = 'withdraw'
       AND command_actor_kind = 'public_user'
       AND command_actor_ref IS DISTINCT FROM listing_seller_id::text THEN
        RAISE EXCEPTION '寄售挂单撤回必须由卖家账户发起'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END
$function$;
REVOKE ALL ON FUNCTION public.enforce_consignment_withdraw_owner_000152() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.enforce_consignment_withdraw_owner_000152()
    TO auction_runtime;

CREATE TRIGGER consignment_events_withdraw_owner_000152
BEFORE INSERT ON public.consignment_events
FOR EACH ROW EXECUTE FUNCTION public.enforce_consignment_withdraw_owner_000152();

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (152, 'breaking');
