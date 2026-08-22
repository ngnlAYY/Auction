ALTER TABLE public.auctions
    ADD COLUMN policy_snapshot jsonb NOT NULL DEFAULT (
        jsonb_build_object(
            'schema_version', 4,
            'start_price', 50000,
            'bid_increment_ppm', 0,
            'minimum_increment', 30000,
            'soft_close_window_seconds', 900,
            'soft_close_extension_seconds', 900,
            'proxy_enabled', true,
            'proxy_default_mode', 'fast',
            'proxy_slow_interval_seconds', 600,
            'proxy_max_outbid_uses', 0,
            'final_phase_lead_seconds', 3600,
            'scheduled_end_at', NULL,
            'settlement', jsonb_build_object(
                'version', 1,
                'mode', 'hold_curve_v1',
                'compensation_pool_enabled', true,
                'isekai_carrier_credit', 1,
                'anchors', jsonb_build_array(
                    jsonb_build_object('hold_ppm', 0, 'adjustment_ppm', 0),
                    jsonb_build_object('hold_ppm', 1000000, 'adjustment_ppm', 0)
                )
            ),
            'recovery', jsonb_build_object(
                'version', 1,
                'mode', 'remind_v1',
                'payment_timeout_seconds', 604800,
                'reminder_limit', 1
            )
        )
    ),
    ADD COLUMN scheduled_open_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    ADD COLUMN settled_at timestamptz,
    ADD CONSTRAINT auctions_policy_snapshot_object_check
        CHECK (jsonb_typeof(policy_snapshot) = 'object');

CREATE TABLE public.inbound_mail (
    id uuid PRIMARY KEY,
    mailbox_key text NOT NULL CHECK (mailbox_key = btrim(mailbox_key) AND mailbox_key <> ''),
    external_mail_id text NOT NULL
        CHECK (external_mail_id = btrim(external_mail_id) AND external_mail_id <> ''),
    world_id text NOT NULL REFERENCES public.worlds(id) ON DELETE RESTRICT,
    mail_kind text NOT NULL CHECK (
        mail_kind IN ('equipment', 'bid', 'delivery', 'registration', 'rebind', 'unknown')
    ),
    content_digest bytea NOT NULL CHECK (octet_length(content_digest) = 32),
    received_at timestamptz NOT NULL,
    status text NOT NULL CHECK (status IN ('accepted', 'rejected', 'failed')),
    decision_code text NOT NULL CHECK (decision_code = btrim(decision_code) AND decision_code <> ''),
    decision_metadata jsonb NOT NULL DEFAULT '{}'::jsonb
        CHECK (jsonb_typeof(decision_metadata) = 'object'),
    processed_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    UNIQUE (mailbox_key, external_mail_id),
    CHECK (processed_at >= received_at)
);

CREATE INDEX inbound_mail_poll_idx
    ON public.inbound_mail (mailbox_key, received_at, id);

CREATE TABLE public.auction_submissions (
    id uuid PRIMARY KEY,
    source_command_id uuid NOT NULL UNIQUE
        REFERENCES public.auction_commands(command_id) ON DELETE RESTRICT,
    auction_id uuid NOT NULL REFERENCES public.auctions(id) ON DELETE RESTRICT,
    mailbox_key text NOT NULL CHECK (mailbox_key = btrim(mailbox_key) AND mailbox_key <> ''),
    external_mail_id text NOT NULL
        CHECK (external_mail_id = btrim(external_mail_id) AND external_mail_id <> ''),
    seller text NOT NULL CHECK (seller = btrim(seller) AND seller <> ''),
    received_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    UNIQUE (mailbox_key, external_mail_id)
);

CREATE INDEX auction_submissions_auction_idx
    ON public.auction_submissions (auction_id, received_at, id);

ALTER TABLE public.lots
    ADD COLUMN leader_since timestamptz,
    ADD COLUMN submission_id uuid
        REFERENCES public.auction_submissions(id) ON DELETE RESTRICT,
    ADD CONSTRAINT lots_auction_id_id_key UNIQUE (auction_id, id),
    ADD CONSTRAINT lots_leader_since_requires_leader
        CHECK (leader_since IS NULL OR leader_user_id IS NOT NULL);

CREATE INDEX lots_submission_idx
    ON public.lots (submission_id, lot_number)
    WHERE submission_id IS NOT NULL;

CREATE TABLE public.settlement_pools (
    world_id text PRIMARY KEY REFERENCES public.worlds(id) ON DELETE RESTRICT,
    balance bigint NOT NULL DEFAULT 0 CHECK (balance >= 0),
    state_version bigint NOT NULL DEFAULT 1 CHECK (state_version > 0),
    created_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    updated_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    CHECK (updated_at >= created_at)
);

