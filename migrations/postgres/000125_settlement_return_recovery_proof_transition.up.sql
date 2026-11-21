-- 结果未知的结算退件 Take 只能凭原会话完整领取证明收口为 satisfied；保留当前投递 fence 与 append-only Take 事实核验。
-- owner: auction-storage
-- Breaking：扩展退件 action 生命周期；新程序在原 Take fence 与完整领取证明均匹配时允许 manual_reconcile → satisfied。
LOCK TABLE
    public.delivery_take_attempts,
    public.deliveries,
    public.settlement_return_actions,
    public.settlement_return_cases,
    public.settlement_return_takes
IN SHARE ROW EXCLUSIVE MODE;

CREATE OR REPLACE FUNCTION public.enforce_settlement_return_action_transition_000123()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, public
AS $function$
BEGIN
    IF NEW.state_version <> OLD.state_version + 1
       OR NEW.return_mail_id IS DISTINCT FROM OLD.return_mail_id
       OR NEW.world_id IS DISTINCT FROM OLD.world_id
       OR NEW.source_external_mail_id IS DISTINCT FROM OLD.source_external_mail_id
       OR NEW.expected_credits IS DISTINCT FROM OLD.expected_credits
       OR NEW.expected_cod IS DISTINCT FROM OLD.expected_cod
       OR NEW.expected_attachment_ids IS DISTINCT FROM OLD.expected_attachment_ids
       OR NEW.created_at IS DISTINCT FROM OLD.created_at
       OR (OLD.delivery_id IS NOT NULL AND NEW.delivery_id IS DISTINCT FROM OLD.delivery_id)
       OR NOT (
           (OLD.execution_state = 'pending' AND NEW.execution_state = 'enqueued')
           OR (OLD.execution_state = 'enqueued'
               AND NEW.execution_state IN ('satisfied', 'manual_reconcile'))
           OR (OLD.execution_state = 'manual_reconcile' AND NEW.execution_state = 'enqueued')
           OR (OLD.execution_state = 'manual_reconcile'
               AND NEW.execution_state = 'satisfied'
               AND EXISTS (
                   SELECT 1
                   FROM public.settlement_return_takes AS proof
                   JOIN public.settlement_return_cases AS case_row
                     ON case_row.return_mail_id = proof.return_mail_id
                   JOIN public.deliveries AS delivery
                     ON delivery.id = proof.take_delivery_id
                   JOIN LATERAL public.delivery_current_take_attempt(delivery.id) AS taking
                     ON true
                   JOIN public.delivery_take_attempts AS attempt
                     ON attempt.delivery_id = delivery.id
                    AND attempt.fencing_version = taking.taking_fencing_version
                   WHERE proof.return_mail_id = OLD.return_mail_id
                     AND proof.take_delivery_id = OLD.delivery_id
                     AND proof.original_delivery_id = case_row.delivery_id
                     AND proof.obligation_id = case_row.obligation_id
                     AND proof.world_id = OLD.world_id
                     AND proof.external_mail_id = OLD.source_external_mail_id
                     AND proof.credits = OLD.expected_credits
                     AND proof.cod = OLD.expected_cod
                     AND proof.attachment_ids = OLD.expected_attachment_ids
                     AND proof.proof_digest <> decode(repeat('00', 32), 'hex')
                     AND delivery.status = 'succeeded'
                     AND delivery.origin_kind = 'settlement_return_action'
                     AND delivery.origin_id = 'settlement_return:' || OLD.return_mail_id::text
                     AND delivery.command_id = OLD.prepare_command_id
                     AND delivery.activation_command_id = OLD.activate_command_id
                     AND delivery.payload_world = OLD.world_id
                     AND delivery.payload_operation = 'take'
                     AND delivery.payload->>'external_mail_id' = OLD.source_external_mail_id
                     AND delivery.external_reference =
                         'mooglemail:' || OLD.world_id || ':take:' || OLD.source_external_mail_id
                     AND attempt.command_id = delivery.command_id
                     AND attempt.request_digest = delivery.request_digest
                     AND attempt.world_id = OLD.world_id
                     AND attempt.expectation->>'world_id' = OLD.world_id
                     AND attempt.expectation->>'external_mail_id' = OLD.source_external_mail_id
                     AND attempt.sending_at = delivery.sending_at
               ))
       ) THEN
        RAISE EXCEPTION 'settlement return action lifecycle transition is invalid'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END
$function$;

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (125, 'breaking');
