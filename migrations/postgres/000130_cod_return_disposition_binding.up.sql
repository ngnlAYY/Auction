-- delivery/recovery owners：COD 退回处置必须引用同一付款义务的原 Send 投递与 fence。
-- owner: auction-storage
-- 不变量：cod_returned 的 recovery 与 delivery.origin_id 指向同一付款义务，且处置绑定投递当前 fence。
-- Breaking：拒绝无法证明的既有处置及后续身份改写；升级前停止 Worker、Admin 与写入角色。
LOCK TABLE
    public.settlement_recoveries,
    public.settlement_obligations,
    public.deliveries,
    public.delivery_dispositions
IN SHARE ROW EXCLUSIVE MODE;

DO $block$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM public.delivery_dispositions AS disposition
        LEFT JOIN public.settlement_recoveries AS recovery
          ON recovery.id = disposition.settlement_recovery_id
        LEFT JOIN public.settlement_obligations AS payment
          ON payment.id = recovery.payment_obligation_id
        LEFT JOIN public.deliveries AS delivery
          ON delivery.id = disposition.delivery_id
        WHERE disposition.kind = 'cod_returned'
          AND (
              recovery.id IS NULL
              OR payment.id IS NULL
              OR payment.obligation_kind NOT IN ('winner_equipment_cod', 'winner_payment_carrier_cod')
              OR delivery.id IS NULL
              OR delivery.origin_kind IS DISTINCT FROM 'settlement'
              OR delivery.origin_id IS DISTINCT FROM recovery.payment_obligation_id::text
              OR delivery.payload_operation IS DISTINCT FROM 'send'
              OR delivery.fencing_version IS DISTINCT FROM disposition.fencing_version
          )
    ) THEN
        RAISE EXCEPTION '已有 cod_returned 处置未绑定其 recovery 付款投递'
            USING ERRCODE = '23514';
    END IF;
END
$block$;

CREATE FUNCTION public.validate_cod_return_disposition_binding_000130()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, public
AS $function$
DECLARE
    recovery_payment_obligation_id uuid;
    delivery_origin_kind text;
    delivery_origin_id text;
    delivery_operation text;
    payment_obligation_kind text;
    delivery_fencing_version bigint;
BEGIN
    IF TG_TABLE_NAME = 'delivery_dispositions' THEN
        IF NEW.kind <> 'cod_returned' THEN
            RETURN NEW;
        END IF;
        SELECT recovery.payment_obligation_id
        INTO recovery_payment_obligation_id
        FROM public.settlement_recoveries AS recovery
        WHERE recovery.id = NEW.settlement_recovery_id
        FOR SHARE;
        IF NOT FOUND THEN
            RAISE EXCEPTION 'cod_returned 对应的 recovery 不存在'
                USING ERRCODE = '23514';
        END IF;
        SELECT payment.obligation_kind
        INTO payment_obligation_kind
        FROM public.settlement_obligations AS payment
        WHERE payment.id = recovery_payment_obligation_id
        FOR SHARE;
        IF NOT FOUND THEN
            RAISE EXCEPTION 'cod_returned 对应的付款义务不存在'
                USING ERRCODE = '23514';
        END IF;
        SELECT delivery.origin_kind, delivery.origin_id,
               delivery.payload_operation, delivery.fencing_version
        INTO delivery_origin_kind, delivery_origin_id,
             delivery_operation, delivery_fencing_version
        FROM public.deliveries AS delivery
        WHERE delivery.id = NEW.delivery_id
        FOR SHARE;
        IF NOT FOUND
           OR delivery_origin_kind IS DISTINCT FROM 'settlement'
           OR delivery_origin_id IS DISTINCT FROM recovery_payment_obligation_id::text
           OR delivery_operation IS DISTINCT FROM 'send'
           OR delivery_fencing_version IS DISTINCT FROM NEW.fencing_version
           OR payment_obligation_kind NOT IN (
               'winner_equipment_cod', 'winner_payment_carrier_cod'
           ) THEN
            RAISE EXCEPTION 'cod_returned 处置必须绑定 recovery 付款 Send 与当前 fence'
                USING ERRCODE = '23514';
        END IF;
        RETURN NEW;
    END IF;

    IF TG_TABLE_NAME = 'settlement_recoveries' THEN
        IF NEW.payment_obligation_id IS DISTINCT FROM OLD.payment_obligation_id
           AND EXISTS (
               SELECT 1 FROM public.delivery_dispositions AS disposition
               WHERE disposition.settlement_recovery_id = OLD.id
                 AND disposition.kind = 'cod_returned'
           ) THEN
            RAISE EXCEPTION '已有 cod_returned 证据后 recovery 付款身份不可修改'
                USING ERRCODE = '55000';
        END IF;
        RETURN NEW;
    END IF;
    IF TG_TABLE_NAME = 'settlement_obligations' THEN
        IF NEW.obligation_kind IS DISTINCT FROM OLD.obligation_kind
           AND EXISTS (
               SELECT 1
               FROM public.settlement_recoveries AS recovery
               JOIN public.delivery_dispositions AS disposition
                 ON disposition.settlement_recovery_id = recovery.id
               WHERE recovery.payment_obligation_id = OLD.id
                 AND disposition.kind = 'cod_returned'
           ) THEN
            RAISE EXCEPTION '已有 cod_returned 证据后付款义务种类不可修改'
                USING ERRCODE = '55000';
        END IF;
        RETURN NEW;
    END IF;

    IF TG_TABLE_NAME = 'deliveries'
       AND ROW(NEW.origin_kind, NEW.origin_id, NEW.payload->>'operation', NEW.fencing_version)
           IS DISTINCT FROM
           ROW(OLD.origin_kind, OLD.origin_id, OLD.payload->>'operation', OLD.fencing_version)
       AND EXISTS (
           SELECT 1 FROM public.delivery_dispositions AS disposition
           WHERE disposition.delivery_id = OLD.id
             AND disposition.kind = 'cod_returned'
       ) THEN
        RAISE EXCEPTION '已有 cod_returned 证据后 delivery 来源与 fence 不可修改'
            USING ERRCODE = '55000';
    END IF;
    RETURN NEW;
END
$function$;

CREATE TRIGGER delivery_dispositions_cod_return_binding_000130
BEFORE INSERT ON public.delivery_dispositions
FOR EACH ROW EXECUTE FUNCTION public.validate_cod_return_disposition_binding_000130();
CREATE TRIGGER settlement_recovery_cod_return_identity_000130
BEFORE UPDATE OF payment_obligation_id ON public.settlement_recoveries
FOR EACH ROW EXECUTE FUNCTION public.validate_cod_return_disposition_binding_000130();
CREATE TRIGGER settlement_obligation_cod_return_identity_000130
BEFORE UPDATE OF obligation_kind ON public.settlement_obligations
FOR EACH ROW EXECUTE FUNCTION public.validate_cod_return_disposition_binding_000130();
CREATE TRIGGER delivery_cod_return_identity_000130
BEFORE UPDATE OF origin_kind, origin_id, payload, fencing_version ON public.deliveries
FOR EACH ROW EXECUTE FUNCTION public.validate_cod_return_disposition_binding_000130();

INSERT INTO public.audit_records
    (id, actor_type, actor_id, action, entity_type, entity_id, result, source, metadata)
VALUES (
    gen_random_uuid(), 'system', 'migration-000130', 'system.migration', 'migration',
    '000130', 'success', 'system',
    jsonb_build_object(
        'summary', 'bind cod_returned dispositions to recovery payment delivery and fence',
        'compatibility', 'breaking')
);

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (130, 'breaking');