INSERT INTO public.settlement_pools (world_id)
SELECT id FROM public.worlds
ON CONFLICT (world_id) DO NOTHING;

CREATE TABLE public.auction_settlements (
    auction_id uuid PRIMARY KEY REFERENCES public.auctions(id) ON DELETE RESTRICT,
    command_id uuid NOT NULL UNIQUE
        REFERENCES public.auction_commands(command_id) ON DELETE RESTRICT,
    policy_version integer NOT NULL CHECK (policy_version >= 0),
    policy_mode text NOT NULL CHECK (policy_mode IN ('legacy_zero_v0', 'hold_curve_v1')),
    pool_enabled boolean NOT NULL,
    opening_pool_balance bigint NOT NULL CHECK (opening_pool_balance >= 0),
    closing_pool_balance bigint NOT NULL CHECK (closing_pool_balance >= 0),
    tax_total bigint NOT NULL DEFAULT 0 CHECK (tax_total >= 0),
    compensation_total bigint NOT NULL DEFAULT 0 CHECK (compensation_total >= 0),
    opening_pool_version bigint NOT NULL DEFAULT 1 CHECK (opening_pool_version > 0),
    closing_pool_version bigint NOT NULL DEFAULT 1 CHECK (closing_pool_version > 0),
    settled_at timestamptz NOT NULL,
    CHECK (
        (policy_version = 0 AND policy_mode = 'legacy_zero_v0')
        OR (policy_version = 1 AND policy_mode = 'hold_curve_v1')
    ),
    CHECK (closing_pool_balance <= opening_pool_balance + tax_total)
);

CREATE TABLE public.lot_settlements (
    lot_id uuid PRIMARY KEY REFERENCES public.lots(id) ON DELETE RESTRICT,
    end_price bigint NOT NULL CHECK (end_price >= 0),
    listed_price bigint NOT NULL CHECK (listed_price >= 0),
    buyer_amount bigint NOT NULL CHECK (buyer_amount >= 0),
    seller_amount bigint NOT NULL CHECK (seller_amount >= 0),
    tax_amount bigint NOT NULL CHECK (tax_amount >= 0),
    compensation_amount bigint NOT NULL CHECK (compensation_amount >= 0),
    hold_ppm bigint NOT NULL CHECK (hold_ppm BETWEEN 0 AND 1000000),
    adjustment_ppm bigint NOT NULL CHECK (adjustment_ppm BETWEEN -1000000 AND 1000000),
    adjustment_metadata jsonb NOT NULL DEFAULT '{}'::jsonb
        CHECK (jsonb_typeof(adjustment_metadata) = 'object'),
    CHECK (buyer_amount + compensation_amount = seller_amount + tax_amount),
    CHECK (
        (
            end_price = 0
            AND listed_price = 0
            AND buyer_amount = 0
            AND seller_amount = 0
            AND tax_amount = 0
            AND compensation_amount = 0
        )
        OR (
            end_price > 0
            AND listed_price > 0
            AND seller_amount = listed_price
        )
    )
);

CREATE TABLE public.compensation_ledger (
    id uuid PRIMARY KEY,
    auction_id uuid NOT NULL REFERENCES public.auctions(id) ON DELETE RESTRICT,
    lot_id uuid REFERENCES public.lots(id) ON DELETE RESTRICT,
    entry_sequence integer NOT NULL CHECK (entry_sequence > 0),
    movement_kind text NOT NULL CHECK (
        movement_kind IN ('opening', 'tax', 'compensation', 'closing')
    ),
    amount bigint NOT NULL,
    balance_after bigint NOT NULL CHECK (balance_after >= 0),
    created_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    UNIQUE (auction_id, entry_sequence)
);

CREATE INDEX compensation_ledger_order_idx
    ON public.compensation_ledger (auction_id, entry_sequence);

