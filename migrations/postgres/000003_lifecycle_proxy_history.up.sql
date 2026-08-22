ALTER TABLE public.auctions
    ADD COLUMN preview_scheduled_at timestamptz,
    ADD COLUMN preview_opened_at timestamptz,
    ADD COLUMN preview_manual_open boolean NOT NULL DEFAULT false,
    ADD COLUMN preview_revision bigint NOT NULL DEFAULT 0 CHECK (preview_revision >= 0),
    ADD COLUMN opened_at timestamptz,
    ADD COLUMN final_phase_at timestamptz,
    ADD COLUMN predecessor_auction_id uuid,
    ADD COLUMN cancelled_at timestamptz,
    ADD COLUMN cancelled_reason text,
    ADD COLUMN cancelled_by text,
    ADD COLUMN settlement_workflow_id uuid,
    ADD COLUMN settlement_claim_command_id uuid
        REFERENCES public.auction_commands(command_id) ON DELETE RESTRICT
        DEFERRABLE INITIALLY DEFERRED,
    ADD COLUMN settlement_complete_command_id uuid
        REFERENCES public.auction_commands(command_id) ON DELETE RESTRICT
        DEFERRABLE INITIALLY DEFERRED,
    ADD COLUMN settlement_due_at timestamptz,
    ADD COLUMN settlement_started_at timestamptz,
    ADD COLUMN settlement_attempt_count integer NOT NULL DEFAULT 0
        CHECK (settlement_attempt_count >= 0),
    ADD COLUMN settlement_next_attempt_at timestamptz,
    ADD COLUMN settlement_last_error_code text,
    ADD CONSTRAINT auctions_id_world_key UNIQUE (id, world_id),
    ADD CONSTRAINT auctions_id_world_status_key UNIQUE (id, world_id, status),
    ADD CONSTRAINT auctions_predecessor_not_self_check
        CHECK (predecessor_auction_id IS NULL OR predecessor_auction_id <> id),
    ADD CONSTRAINT auctions_predecessor_world_fkey
        FOREIGN KEY (predecessor_auction_id, world_id)
        REFERENCES public.auctions(id, world_id)
        ON DELETE RESTRICT
        DEFERRABLE INITIALLY DEFERRED,
    ADD CONSTRAINT auctions_cancellation_shape_check CHECK (
        (
            status = 'cancelled'
            AND cancelled_at IS NOT NULL
            AND cancelled_reason IS NOT NULL
            AND cancelled_reason = btrim(cancelled_reason)
            AND cancelled_reason <> ''
            AND cancelled_by IS NOT NULL
            AND cancelled_by = btrim(cancelled_by)
            AND cancelled_by <> ''
        )
        OR (
            status <> 'cancelled'
            AND cancelled_at IS NULL
            AND cancelled_reason IS NULL
            AND cancelled_by IS NULL
        )
    ),
    ADD CONSTRAINT auctions_settlement_progress_shape_check CHECK (
        settlement_attempt_count >= 0
        AND (
            (
                settlement_workflow_id IS NULL
                AND settlement_claim_command_id IS NULL
                AND settlement_complete_command_id IS NULL
                AND settlement_due_at IS NULL
                AND settlement_started_at IS NULL
                AND settlement_attempt_count = 0
                AND settlement_next_attempt_at IS NULL
                AND settlement_last_error_code IS NULL
            )
            OR (
                settlement_workflow_id IS NOT NULL
                AND settlement_claim_command_id IS NOT NULL
                AND settlement_due_at IS NOT NULL
                AND settlement_started_at IS NOT NULL
                AND settlement_started_at >= settlement_due_at
                AND settlement_attempt_count > 0
                AND (
                    settlement_last_error_code IS NULL
                    OR (
                        settlement_last_error_code = btrim(settlement_last_error_code)
                        AND settlement_last_error_code <> ''
                    )
                )
                AND (
                    settlement_complete_command_id IS NULL
                    OR status = 'settled'
                )
            )
        )
        AND (status <> 'settling' OR settlement_workflow_id IS NOT NULL)
    );

UPDATE public.auctions
SET preview_scheduled_at = scheduled_open_at
WHERE preview_scheduled_at IS NULL;

