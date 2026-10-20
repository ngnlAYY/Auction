-- 退件消费是追加事实；重寄清空 recovery 当前指针后，原邮件仍不得再次计数。
CREATE TABLE public.settlement_recovery_return_consumptions (
    return_mail_id uuid PRIMARY KEY REFERENCES public.inbound_mail(id),
    recovery_id uuid NOT NULL REFERENCES public.settlement_recoveries(id),
    recorded_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX settlement_recovery_return_consumptions_recovery_idx
    ON public.settlement_recovery_return_consumptions (recovery_id, return_mail_id);

-- 只回填可由当前指针、回收动作或原 take 投递证明已消费的邮件；不重算旧计数。
-- 历史 take 保留了被后续轮次覆盖的 source MID，覆盖重寄后当前指针已清空的窗口。
WITH consumed AS (
    SELECT recovery.return_mail_id, recovery.id AS recovery_id
    FROM public.settlement_recoveries AS recovery
    WHERE recovery.return_mail_id IS NOT NULL
    UNION
    SELECT action.source_mail_id, action.recovery_id
    FROM public.settlement_recovery_actions AS action
    WHERE action.kind IN ('reclaim_equipment_take', 'cancel_carrier_take', 'recover_equipment_take')
      AND action.source_mail_id IS NOT NULL
    UNION
    SELECT inbound.id, recovery.id
    FROM public.deliveries AS delivery
    JOIN public.settlement_recoveries AS recovery
      ON delivery.origin_id = 'recovery:' || recovery.id::text
    JOIN public.inbound_mail AS inbound
      ON inbound.world_id = delivery.payload ->> 'world'
     AND inbound.external_mail_id = delivery.payload ->> 'external_mail_id'
    WHERE delivery.origin_kind = 'settlement_recovery_action'
      AND delivery.payload ->> 'operation' = 'take'
      AND inbound.mail_kind = 'delivery'
      AND inbound.status = 'accepted'
      AND inbound.decision_code = 'settlement_recovery_returned'
      AND inbound.decision_metadata ->> 'obligation_id' IN (
          recovery.payment_obligation_id::text, recovery.equipment_obligation_id::text
      )
)
INSERT INTO public.settlement_recovery_return_consumptions (return_mail_id, recovery_id)
SELECT return_mail_id, recovery_id FROM consumed;

CREATE FUNCTION public.reject_settlement_recovery_return_consumption_mutation()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    RAISE EXCEPTION 'settlement recovery return consumptions are append-only'
        USING ERRCODE = '23514';
END;
$$;

CREATE TRIGGER settlement_recovery_return_consumptions_append_only
BEFORE UPDATE OR DELETE ON public.settlement_recovery_return_consumptions
FOR EACH ROW EXECUTE FUNCTION public.reject_settlement_recovery_return_consumption_mutation();
