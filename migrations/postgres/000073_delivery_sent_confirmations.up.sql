-- 人工确认是独立证明来源，不能伪装成 sent_verified；旧迁移及历史未知结果保持不可变。
CREATE TABLE public.delivery_sent_confirmations (
    command_id uuid PRIMARY KEY CHECK (command_id <> '00000000-0000-0000-0000-000000000000'),
    delivery_id uuid NOT NULL REFERENCES public.deliveries(id) ON DELETE RESTRICT,
    expected_fencing_version bigint NOT NULL CHECK (expected_fencing_version > 0),
    sending_fencing_version bigint NOT NULL CHECK (sending_fencing_version > 0 AND sending_fencing_version <= expected_fencing_version),
    world_id text NOT NULL CHECK (world_id = 'main'),
    external_mail_id text NOT NULL CHECK (length(external_mail_id) BETWEEN 1 AND 64 AND external_mail_id ~ '^[1-9][0-9]*$'),
    request_digest bytea NOT NULL CHECK (octet_length(request_digest) = 32),
    actor text NOT NULL CHECK (length(actor) BETWEEN 1 AND 128 AND actor = btrim(actor) AND actor !~ '[[:cntrl:]]'),
    reason text NOT NULL CHECK (length(reason) BETWEEN 1 AND 2048 AND reason = btrim(reason) AND reason !~ '[[:cntrl:]]'),
    command jsonb NOT NULL CHECK (jsonb_typeof(command) = 'object'
        AND command ?& ARRAY['command_id','delivery_id','expected_fencing_version','actor','reason','evidence','accept_collected_assets','accept_attempt_history']
        AND command-ARRAY['command_id','delivery_id','expected_fencing_version','actor','reason','evidence','accept_collected_assets','accept_attempt_history']::text[]='{}'::jsonb),
    evidence jsonb NOT NULL CHECK (jsonb_typeof(evidence) = 'object'
        AND evidence ?& ARRAY['schema_version','world_id','external_mail_id','sender_uid','sender_name','recipient','subject','sent_minute','observed_at','original_body_digest','observed_body_digest','remaining_credits','cod_pending','equipment_ids','collected','receipt']
        AND evidence-ARRAY['schema_version','world_id','external_mail_id','sender_uid','sender_name','recipient','subject','sent_minute','observed_at','original_body_digest','observed_body_digest','remaining_credits','cod_pending','equipment_ids','collected','receipt']::text[]='{}'::jsonb
        AND evidence->'schema_version' = '1'::jsonb),
    accept_collected_assets boolean NOT NULL,
    accept_attempt_history boolean NOT NULL,
    original_snapshot jsonb NOT NULL CHECK (jsonb_typeof(original_snapshot) = 'object'),
    result jsonb NOT NULL CHECK (jsonb_typeof(result) = 'object'),
    confirmed_at timestamptz NOT NULL DEFAULT clock_timestamp() CHECK (isfinite(confirmed_at)),
    UNIQUE (delivery_id, expected_fencing_version),
    UNIQUE (world_id, external_mail_id),
    FOREIGN KEY (delivery_id, sending_fencing_version)
        REFERENCES public.delivery_send_attempts(delivery_id, fencing_version) ON DELETE RESTRICT,
    FOREIGN KEY (world_id, external_mail_id)
        REFERENCES public.delivery_sent_bindings(world_id, external_mail_id) ON DELETE RESTRICT,
    CHECK (COALESCE(command->>'command_id' = command_id::text AND command->>'delivery_id' = delivery_id::text
        AND command->'expected_fencing_version' = to_jsonb(expected_fencing_version)
        AND command->>'actor' = actor AND command->>'reason' = reason AND command->'evidence' = evidence
        AND command->'accept_collected_assets' = to_jsonb(accept_collected_assets)
        AND command->'accept_attempt_history' = to_jsonb(accept_attempt_history),false)),
    CHECK (COALESCE(evidence->>'world_id' = world_id AND evidence->>'external_mail_id' = external_mail_id
        AND (evidence->'collected' = 'false'::jsonb OR (evidence->'collected' = 'true'::jsonb AND accept_collected_assets)),false)),
    CHECK (COALESCE(original_snapshot->'prior_unbound' = 'false'::jsonb OR accept_attempt_history,false))
);

CREATE TRIGGER delivery_sent_confirmations_append_only
BEFORE UPDATE OR DELETE ON public.delivery_sent_confirmations
FOR EACH ROW EXECUTE FUNCTION public.reject_delivery_send_evidence_mutation();

