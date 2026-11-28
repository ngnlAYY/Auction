-- 结算退件 Take 的领回事实身份必须在写入时绑定案件与原派发，而不是等到人工收口。
-- owner: auction-storage
--
-- 不变量：
--   1. 000114 只给 `settlement_return_takes` 的单列外键/唯一键；INSERT 可以写入与案件
--      `world_id`、`obligation_id`、`original_delivery_id` 不一致的事实。
--   2. 000125 只在 `manual_reconcile -> satisfied` 时核对证明与案件/义务/世界一致；
--      未进入该转换的错误事实仍可持久化。
--   3. 新触发器在 INSERT 时核对案件身份、原派发来源与 Take 投递载荷；不改写既有事实。
--
-- 等级：breaking。新增会拒绝旧程序可能写入的身份不一致 Take 事实，须与同版程序一起切换。

CREATE FUNCTION public.validate_settlement_return_take_identity_000148()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, public
AS $function$
DECLARE
    case_world_id text;
    case_obligation_id uuid;
    case_delivery_id uuid;
    take_origin_kind text;
    take_origin_id text;
    take_payload_world text;
    take_payload_operation text;
    take_external_mail_id text;
    original_origin_kind text;
    original_origin_id text;
BEGIN
    SELECT world_id, obligation_id, delivery_id
    INTO case_world_id, case_obligation_id, case_delivery_id
    FROM public.settlement_return_cases
    WHERE return_mail_id = NEW.return_mail_id;
    IF NOT FOUND THEN
        RAISE EXCEPTION '结算退件 Take 缺少案件'
            USING ERRCODE = '23514';
    END IF;
    IF case_world_id IS DISTINCT FROM NEW.world_id
       OR case_obligation_id IS DISTINCT FROM NEW.obligation_id
       OR case_delivery_id IS DISTINCT FROM NEW.original_delivery_id THEN
        RAISE EXCEPTION '结算退件 Take 与案件身份不一致'
            USING ERRCODE = '23514';
    END IF;

    SELECT origin_kind, origin_id
    INTO original_origin_kind, original_origin_id
    FROM public.deliveries
    WHERE id = NEW.original_delivery_id;
    IF NOT FOUND
       OR original_origin_kind <> 'settlement'
       OR original_origin_id IS DISTINCT FROM NEW.obligation_id::text THEN
        RAISE EXCEPTION '结算退件 Take 的原派发不属于该付款义务'
            USING ERRCODE = '23514';
    END IF;

    SELECT origin_kind, origin_id, payload_world, payload_operation,
           payload ->> 'external_mail_id'
    INTO take_origin_kind, take_origin_id, take_payload_world, take_payload_operation,
         take_external_mail_id
    FROM public.deliveries
    WHERE id = NEW.take_delivery_id;
    IF NOT FOUND
       OR take_origin_kind <> 'settlement_return_action'
       OR take_origin_id IS DISTINCT FROM 'settlement_return:' || NEW.return_mail_id::text
       OR take_payload_world IS DISTINCT FROM NEW.world_id
       OR take_payload_operation IS DISTINCT FROM 'take'
       OR take_external_mail_id IS DISTINCT FROM NEW.external_mail_id THEN
        RAISE EXCEPTION '结算退件 Take 的投递载荷与案件不一致'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END
$function$;
REVOKE ALL ON FUNCTION public.validate_settlement_return_take_identity_000148()
    FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.validate_settlement_return_take_identity_000148()
    TO auction_runtime;

CREATE TRIGGER settlement_return_takes_identity_000148
BEFORE INSERT ON public.settlement_return_takes
FOR EACH ROW EXECUTE FUNCTION public.validate_settlement_return_take_identity_000148();

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (148, 'breaking');
