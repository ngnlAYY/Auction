-- 投稿装备已由入库 Take 领取，流拍须逐件 Send；原邮件 Return 不会寄回库存装备。
-- 只升级从未领取、从未冻结且不存在关联 delivery 的初始 pending 记录。
-- 已冻结、历史成功、结果未知与有过领取的记录保留原身份，交由人工核对，禁止自动重发。
LOCK TABLE public.settlement_obligations, public.settlement_obligation_fulfillments,
    public.deliveries IN SHARE ROW EXCLUSIVE MODE;

WITH converted AS (
    UPDATE public.settlement_obligation_fulfillments AS fulfillment
    SET delivery_operation = 'send',
        state_version = fulfillment.state_version + 1,
        updated_at = statement_timestamp()
    FROM public.settlement_obligations AS obligation
    WHERE obligation.id = fulfillment.obligation_id
      AND obligation.obligation_kind = 'unsold_return'
      AND obligation.business_state = 'ready'
      AND fulfillment.delivery_operation = 'return'
      AND fulfillment.execution_state = 'pending'
      AND fulfillment.state_version = 1
      AND fulfillment.claim_token IS NULL
      AND fulfillment.claimed_at IS NULL
      AND fulfillment.claim_expires_at IS NULL
      AND fulfillment.claim_fencing = 0
      AND fulfillment.retry_count = 0
      AND fulfillment.next_attempt_at IS NULL
      AND fulfillment.last_error_code IS NULL
      AND fulfillment.delivery_id IS NULL
      AND fulfillment.prepare_command_id IS NULL
      AND fulfillment.prepare_request_digest IS NULL
      AND fulfillment.activate_command_id IS NULL
      AND fulfillment.activate_request_digest IS NULL
      AND fulfillment.proof_kind IS NULL
      AND fulfillment.proof_reference IS NULL
      AND fulfillment.proof_digest IS NULL
      AND fulfillment.proof_observed_at IS NULL
      AND fulfillment.completed_at IS NULL
      AND NOT EXISTS (
          SELECT 1
          FROM public.deliveries AS delivery
          WHERE delivery.origin_kind = 'settlement'
            AND delivery.origin_id = obligation.id::text
      )
    RETURNING fulfillment.obligation_id, fulfillment.updated_at
)
INSERT INTO public.audit_records
    (id, occurred_at, actor_type, actor_id, action, entity_type, entity_id,
     result, source, metadata)
SELECT gen_random_uuid(), converted.updated_at, 'system', 'auction-migration',
       'auction.fulfillment.dispatch_updated', 'settlement_obligation',
       converted.obligation_id::text, 'success', 'system',
       '{"migration":"000038_unsold_return_send","previous_operation":"return","operation":"send","reason":"unclaimed_unfrozen_no_delivery"}'::jsonb
FROM converted;