-- 能力来自不可变命令/当前冻结尝试/唯一MID/当前投递的完整关联，不来自元数据字符串。
CREATE FUNCTION public.delivery_has_operator_sent_proof(
    delivery_id uuid, result_metadata jsonb, external_reference text
) RETURNS boolean LANGUAGE sql STABLE AS $$
    SELECT COALESCE(
        $2->>'proof'='sent_operator_confirmed_v1'
        AND $2->>'operation'='send' AND $2->'external_reference_proven'='true'::jsonb
        AND NOT COALESCE($2->'sent_verified'='true'::jsonb,false)
        AND EXISTS (
            SELECT 1 FROM public.delivery_sent_confirmations confirmation
            JOIN public.delivery_send_attempts attempt
                ON attempt.delivery_id=confirmation.delivery_id
               AND attempt.fencing_version=confirmation.sending_fencing_version
            JOIN public.delivery_sent_bindings binding
                ON binding.delivery_id=attempt.delivery_id AND binding.fencing_version=attempt.fencing_version
               AND binding.expectation_fencing_version=attempt.fencing_version
               AND binding.world_id=confirmation.world_id AND binding.external_mail_id=confirmation.external_mail_id
            JOIN public.deliveries delivery ON delivery.id=confirmation.delivery_id
            JOIN public.audit_records audit ON audit.id=confirmation.command_id
                AND audit.action='delivery.sent_operator_confirmed'
                AND audit.entity_type='delivery' AND audit.entity_id=delivery.id::text
                AND audit.actor_type='admin' AND audit.actor_id=confirmation.actor
                AND audit.result='success'
            WHERE confirmation.delivery_id=$1 AND delivery.status='succeeded'
              AND delivery.origin_kind='settlement' AND delivery.payload->>'operation'='send'
              AND delivery.fencing_version=confirmation.expected_fencing_version
              AND delivery.external_reference=$3 AND delivery.result_metadata=$2
              AND $2->>'operator_confirmation_id'=confirmation.command_id::text
              AND $2->'sending_fencing_version'=to_jsonb(attempt.fencing_version)
              AND $2->'send_expectation'=attempt.expectation
              AND attempt.fencing_version=(SELECT sending_fencing_version FROM public.delivery_current_send_attempt(delivery.id))
              AND $3='mooglemail:'||binding.world_id||':send:'||binding.external_mail_id
              AND confirmation.evidence->>'world_id'=attempt.world_id
              AND confirmation.evidence->>'recipient'=attempt.expectation->>'recipient'
              AND confirmation.evidence->>'subject'=attempt.expectation->>'subject'
              AND confirmation.evidence->'original_body_digest'=attempt.expectation->'body_digest'
              AND confirmation.original_snapshot->'expectation'=attempt.expectation
              AND confirmation.original_snapshot->'sending_fencing_version'=to_jsonb(attempt.fencing_version)
              AND confirmation.result->>'command_id'=confirmation.command_id::text
              AND confirmation.result->>'delivery_id'=delivery.id::text
              AND confirmation.result->>'external_reference'=$3
              AND confirmation.original_snapshot->'delivery'->>'id'=delivery.id::text
              AND confirmation.original_snapshot->'delivery'->>'command_id'=delivery.command_id::text
              AND confirmation.original_snapshot->'delivery'->>'request_digest'=to_jsonb(delivery)->>'request_digest'
              AND confirmation.original_snapshot->'delivery'->'payload'=delivery.payload
              AND confirmation.original_snapshot->'delivery'->>'status'='manual_reconcile'
              AND confirmation.original_snapshot->'delivery'->>'failure_code'='write_unknown'
              AND confirmation.original_snapshot->'delivery'->'result_metadata'->'write_started'='true'::jsonb
              AND NOT EXISTS(SELECT 1 FROM public.settlement_equipment_losses loss WHERE loss.delivery_id=delivery.id)
        ), false);
$$;

-- 保持 61 的自动证明行为，所有既有资金/履约/退货调用方统一扩展到强人工谓词。
CREATE OR REPLACE FUNCTION public.delivery_has_frozen_sent_proof(
    delivery_id uuid, result_metadata jsonb, external_reference text
) RETURNS boolean LANGUAGE sql STABLE AS $$
    SELECT COALESCE(
        $2->>'operation' = 'send'
        AND $2->'external_reference_proven' = 'true'::jsonb
        AND (
            ($2->>'proof' = 'sent_exact_payload_v1' AND $2->'sent_verified' = 'true'::jsonb)
            OR ($2->>'proof' = 'sent_recovered_frozen_v1' AND $2->'sent_recovered' = 'true'::jsonb)
        )
        AND EXISTS (
            SELECT 1 FROM public.delivery_send_attempts attempt
            JOIN public.delivery_sent_bindings binding
                ON binding.delivery_id=attempt.delivery_id
               AND binding.fencing_version=attempt.fencing_version
               AND binding.expectation_fencing_version=attempt.fencing_version
               AND binding.world_id=attempt.world_id
            JOIN public.deliveries delivery
                ON delivery.id=attempt.delivery_id
               AND attempt.fencing_version=(
                   SELECT sending_fencing_version FROM public.delivery_current_send_attempt(delivery.id)
               )
            WHERE attempt.delivery_id=$1 AND delivery.status='succeeded'
              AND delivery.external_reference=$3 AND delivery.result_metadata=$2
              AND $2->'send_expectation'=attempt.expectation
              AND $2->'sending_fencing_version'=to_jsonb(attempt.fencing_version)
              AND $3='mooglemail:' || binding.world_id || ':send:' || binding.external_mail_id
        ), false
    ) OR public.delivery_has_operator_sent_proof($1,$2,$3);
$$;
