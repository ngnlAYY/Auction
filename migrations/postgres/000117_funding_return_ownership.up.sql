-- 补上两项关系归属约束，不改变有效的应用写入。
-- owner: auction-storage
--
-- Compatibility: breaking. 新增唯一/FK 约束及延迟绑定触发器会拒绝或限制既有数据库写入语义，需旧程序停止后迁移。

LOCK TABLE public.settlement_funding_entries,
    public.settlement_funding_allocations,
    public.settlement_funding_sources,
    public.settlement_return_actions,
    public.deliveries
    IN SHARE ROW EXCLUSIVE MODE;

DO $block$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM public.settlement_funding_entries AS entry
        LEFT JOIN public.settlement_funding_allocations AS debit
          ON debit.id = entry.debit_id
        LEFT JOIN public.settlement_funding_allocations AS credit
          ON credit.id = entry.credit_id
        LEFT JOIN public.settlement_funding_sources AS source
          ON source.id = entry.source_id
        WHERE (entry.debit_id IS NOT NULL AND debit.uid IS DISTINCT FROM entry.uid)
           OR (entry.credit_id IS NOT NULL AND credit.uid IS DISTINCT FROM entry.uid)
           OR (entry.source_id IS NOT NULL AND source.uid IS DISTINCT FROM entry.uid)
    ) THEN
        RAISE EXCEPTION 'funding entry UID does not match a referenced allocation or source'
            USING ERRCODE = '23514';
    END IF;

    IF EXISTS (
        SELECT 1
        FROM public.settlement_return_actions AS action
        LEFT JOIN public.deliveries AS delivery ON delivery.id = action.delivery_id
        WHERE action.delivery_id IS NOT NULL
          AND (delivery.id IS NULL
               OR delivery.origin_kind IS DISTINCT FROM 'settlement_return_action'
               OR delivery.origin_id IS DISTINCT FROM 'settlement_return:' || action.return_mail_id::text
               OR delivery.command_id IS DISTINCT FROM action.prepare_command_id
               OR delivery.activation_command_id IS DISTINCT FROM action.activate_command_id
               OR delivery.payload_world IS DISTINCT FROM action.world_id
               OR delivery.payload_operation IS DISTINCT FROM 'take'
               OR delivery.payload ->> 'external_mail_id' IS DISTINCT FROM action.source_external_mail_id)
    ) OR EXISTS (
        SELECT 1
        FROM public.deliveries AS delivery
        WHERE delivery.origin_kind = 'settlement_return_action'
          AND NOT EXISTS (
              SELECT 1 FROM public.settlement_return_actions AS action
              WHERE action.delivery_id = delivery.id
          )
    ) THEN
        RAISE EXCEPTION 'settlement return action is not bound to its exact Take delivery'
            USING ERRCODE = '23514';
    END IF;
END
$block$;

ALTER TABLE public.settlement_funding_allocations
    ADD CONSTRAINT settlement_funding_allocations_uid_id_key UNIQUE (uid, id);
ALTER TABLE public.settlement_funding_sources
    ADD CONSTRAINT settlement_funding_sources_uid_id_key UNIQUE (uid, id);

ALTER TABLE public.settlement_funding_entries
    ADD CONSTRAINT settlement_funding_entries_debit_uid_fkey
        FOREIGN KEY (uid, debit_id)
        REFERENCES public.settlement_funding_allocations (uid, id)
        ON DELETE RESTRICT NOT VALID,
    ADD CONSTRAINT settlement_funding_entries_credit_uid_fkey
        FOREIGN KEY (uid, credit_id)
        REFERENCES public.settlement_funding_allocations (uid, id)
        ON DELETE RESTRICT NOT VALID,
    ADD CONSTRAINT settlement_funding_entries_source_uid_fkey
        FOREIGN KEY (uid, source_id)
        REFERENCES public.settlement_funding_sources (uid, id)
        ON DELETE RESTRICT NOT VALID;

ALTER TABLE public.settlement_funding_entries
    VALIDATE CONSTRAINT settlement_funding_entries_debit_uid_fkey,
    VALIDATE CONSTRAINT settlement_funding_entries_credit_uid_fkey,
    VALIDATE CONSTRAINT settlement_funding_entries_source_uid_fkey;

