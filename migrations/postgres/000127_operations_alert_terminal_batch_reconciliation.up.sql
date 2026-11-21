-- operations alert owner：按 delivery 权威终态收口告警批次，只保留身份一致的原始 prepared delivery。
-- owner: auction-storage
-- 不变量：终态投递不得留下可领取批次；未激活、零尝试且身份完整的 prepared 投递保持可领取。
-- Breaking：修复既有投影并拒绝无法证明身份或状态的关联；升级前停止 Worker。
LOCK TABLE public.deliveries, public.operations_alert_batches IN SHARE ROW EXCLUSIVE MODE;

DO $block$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM public.operations_alert_batches AS batch
        JOIN public.deliveries AS delivery ON delivery.id = batch.delivery_id
        WHERE batch.status IN ('pending', 'claimed')
          AND NOT COALESCE((
              delivery.origin_kind = 'operations_alert'
              AND delivery.origin_id = 'operations_alert:' || batch.id::text
              AND delivery.command_id = batch.command_id
              AND delivery.request_digest = batch.request_digest
              AND delivery.payload ?& ARRAY[
                  'schema_version', 'operation', 'world', 'external_mail_id', 'template',
                  'result_code', 'user_key_user_id', 'credits', 'cod', 'attachment_ids', 'body'
              ]
              AND delivery.payload - ARRAY[
                  'schema_version', 'operation', 'world', 'external_mail_id', 'template',
                  'result_code', 'user_key_user_id', 'credits', 'cod', 'attachment_ids', 'body'
              ]::text[] = '{}'::jsonb
              AND delivery.payload->>'schema_version' = '1'
              AND delivery.payload->>'operation' = 'send'
              AND delivery.payload->>'world' IN ('main', 'isekai')
              AND delivery.payload->>'external_mail_id' = 'operations-alert:' || batch.id::text
              AND delivery.payload->>'template' = 'operations_alert'
              AND delivery.payload->>'result_code' = 'operations_alert'
              AND delivery.payload->'user_key_user_id' = 'null'::jsonb
              AND delivery.payload->>'credits' = '0'
              AND delivery.payload->>'cod' = '0'
              AND delivery.payload->'attachment_ids' = '[]'::jsonb
              AND delivery.payload->>'body' <> ''
              AND (
                  delivery.status IN ('succeeded', 'failed', 'manual_reconcile')
                  OR (
                      delivery.status = 'prepared'
                      AND delivery.attempt_count = 0
                      AND delivery.fencing_version = 0
                      AND delivery.lease_token IS NULL
                      AND delivery.lease_expires_at IS NULL
                      AND delivery.claimed_at IS NULL
                      AND delivery.sending_at IS NULL
                      AND delivery.completed_at IS NULL
                      AND delivery.external_reference IS NULL
                      AND delivery.failure_code IS NULL
                      AND delivery.result_metadata = '{}'::jsonb
                      AND delivery.activation_command_id IS NULL
                      AND delivery.activation_request_digest IS NULL
                      AND delivery.activated_at IS NULL
                  )
              )
          ), false)
    ) THEN
        RAISE EXCEPTION
            'pending/claimed 告警批次与 delivery 身份不一致，或 delivery 状态无法安全判定；必须先人工核对'
            USING ERRCODE = '23514';
    END IF;
END
$block$;

UPDATE public.operations_alert_batches AS batch
SET status = delivery.status,
    claim_token = NULL,
    claimed_at = NULL,
    claim_expires_at = NULL,
    next_attempt_at = NULL,
    delivery_error_code = CASE delivery.status
        WHEN 'succeeded' THEN ''
        WHEN 'failed' THEN 'delivery_failed'
        WHEN 'manual_reconcile' THEN 'delivery_manual_reconcile'
    END,
    completed_at = GREATEST(batch.created_at, batch.updated_at, delivery.completed_at),
    updated_at = GREATEST(batch.created_at, batch.updated_at, delivery.completed_at)
FROM public.deliveries AS delivery
WHERE batch.delivery_id = delivery.id
  AND batch.status IN ('pending', 'claimed')
  AND delivery.status IN ('succeeded', 'failed', 'manual_reconcile');

INSERT INTO public.audit_records
    (id, actor_type, actor_id, action, entity_type, entity_id, result, source, metadata)
VALUES (
    gen_random_uuid(), 'system', 'migration-000127', 'system.migration', 'migration',
    '000127', 'success', 'system',
    jsonb_build_object(
        'summary', 'reconcile alert batches with terminal deliveries',
        'compatibility', 'breaking')
);

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (127, 'breaking');