CREATE TABLE public.deliveries (
    id uuid PRIMARY KEY,
    command_id uuid NOT NULL UNIQUE,
    request_digest bytea NOT NULL CHECK (octet_length(request_digest) = 32),
    origin_kind text NOT NULL CHECK (
        origin_kind IN ('settlement', 'settlement_recovery_action', 'user_key', 'manual', 'mail_intake')
    ),
    origin_id text NOT NULL CHECK (origin_id = btrim(origin_id) AND origin_id <> ''),
    recipient text NOT NULL CHECK (recipient = btrim(recipient) AND recipient <> ''),
    payload jsonb NOT NULL CHECK (jsonb_typeof(payload) = 'object'),
    status text NOT NULL CHECK (
        status IN ('prepared', 'pending', 'claimed', 'sending', 'succeeded', 'failed', 'manual_reconcile')
    ),
    lease_token uuid,
    lease_expires_at timestamptz,
    fencing_version bigint NOT NULL DEFAULT 0 CHECK (fencing_version >= 0),
    attempt_count integer NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    claimed_at timestamptz,
    sending_at timestamptz,
    completed_at timestamptz,
    external_reference text,
    failure_code text,
    result_metadata jsonb NOT NULL DEFAULT '{}'::jsonb
        CHECK (jsonb_typeof(result_metadata) = 'object'),
    activation_command_id uuid,
    activation_request_digest bytea,
    activated_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    updated_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    CHECK (updated_at >= created_at),
    CHECK (
        (status IN ('claimed', 'sending') AND lease_token IS NOT NULL AND lease_expires_at IS NOT NULL)
        OR (
            status IN ('prepared', 'pending', 'succeeded', 'failed', 'manual_reconcile')
            AND lease_token IS NULL
            AND lease_expires_at IS NULL
        )
    ),
    CHECK (
        (activation_command_id IS NULL AND activation_request_digest IS NULL AND activated_at IS NULL)
        OR (
            activation_command_id IS NOT NULL
            AND activation_request_digest IS NOT NULL
            AND octet_length(activation_request_digest) = 32
            AND activated_at IS NOT NULL
        )
    ),
    CHECK (
        (status IN ('prepared', 'pending', 'claimed', 'sending') AND completed_at IS NULL)
        OR (status IN ('succeeded', 'failed', 'manual_reconcile') AND completed_at IS NOT NULL)
    ),
    CHECK (
        status <> 'succeeded'
        OR (external_reference IS NOT NULL AND failure_code IS NULL)
    ),
    CHECK (
        status NOT IN ('failed', 'manual_reconcile')
        OR failure_code IS NOT NULL
    )
);

CREATE INDEX deliveries_claim_idx
    ON public.deliveries (created_at, id)
    WHERE status IN ('prepared', 'pending', 'claimed');

CREATE INDEX deliveries_lease_expiry_idx
    ON public.deliveries (lease_expires_at, id)
    WHERE status IN ('claimed', 'sending');

CREATE TABLE public.settlement_obligations (
    id uuid PRIMARY KEY,
    auction_id uuid NOT NULL REFERENCES public.auctions(id) ON DELETE RESTRICT,
    lot_id uuid NOT NULL,
    world_id text NOT NULL REFERENCES public.worlds(id) ON DELETE RESTRICT,
    origin_command_id uuid NOT NULL
        REFERENCES public.auction_commands(command_id) ON DELETE RESTRICT
        DEFERRABLE INITIALLY DEFERRED,
    policy_version integer NOT NULL CHECK (policy_version >= 0),
    obligation_kind text NOT NULL CHECK (
        obligation_kind IN (
            'winner_equipment_cod',
            'winner_payment_carrier_cod',
            'winner_equipment_handoff',
            'seller_payout',
            'buyer_refund',
            'unsold_return',
            'withdrawn_return'
        )
    ),
    business_state text NOT NULL CHECK (business_state IN ('ready', 'blocked', 'contingent')),
    state_version bigint NOT NULL DEFAULT 1 CHECK (state_version > 0),
    recipient_user_id uuid REFERENCES public.users(id) ON DELETE RESTRICT,
    recipient_game_identity text,
    amount bigint NOT NULL DEFAULT 0 CHECK (amount >= 0),
    carrier_credit bigint NOT NULL DEFAULT 0 CHECK (carrier_credit >= 0),
    equipment_id uuid NOT NULL REFERENCES public.equipment(id) ON DELETE RESTRICT,
    submission_id uuid REFERENCES public.auction_submissions(id) ON DELETE RESTRICT,
    prerequisite_obligation_id uuid
        REFERENCES public.settlement_obligations(id) ON DELETE RESTRICT
        DEFERRABLE INITIALLY DEFERRED,
    created_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    updated_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    UNIQUE (auction_id, lot_id, obligation_kind),
    FOREIGN KEY (auction_id, lot_id)
        REFERENCES public.lots (auction_id, id) ON DELETE RESTRICT,
    CHECK (updated_at >= created_at),
    CHECK (
        (recipient_user_id IS NOT NULL) <> (recipient_game_identity IS NOT NULL)
    ),
    CHECK (
        recipient_game_identity IS NULL
        OR (
            recipient_game_identity = btrim(recipient_game_identity)
            AND recipient_game_identity <> ''
        )
    ),
    CHECK (
        (
            obligation_kind = 'winner_equipment_cod'
            AND world_id = 'main'
            AND business_state = 'ready'
            AND recipient_user_id IS NOT NULL
            AND amount > 0
            AND carrier_credit = 0
            AND prerequisite_obligation_id IS NULL
        )
        OR (
            obligation_kind = 'winner_payment_carrier_cod'
            AND world_id = 'isekai'
            AND business_state = 'ready'
            AND recipient_user_id IS NOT NULL
            AND amount > 0
            AND carrier_credit = 1
            AND prerequisite_obligation_id IS NULL
        )
        OR (
            obligation_kind = 'winner_equipment_handoff'
            AND world_id = 'isekai'
            AND business_state IN ('blocked', 'ready')
            AND recipient_user_id IS NOT NULL
            AND amount = 0
            AND carrier_credit = 0
            AND prerequisite_obligation_id IS NOT NULL
        )
        OR (
            obligation_kind = 'seller_payout'
            AND business_state IN ('blocked', 'ready')
            AND recipient_game_identity IS NOT NULL
            AND amount > 0
            AND carrier_credit = 0
            AND prerequisite_obligation_id IS NOT NULL
        )
        OR (
            obligation_kind = 'buyer_refund'
            AND business_state IN ('contingent', 'ready')
            AND recipient_user_id IS NOT NULL
            AND amount > 0
            AND carrier_credit = 0
            AND prerequisite_obligation_id IS NOT NULL
        )
        OR (
            obligation_kind IN ('unsold_return', 'withdrawn_return')
            AND business_state = 'ready'
            AND recipient_game_identity IS NOT NULL
            AND amount = 0
            AND carrier_credit = 0
            AND prerequisite_obligation_id IS NULL
        )
    )
);