CREATE FUNCTION public.enforce_settlement_return_action_delivery_binding()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, public
AS $function$
DECLARE
    checked_return_mail_id uuid;
    checked_delivery_id uuid;
    action_row record;
    delivery_row record;
BEGIN
    IF TG_TABLE_NAME = 'settlement_return_actions' THEN
        IF TG_OP = 'DELETE' THEN
            IF OLD.delivery_id IS NOT NULL THEN
                RAISE EXCEPTION 'a bound settlement return action cannot be deleted'
                    USING ERRCODE = '23514';
            END IF;
            RETURN NULL;
        END IF;
        IF TG_OP = 'UPDATE'
           AND OLD.delivery_id IS NOT NULL
           AND NEW.delivery_id IS DISTINCT FROM OLD.delivery_id THEN
            RAISE EXCEPTION 'a bound settlement return action cannot change its Take delivery'
                USING ERRCODE = '23514';
        END IF;
        checked_return_mail_id := NEW.return_mail_id;
        checked_delivery_id := NEW.delivery_id;
        IF checked_delivery_id IS NULL THEN
            RETURN NULL;
        END IF;
    ELSE
        checked_delivery_id := NEW.id;
        IF NEW.origin_kind IS DISTINCT FROM 'settlement_return_action'
           AND (TG_OP = 'INSERT'
                OR OLD.origin_kind IS DISTINCT FROM 'settlement_return_action')
           AND NOT EXISTS (
               SELECT 1 FROM public.settlement_return_actions AS action
               WHERE action.delivery_id = checked_delivery_id
           ) THEN
            RETURN NULL;
        END IF;
        SELECT action.return_mail_id
        INTO checked_return_mail_id
        FROM public.settlement_return_actions AS action
        WHERE action.delivery_id = checked_delivery_id;
        IF checked_return_mail_id IS NULL THEN
            RAISE EXCEPTION 'settlement return Take delivery has no owning action'
                USING ERRCODE = '23514';
        END IF;
    END IF;

    SELECT * INTO action_row
    FROM public.settlement_return_actions AS action
    WHERE action.return_mail_id = checked_return_mail_id
      AND action.delivery_id = checked_delivery_id;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'settlement return action does not own its Take delivery'
            USING ERRCODE = '23514';
    END IF;

    SELECT * INTO delivery_row
    FROM public.deliveries AS delivery
    WHERE delivery.id = checked_delivery_id;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'settlement return Take delivery does not exist'
            USING ERRCODE = '23514';
    END IF;
    IF delivery_row.origin_kind IS DISTINCT FROM 'settlement_return_action'
       OR delivery_row.origin_id IS DISTINCT FROM 'settlement_return:' || checked_return_mail_id::text
       OR delivery_row.command_id IS DISTINCT FROM action_row.prepare_command_id
       OR delivery_row.activation_command_id IS DISTINCT FROM action_row.activate_command_id
       OR delivery_row.payload_world IS DISTINCT FROM action_row.world_id
       OR delivery_row.payload_operation IS DISTINCT FROM 'take'
       OR delivery_row.payload ->> 'external_mail_id' IS DISTINCT FROM action_row.source_external_mail_id THEN
        RAISE EXCEPTION 'settlement return action is not bound to its exact Take delivery'
            USING ERRCODE = '23514';
    END IF;
    RETURN NULL;
END
$function$;

CREATE CONSTRAINT TRIGGER settlement_return_action_delivery_binding
AFTER INSERT OR UPDATE OF delivery_id, world_id, source_external_mail_id,
    prepare_command_id, activate_command_id OR DELETE
ON public.settlement_return_actions
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION public.enforce_settlement_return_action_delivery_binding();

CREATE CONSTRAINT TRIGGER settlement_return_delivery_action_binding
AFTER INSERT OR UPDATE OF origin_kind, origin_id, command_id, activation_command_id,
    payload_world, payload_operation, payload
ON public.deliveries
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION public.enforce_settlement_return_action_delivery_binding();

INSERT INTO public.audit_records
    (id, occurred_at, actor_type, actor_id, action, entity_type, entity_id, result, source, metadata)
VALUES (
    gen_random_uuid(), clock_timestamp(), 'system', 'migration-000117', 'system.migration',
    'migration', '000117', 'success', 'system',
    jsonb_build_object(
        'summary', 'enforce funding UID and settlement return Take ownership bindings',
        'compatibility', 'breaking'
    )
);

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (117, 'breaking');
