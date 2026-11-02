-- 操作者承担装备损失的人工处置事实；不构成 COD 收款或线下买方补偿完成证明。
-- 升级前停止全部旧角色；原投递永久退役，卖家仍通过正常履约队列付款。
ALTER TABLE public.settlement_funding_sources DROP CONSTRAINT settlement_funding_sources_kind_check;
ALTER TABLE public.settlement_funding_sources ADD CONSTRAINT settlement_funding_sources_kind_check
    CHECK (kind IN ('reconciliation','collected_payment','returned_delivery',
                   'record_opening','record_internal_purchase','record_compensation','record_carrier',
                   'record_operator_equipment_loss'));
ALTER TABLE public.settlement_funding_lots DROP CONSTRAINT settlement_funding_lots_source_kind_check;
ALTER TABLE public.settlement_funding_lots ADD CONSTRAINT settlement_funding_lots_source_kind_check
    CHECK (source_kind IN ('internal','collected','operator_equipment_loss'));

CREATE TABLE public.settlement_equipment_losses (
    command_id uuid PRIMARY KEY,
    lot_id uuid NOT NULL UNIQUE REFERENCES public.lot_settlements(lot_id) ON DELETE RESTRICT,
    recovery_id uuid NOT NULL UNIQUE REFERENCES public.settlement_recoveries(id) ON DELETE RESTRICT,
    payment_obligation_id uuid NOT NULL UNIQUE REFERENCES public.settlement_obligations(id) ON DELETE RESTRICT,
    delivery_id uuid NOT NULL UNIQUE REFERENCES public.deliveries(id) ON DELETE RESTRICT,
    seller_obligation_id uuid NOT NULL UNIQUE REFERENCES public.settlement_obligations(id) ON DELETE RESTRICT,
    refund_obligation_id uuid NOT NULL UNIQUE REFERENCES public.settlement_obligations(id) ON DELETE RESTRICT,
    uid text NOT NULL REFERENCES public.settlement_funding_accounts(uid) ON DELETE RESTRICT,
    actor text NOT NULL CHECK (actor=btrim(actor) AND length(actor) BETWEEN 1 AND 128),
    reason text NOT NULL CHECK (reason=btrim(reason) AND length(reason) BETWEEN 1 AND 2048),
    buyer_compensation_external boolean NOT NULL CHECK (buyer_compensation_external),
    expected_recovery_version bigint NOT NULL CHECK (expected_recovery_version>0),
    expected_payment_version bigint NOT NULL CHECK (expected_payment_version>0),
    expected_delivery_fencing_version bigint NOT NULL CHECK (expected_delivery_fencing_version>=0),
    buyer_amount bigint NOT NULL CHECK (buyer_amount>0),
    compensation_amount bigint NOT NULL CHECK (compensation_amount>=0),
    seller_amount bigint NOT NULL CHECK (seller_amount>0),
    tax_amount bigint NOT NULL CHECK (tax_amount>=0),
    request_digest bytea NOT NULL CHECK (octet_length(request_digest)=32),
    original_snapshot jsonb NOT NULL CHECK (jsonb_typeof(original_snapshot)='object'),
    result jsonb NOT NULL CHECK (jsonb_typeof(result)='object'),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    CHECK (buyer_amount::numeric+compensation_amount::numeric=seller_amount::numeric+tax_amount::numeric)
);
CREATE TRIGGER settlement_equipment_losses_append_only
BEFORE UPDATE OR DELETE ON public.settlement_equipment_losses
FOR EACH ROW EXECUTE FUNCTION public.reject_compensation_fact_mutation();
COMMENT ON TABLE public.settlement_equipment_losses IS
    '人工装备损失与外部补偿责任声明；不是 Sent、COD 实收或买方补偿完成回执';