CREATE INDEX settlement_obligations_read_idx
    ON public.settlement_obligations (
        auction_id, business_state, obligation_kind, lot_id, id
    );

CREATE TABLE public.settlement_obligation_fulfillments (
    obligation_id uuid PRIMARY KEY
        REFERENCES public.settlement_obligations(id) ON DELETE RESTRICT,
    execution_state text NOT NULL CHECK (
        execution_state IN ('pending', 'prepared', 'enqueued', 'satisfied', 'failed', 'manual_reconcile')
    ),
    state_version bigint NOT NULL DEFAULT 1 CHECK (state_version > 0),
    claim_token uuid,
    claimed_at timestamptz,
    claim_expires_at timestamptz,
    claim_fencing bigint NOT NULL DEFAULT 0 CHECK (claim_fencing >= 0),
    retry_count integer NOT NULL DEFAULT 0 CHECK (retry_count >= 0),
    next_attempt_at timestamptz,
    dispatch_world text NOT NULL REFERENCES public.worlds(id) ON DELETE RESTRICT,
    delivery_operation text NOT NULL CHECK (delivery_operation IN ('send', 'return')),
    delivery_id uuid UNIQUE REFERENCES public.deliveries(id) ON DELETE RESTRICT,
    prepare_command_id uuid UNIQUE,
    prepare_request_digest bytea CHECK (
        prepare_request_digest IS NULL OR octet_length(prepare_request_digest) = 32
    ),
    activate_command_id uuid UNIQUE,
    activate_request_digest bytea CHECK (
        activate_request_digest IS NULL OR octet_length(activate_request_digest) = 32
    ),
    proof_kind text,
    proof_reference text,
    proof_digest bytea CHECK (proof_digest IS NULL OR octet_length(proof_digest) = 32),
    proof_observed_at timestamptz,
    last_error_code text,
    created_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    updated_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    completed_at timestamptz,
    CHECK (updated_at >= created_at),
    CHECK (
        (claim_token IS NULL AND claimed_at IS NULL AND claim_expires_at IS NULL)
        OR (
            claim_token IS NOT NULL
            AND claimed_at IS NOT NULL
            AND claim_expires_at IS NOT NULL
            AND claim_expires_at > claimed_at
        )
    ),
    CHECK (claim_token IS NULL OR next_attempt_at IS NULL),
    CHECK (
        (execution_state IN ('satisfied', 'failed', 'manual_reconcile')
            AND completed_at IS NOT NULL)
        OR (execution_state IN ('pending', 'prepared', 'enqueued')
            AND completed_at IS NULL)
    ),
    CHECK (
        (proof_kind IS NULL AND proof_reference IS NULL
            AND proof_digest IS NULL AND proof_observed_at IS NULL)
        OR (proof_kind IS NOT NULL AND proof_reference IS NOT NULL
            AND proof_digest IS NOT NULL AND proof_observed_at IS NOT NULL)
    )
);

CREATE INDEX settlement_obligation_fulfillments_claim_idx
    ON public.settlement_obligation_fulfillments (next_attempt_at, created_at, obligation_id)
    WHERE execution_state IN ('pending', 'prepared', 'enqueued');

CREATE INDEX settlement_obligation_fulfillments_lease_idx
    ON public.settlement_obligation_fulfillments (claim_expires_at, obligation_id)
    WHERE claim_token IS NOT NULL;

UPDATE public.auction_schema_identity
SET baseline_version = 2
WHERE singleton;
