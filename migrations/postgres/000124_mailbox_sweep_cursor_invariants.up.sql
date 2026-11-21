-- 邮箱轮转游标绑定 sweep 与 attempt 身份，拒绝迟到批次回退。
-- owner: auction-storage
-- Breaking：增加游标身份列与回绕守卫；升级前须停止旧 Worker。
LOCK TABLE
    public.deliveries,
    public.mailbox_sweep_cursors,
    public.settlement_obligation_fulfillments
IN SHARE ROW EXCLUSIVE MODE;

-- 轮转更新携带 sweep ID 与一次性 attempt ID；旧程序的无 token upsert 只能单调前进，不能回绕到旧批次。
ALTER TABLE public.mailbox_sweep_cursors
    ADD COLUMN last_sweep_id uuid,
    ADD COLUMN last_attempt_id uuid,
    ADD CONSTRAINT mailbox_sweep_cursor_token_pair_000124 CHECK (
        (last_sweep_id IS NULL) = (last_attempt_id IS NULL)
        AND (last_sweep_id IS NULL OR last_sweep_id <> '00000000-0000-0000-0000-000000000000')
        AND (last_attempt_id IS NULL OR last_attempt_id <> '00000000-0000-0000-0000-000000000000')
    );

CREATE FUNCTION public.reject_stale_mailbox_sweep_cursor_000124()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, public
AS $function$
BEGIN
    IF (NEW.last_created_at, NEW.last_delivery_id)
         < (OLD.last_created_at, OLD.last_delivery_id)
       AND (NEW.last_sweep_id IS NULL OR NEW.last_attempt_id IS NULL
            OR NEW.last_attempt_id IS NOT DISTINCT FROM OLD.last_attempt_id) THEN
        RAISE EXCEPTION 'mailbox sweep cursor regression requires a fresh attempt token'
            USING ERRCODE = '23514';
    END IF;
    IF (NEW.last_sweep_id IS NULL) <> (NEW.last_attempt_id IS NULL) THEN
        RAISE EXCEPTION 'mailbox sweep cursor attempt token pair is incomplete'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END
$function$;
CREATE TRIGGER mailbox_sweep_cursor_no_stale_regression_000124
BEFORE UPDATE ON public.mailbox_sweep_cursors
FOR EACH ROW EXECUTE FUNCTION public.reject_stale_mailbox_sweep_cursor_000124();

COMMENT ON COLUMN public.mailbox_sweep_cursors.last_sweep_id IS
    '本轮游标快照身份；迟到批次必须匹配轮首位置或当前 sweep';
COMMENT ON COLUMN public.mailbox_sweep_cursors.last_attempt_id IS
    '每次持久核对推进的唯一 attempt token；只有带新 token 的同轮且无后缀回绕可回退排序键';
INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (124, 'breaking');
