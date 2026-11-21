-- settlement recovery owner：新建 buyer_blacklist action 必须冻结公开理由，保留旧 NULL 历史行。
-- owner: auction-storage
-- 不变量：新黑名单事实携带可审计理由；既有 NULL 行不回填、不改写。
-- Breaking：收紧新写入准入；升级前停止可能写入 recovery action 的角色。
LOCK TABLE public.settlement_recovery_actions IN SHARE ROW EXCLUSIVE MODE;

CREATE FUNCTION public.require_new_buyer_blacklist_reason_000129()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, public
AS $function$
BEGIN
    IF NEW.kind = 'buyer_blacklist' AND NEW.reason IS NULL THEN
        IF TG_OP = 'INSERT' THEN
            RAISE EXCEPTION 'buyer_blacklist recovery action 必须冻结理由'
                USING ERRCODE = '23514';
        ELSIF OLD.kind IS DISTINCT FROM NEW.kind OR OLD.reason IS NOT NULL THEN
            RAISE EXCEPTION 'buyer_blacklist recovery action 必须有理由'
                USING ERRCODE = '23514';
        END IF;
    END IF;
    RETURN NEW;
END
$function$;

CREATE TRIGGER settlement_recovery_blacklist_reason_000129
BEFORE INSERT OR UPDATE OF kind, reason ON public.settlement_recovery_actions
FOR EACH ROW EXECUTE FUNCTION public.require_new_buyer_blacklist_reason_000129();

INSERT INTO public.audit_records
    (id, actor_type, actor_id, action, entity_type, entity_id, result, source, metadata)
VALUES (
    gen_random_uuid(), 'system', 'migration-000129', 'system.migration', 'migration',
    '000129', 'success', 'system',
    jsonb_build_object(
        'summary', 'require reasons for new buyer blacklist recovery actions',
        'compatibility', 'breaking')
);

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (129, 'breaking');
