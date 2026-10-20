-- 只追加经真实账号、干净草稿和精确库存核验的旧自寄处置；原投递及版本快照永久保留。
CREATE TABLE public.settlement_legacy_self_reconciliations (
    command_id uuid PRIMARY KEY,
    delivery_id uuid NOT NULL UNIQUE REFERENCES public.deliveries(id) ON DELETE RESTRICT,
    obligation_id uuid NOT NULL UNIQUE REFERENCES public.settlement_obligations(id) ON DELETE RESTRICT,
    action text NOT NULL CHECK (action IN ('keep_unsold', 'reopen_payment')),
    request_digest bytea NOT NULL CHECK (octet_length(request_digest) = 32),
    uid text NOT NULL CHECK (uid ~ '^[1-9][0-9]*$'),
    expected_delivery_fencing bigint NOT NULL CHECK (expected_delivery_fencing >= 0),
    expected_fulfillment_version bigint NOT NULL CHECK (expected_fulfillment_version > 0),
    expected_obligation_version bigint NOT NULL CHECK (expected_obligation_version > 0),
    snapshot_digest bytea NOT NULL CHECK (octet_length(snapshot_digest) = 32),
    original_snapshot jsonb NOT NULL CHECK (jsonb_typeof(original_snapshot) = 'object'),
    evidence_digest bytea NOT NULL CHECK (octet_length(evidence_digest) = 32),
    evidence jsonb NOT NULL CHECK (jsonb_typeof(evidence) = 'object'),
    result jsonb NOT NULL CHECK (jsonb_typeof(result) = 'object'),
    actor_id text NOT NULL CHECK (actor_id = btrim(actor_id) AND actor_id <> ''),
    reason text NOT NULL CHECK (reason = btrim(reason) AND reason <> ''),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp()
);

CREATE FUNCTION public.reject_settlement_legacy_self_reconciliation_mutation()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'settlement legacy self reconciliations are append only' USING ERRCODE = '23514';
END;
$$;

CREATE TRIGGER settlement_legacy_self_reconciliations_append_only
BEFORE UPDATE OR DELETE ON public.settlement_legacy_self_reconciliations
FOR EACH ROW EXECUTE FUNCTION public.reject_settlement_legacy_self_reconciliation_mutation();

COMMENT ON TABLE public.settlement_legacy_self_reconciliations IS
    '旧自寄原版本、确定未外发/当前库存证据及显式恢复结果；不冒充付款或Sent收款';
