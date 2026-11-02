-- 人工 Sent 证明扩展到“Admin 对账判失败、但原生 Sent 证明已寄出”的结算发送。
-- owner：auction-storage sent_confirmation。只替换强谓词函数，不改写任何既有行：
-- 原失败命令（delivery_admin_commands）、审计与 confirmation 原快照保持不可变，更正以新的
-- confirmation 事实表达。既有 write_unknown 分支的条件逐字保留；新分支要求快照中的失败结论
-- 仍能在命令账本中按命令 ID、结果状态、fence 与失败码精确找到。
-- 部署顺序：仅替换 STABLE 函数，旧进程读取结果不变，可在常规停机门禁下执行。

CREATE OR REPLACE FUNCTION public.delivery_has_operator_sent_proof(
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
              AND (
                  (confirmation.original_snapshot->'delivery'->>'status'='manual_reconcile'
                   AND confirmation.original_snapshot->'delivery'->>'failure_code'='write_unknown'
                   AND confirmation.original_snapshot->'delivery'->'result_metadata'->'write_started'='true'::jsonb)
                  OR (confirmation.original_snapshot->'delivery'->>'status'='failed'
                   AND confirmation.original_snapshot->'delivery'->'result_metadata'->>'resolution'='admin_reconciled_failed'
                   AND confirmation.original_snapshot->'fulfillment'->>'execution_state'='failed'
                   AND $2->>'corrected_admin_failure_command_id'=confirmation.original_snapshot->'admin_failure'->>'command_id'
                   AND EXISTS (
                       SELECT 1 FROM public.delivery_admin_commands admin
                       WHERE admin.delivery_id=delivery.id AND admin.command_kind='reconcile'
                         AND admin.command_id::text=confirmation.original_snapshot->'admin_failure'->>'command_id'
                         AND admin.result_payload->>'Status'='failed'
                         AND admin.result_payload->'FencingVersion'=confirmation.original_snapshot->'delivery'->'fencing_version'
                         AND admin.result_payload->>'FailureCode'=confirmation.original_snapshot->'delivery'->>'failure_code'
                         AND NOT EXISTS (
                             SELECT 1 FROM public.delivery_admin_commands later
                             WHERE later.delivery_id=admin.delivery_id
                               AND (later.created_at, later.command_id) > (admin.created_at, admin.command_id)
                         )
                   ))
              )
              AND NOT EXISTS(SELECT 1 FROM public.settlement_equipment_losses loss WHERE loss.delivery_id=delivery.id)
        ), false);
$$;
