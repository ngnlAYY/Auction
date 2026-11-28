-- 闭账恢复的 Send/退件归属不能只由 settlement_recoveries 的写入时校验保护：
-- 退件消费与发送尝试是追加事实，闭账后仍可让唯一归属不再成立。
-- owner: auction-storage
--
-- 不变量：
--   1. `settlement_recoveries.resolution IN ('reclaimed', 'cancelled')` 且付款履约为对应
--      `failed` 时，本次恢复的全部可能外达 Send 必须被已消费退件一一归属。
--   2. 000123 只在恢复行插入/更新时校验；`settlement_recovery_return_consumptions` 与
--      `delivery_send_attempts` 之后追加的事实可以改变同一判定。
--   3. 新增延迟约束触发器在提交前重跑 000123 的归属函数；既不改写历史事实，也不删除追加事实。
--
-- 等级：breaking。新增会拒绝闭账后破坏唯一归属的追加写入，须与同版程序一起切换。

CREATE FUNCTION public.validate_recovery_return_attribution_after_append_000145()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, public
AS $function$
DECLARE
    target_recovery_id uuid;
BEGIN
    IF TG_TABLE_NAME = 'settlement_recovery_return_consumptions' THEN
        target_recovery_id := NEW.recovery_id;
    ELSE
        SELECT recovery.id
        INTO target_recovery_id
        FROM public.deliveries AS delivery
        JOIN public.settlement_recoveries AS recovery
          ON recovery.payment_obligation_id::text = delivery.origin_id
        WHERE delivery.id = NEW.delivery_id
          AND delivery.origin_kind = 'settlement';
    END IF;

    IF target_recovery_id IS NULL THEN
        RETURN NULL;
    END IF;

    IF EXISTS (
        SELECT 1
        FROM public.settlement_recoveries AS recovery
        WHERE recovery.id = target_recovery_id
          AND recovery.resolution IN ('reclaimed', 'cancelled')
          AND EXISTS (
              SELECT 1
              FROM public.settlement_obligation_fulfillments AS payment
              WHERE payment.obligation_id = recovery.payment_obligation_id
                AND payment.execution_state = 'failed'
                AND payment.last_error_code IN ('recovery_reclaimed', 'recovery_cancelled')
          )
          AND NOT public.recovery_payment_return_attributable_000123(recovery.id)
    ) THEN
        RAISE EXCEPTION '闭账恢复的 Send/退件归属被追加事实破坏；须人工核对'
            USING ERRCODE = '23514';
    END IF;
    RETURN NULL;
END
$function$;
REVOKE ALL ON FUNCTION public.validate_recovery_return_attribution_after_append_000145()
    FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.validate_recovery_return_attribution_after_append_000145()
    TO auction_runtime;

CREATE CONSTRAINT TRIGGER settlement_recovery_return_consumptions_attribution_000145
AFTER INSERT ON public.settlement_recovery_return_consumptions
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION public.validate_recovery_return_attribution_after_append_000145();

CREATE CONSTRAINT TRIGGER delivery_send_attempts_recovery_attribution_000145
AFTER INSERT ON public.delivery_send_attempts
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION public.validate_recovery_return_attribution_after_append_000145();

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (145, 'breaking');