ALTER TABLE public.auctions
    ALTER COLUMN preview_scheduled_at SET NOT NULL,
    ADD CONSTRAINT auctions_preview_schedule_check
        CHECK (preview_scheduled_at <= scheduled_open_at),
    ADD CONSTRAINT auctions_preview_open_shape_check CHECK (
        preview_manual_open IS FALSE OR preview_opened_at IS NOT NULL
    ),
    ADD CONSTRAINT auctions_open_shape_check CHECK (
        opened_at IS NULL
        OR (preview_opened_at IS NOT NULL AND opened_at >= preview_opened_at)
    ),
    ADD CONSTRAINT auctions_final_phase_shape_check CHECK (
        final_phase_at IS NULL
        OR (opened_at IS NOT NULL AND final_phase_at >= opened_at)
    );

CREATE UNIQUE INDEX auctions_one_draft_world_idx
    ON public.auctions (world_id)
    WHERE status = 'draft';

CREATE UNIQUE INDEX auctions_one_successor_idx
    ON public.auctions (predecessor_auction_id)
    WHERE predecessor_auction_id IS NOT NULL;

UPDATE public.auctions AS auction
SET policy_snapshot = jsonb_set(
    auction.policy_snapshot,
    '{scheduled_end_at}',
    to_jsonb(COALESCE(
        auction.effective_end_at,
        auction.scheduled_end_at,
        auction.scheduled_open_at + interval '24 hours'
    )),
    true
)
WHERE auction.policy_snapshot ->> 'scheduled_end_at' IS NULL;

ALTER TABLE public.auctions
    ADD CONSTRAINT auctions_policy_snapshot_shape_check CHECK (
        jsonb_typeof(policy_snapshot) = 'object'
        AND policy_snapshot ?& ARRAY[
            'schema_version',
            'start_price',
            'bid_increment_ppm',
            'minimum_increment',
            'soft_close_window_seconds',
            'soft_close_extension_seconds',
            'proxy_enabled',
            'proxy_default_mode',
            'proxy_slow_interval_seconds',
            'proxy_max_outbid_uses',
            'final_phase_lead_seconds',
            'scheduled_end_at',
            'settlement',
            'recovery'
        ]
        AND (policy_snapshot ->> 'schema_version') = '4'
        AND (policy_snapshot ->> 'proxy_default_mode') IN ('fast', 'slow')
        AND (policy_snapshot ->> 'scheduled_end_at') IS NOT NULL
        AND jsonb_typeof(policy_snapshot -> 'settlement') = 'object'
        AND (policy_snapshot -> 'settlement' ->> 'version') IN ('0', '1')
        AND policy_snapshot -> 'settlement' ->> 'mode'
            IN ('legacy_zero_v0', 'hold_curve_v1')
        AND jsonb_typeof(policy_snapshot -> 'settlement' -> 'anchors') = 'array'
        AND jsonb_typeof(policy_snapshot -> 'recovery') = 'object'
        AND (policy_snapshot -> 'recovery' ->> 'version') = '1'
        AND policy_snapshot -> 'recovery' ->> 'mode' = 'remind_v1'
    );

CREATE TABLE public.auction_preview_items (
    auction_id uuid NOT NULL REFERENCES public.auctions(id) ON DELETE CASCADE,
    equipment_id uuid NOT NULL REFERENCES public.equipment(id) ON DELETE RESTRICT,
    display_order integer NOT NULL CHECK (display_order >= 0),
    start_price bigint NOT NULL CHECK (start_price > 0),
    source_revision bigint NOT NULL CHECK (source_revision > 0),
    collected_at timestamptz NOT NULL,
    PRIMARY KEY (auction_id, equipment_id),
    UNIQUE (auction_id, display_order)
);

CREATE TABLE public.scheduled_rounds (
    id uuid PRIMARY KEY,
    world_id text NOT NULL REFERENCES public.worlds(id) ON DELETE RESTRICT,
    local_week date NOT NULL,
    auction_id uuid UNIQUE REFERENCES public.auctions(id) ON DELETE RESTRICT,
    status text NOT NULL CHECK (status IN ('active', 'missed')),
    preview_due_at timestamptz NOT NULL,
    open_due_at timestamptz NOT NULL,
    scheduled_end_at timestamptz NOT NULL,
    create_command_id uuid NOT NULL UNIQUE
        REFERENCES public.auction_commands(command_id) ON DELETE RESTRICT
        DEFERRABLE INITIALLY DEFERRED,
    preview_command_id uuid UNIQUE
        REFERENCES public.auction_commands(command_id) ON DELETE RESTRICT
        DEFERRABLE INITIALLY DEFERRED,
    open_command_id uuid UNIQUE
        REFERENCES public.auction_commands(command_id) ON DELETE RESTRICT
        DEFERRABLE INITIALLY DEFERRED,
    preview_completed_at timestamptz,
    open_completed_at timestamptz,
    state_version bigint NOT NULL DEFAULT 1 CHECK (state_version > 0),
    created_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    updated_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    UNIQUE (world_id, local_week),
    CHECK (preview_due_at < open_due_at AND open_due_at < scheduled_end_at),
    CHECK (
        (status = 'active' AND auction_id IS NOT NULL)
        OR (status = 'missed' AND auction_id IS NULL)
    ),
    CHECK (
        (preview_completed_at IS NULL AND preview_command_id IS NULL)
        OR (preview_completed_at IS NOT NULL AND preview_command_id IS NOT NULL)
    ),
    CHECK (
        (open_completed_at IS NULL AND open_command_id IS NULL)
        OR (
            open_completed_at IS NOT NULL
            AND open_command_id IS NOT NULL
            AND preview_completed_at IS NOT NULL
        )
    ),
    FOREIGN KEY (auction_id, world_id)
        REFERENCES public.auctions(id, world_id) ON DELETE RESTRICT
        DEFERRABLE INITIALLY DEFERRED
);

