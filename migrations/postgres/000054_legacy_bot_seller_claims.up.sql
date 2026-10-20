-- 旧已收款只从开户隔离责任转移，不重复发行收入。每个付款/MID/lot/卖家义务只能认领一次。
CREATE TABLE public.settlement_legacy_bot_seller_claims (
    command_id uuid PRIMARY KEY,
    lot_id uuid NOT NULL UNIQUE REFERENCES public.lots(id),
    obligation_id uuid NOT NULL UNIQUE REFERENCES public.settlement_obligations(id),
    payment_obligation_id uuid NOT NULL UNIQUE REFERENCES public.settlement_obligations(id),
    payment_delivery_id uuid NOT NULL UNIQUE REFERENCES public.deliveries(id),
    uid text NOT NULL REFERENCES public.settlement_funding_accounts(uid),
    external_mail_id text NOT NULL CHECK (external_mail_id ~ '^[1-9][0-9]*$'),
    request_digest bytea NOT NULL CHECK (octet_length(request_digest) = 32),
    snapshot_digest bytea NOT NULL CHECK (octet_length(snapshot_digest) = 32),
    evidence_digest bytea NOT NULL CHECK (octet_length(evidence_digest) = 32),
    evidence jsonb NOT NULL CHECK (jsonb_typeof(evidence) = 'object'),
    original_snapshot jsonb NOT NULL CHECK (jsonb_typeof(original_snapshot) = 'object'),
    result jsonb NOT NULL CHECK (jsonb_typeof(result) = 'object'),
    actor_id text NOT NULL CHECK (actor_id = btrim(actor_id) AND actor_id <> ''),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    UNIQUE (uid, external_mail_id)
);

CREATE FUNCTION public.reject_legacy_bot_seller_claim_mutation() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'legacy bot seller claims are append-only' USING ERRCODE = '23514';
END;
$$;
CREATE TRIGGER settlement_legacy_bot_seller_claims_append_only
BEFORE UPDATE OR DELETE ON public.settlement_legacy_bot_seller_claims
FOR EACH ROW EXECUTE FUNCTION public.reject_legacy_bot_seller_claim_mutation();