CREATE INDEX scheduled_rounds_incomplete_idx
    ON public.scheduled_rounds (local_week, preview_due_at, open_due_at, world_id)
    WHERE status = 'active' AND open_completed_at IS NULL;

ALTER TABLE public.auction_submissions
    ADD COLUMN source_kind text NOT NULL DEFAULT 'mail'
        CHECK (source_kind IN ('mail', 'admin')),
    ADD COLUMN source_reference text,
    ADD COLUMN source_fingerprint bytea,
    ALTER COLUMN mailbox_key DROP NOT NULL,
    ALTER COLUMN external_mail_id DROP NOT NULL,
    ADD CONSTRAINT auction_submissions_source_shape_check CHECK (
        (
            source_kind = 'mail'
            AND mailbox_key IS NOT NULL
            AND external_mail_id IS NOT NULL
            AND source_reference IS NULL
            AND source_fingerprint IS NULL
        )
        OR (
            source_kind = 'admin'
            AND mailbox_key IS NULL
            AND external_mail_id IS NULL
            AND source_reference IS NOT NULL
            AND source_reference = btrim(source_reference)
            AND source_reference <> ''
            AND source_fingerprint IS NOT NULL
            AND octet_length(source_fingerprint) = 32
        )
    );

CREATE UNIQUE INDEX auction_submissions_mail_identity_idx
    ON public.auction_submissions (mailbox_key, external_mail_id)
    WHERE source_kind = 'mail';

CREATE UNIQUE INDEX auction_submissions_admin_fingerprint_idx
    ON public.auction_submissions (source_fingerprint)
    WHERE source_kind = 'admin';

ALTER TABLE public.lots
    ADD CONSTRAINT lots_pending_submission_check
        CHECK (status <> 'pending' OR submission_id IS NOT NULL);

CREATE TABLE public.auction_intake_targets (
    world_id text PRIMARY KEY REFERENCES public.worlds(id) ON DELETE RESTRICT,
    auction_id uuid NOT NULL UNIQUE,
    target_status text NOT NULL DEFAULT 'draft' CHECK (target_status = 'draft'),
    state_version bigint NOT NULL DEFAULT 1 CHECK (state_version > 0),
    updated_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    FOREIGN KEY (auction_id, world_id, target_status)
        REFERENCES public.auctions(id, world_id, status)
        ON DELETE RESTRICT
        DEFERRABLE INITIALLY DEFERRED
);

CREATE TABLE public.equipment_secrets (
    equipment_id uuid PRIMARY KEY REFERENCES public.equipment(id) ON DELETE RESTRICT,
    ciphertext bytea NOT NULL CHECK (octet_length(ciphertext) > 0),
    nonce bytea NOT NULL CHECK (octet_length(nonce) = 12),
    encryption_key_id text NOT NULL
        CHECK (encryption_key_id = btrim(encryption_key_id) AND encryption_key_id <> ''),
    fingerprint text NOT NULL
        CHECK (fingerprint = btrim(fingerprint) AND fingerprint <> ''),
    created_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    updated_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    CHECK (updated_at >= created_at)
);

CREATE TABLE public.bid_proxy_outbid_uses (
    authorization_id uuid NOT NULL
        REFERENCES public.bid_proxy_authorizations(authorization_id) ON DELETE RESTRICT,
    use_number bigint NOT NULL CHECK (use_number > 0),
    command_id uuid NOT NULL UNIQUE
        REFERENCES public.auction_commands(command_id) ON DELETE RESTRICT
        DEFERRABLE INITIALLY DEFERRED,
    created_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    PRIMARY KEY (authorization_id, use_number)
);

CREATE INDEX bid_proxy_outbid_uses_command_idx
    ON public.bid_proxy_outbid_uses (command_id);

CREATE TABLE public.bid_proxy_attempts (
    authorization_id uuid NOT NULL
        REFERENCES public.bid_proxy_authorizations(authorization_id) ON DELETE RESTRICT,
    attempt_number bigint NOT NULL CHECK (attempt_number > 0),
    maximum_amount bigint NOT NULL CHECK (maximum_amount > 0),
    mode text NOT NULL CHECK (mode IN ('fast', 'slow')),
    command_id uuid NOT NULL UNIQUE
        REFERENCES public.auction_commands(command_id) ON DELETE RESTRICT
        DEFERRABLE INITIALLY DEFERRED,
    created_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    PRIMARY KEY (authorization_id, attempt_number)
);

CREATE INDEX bid_proxy_attempts_owner_order_idx
    ON public.bid_proxy_attempts (authorization_id, attempt_number);

ALTER TABLE public.bid_proxy_authorization_transitions
    ADD CONSTRAINT bid_proxy_authorization_transitions_matrix_check CHECK (
        (
            transition_kind = 'created'
            AND transition_number = 1
            AND old_maximum_amount IS NULL
            AND old_mode IS NULL
            AND old_status IS NULL
            AND new_status = 'active'
            AND reason IS NULL
            AND resulting_state_version = 1
        )
        OR (
            transition_kind = 'lowered'
            AND old_maximum_amount IS NOT NULL
            AND new_maximum_amount < old_maximum_amount
            AND old_mode = new_mode
            AND old_status = 'active'
            AND new_status = 'active'
            AND reason IS NULL
        )
        OR (
            transition_kind = 'final_fast'
            AND old_maximum_amount = new_maximum_amount
            AND old_mode = 'slow'
            AND new_mode = 'fast'
            AND old_status = 'active'
            AND new_status = 'final_fast'
            AND reason IS NULL
        )
        OR (
            transition_kind = 'exhausted'
            AND old_maximum_amount = new_maximum_amount
            AND old_mode = new_mode
            AND old_status IN ('active', 'final_fast')
            AND new_status = 'exhausted'
            AND reason IN ('strictly_defeated', 'ranked_below')
        )
        OR (
            transition_kind = 'frozen'
            AND old_maximum_amount = new_maximum_amount
            AND old_mode = new_mode
            AND old_status IN ('active', 'final_fast')
            AND new_status = 'frozen'
            AND reason = 'eligibility_lost'
        )
        OR (
            transition_kind = 'closed'
            AND old_maximum_amount = new_maximum_amount
            AND old_mode = new_mode
            AND old_status IN ('active', 'final_fast')
            AND new_status = 'closed'
            AND reason = 'competition_closed'
        )
    );

CREATE FUNCTION public.reject_bid_proxy_authorization_transition_mutation()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    RAISE EXCEPTION 'bid proxy authorization transitions are append-only'
        USING ERRCODE = '55000';
END;
$$;

CREATE TRIGGER bid_proxy_authorization_transitions_append_only
BEFORE UPDATE OR DELETE ON public.bid_proxy_authorization_transitions
FOR EACH ROW EXECUTE FUNCTION public.reject_bid_proxy_authorization_transition_mutation();

CREATE FUNCTION public.enforce_bid_proxy_authorization_transition_sequence()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    expected_transition_number bigint;
BEGIN
    PERFORM 1
    FROM public.bid_proxy_authorizations
    WHERE authorization_id = NEW.authorization_id
    FOR UPDATE;

    IF NOT FOUND THEN
        RAISE EXCEPTION 'proxy authorization does not exist for transition'
            USING ERRCODE = '23503';
    END IF;

    SELECT COALESCE(max(transition_number), 0) + 1
    INTO expected_transition_number
    FROM public.bid_proxy_authorization_transitions
    WHERE authorization_id = NEW.authorization_id;

    IF NEW.transition_number <> expected_transition_number THEN
        RAISE EXCEPTION 'proxy authorization transition sequence must be contiguous'
            USING ERRCODE = '23514';
    END IF;

    RETURN NEW;
END;
$$;

CREATE TRIGGER bid_proxy_authorization_transitions_contiguous
BEFORE INSERT ON public.bid_proxy_authorization_transitions
FOR EACH ROW EXECUTE FUNCTION public.enforce_bid_proxy_authorization_transition_sequence();
