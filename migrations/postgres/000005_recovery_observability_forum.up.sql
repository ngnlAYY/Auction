ALTER TABLE public.auction_settlements
    ADD COLUMN discount_demand bigint NOT NULL DEFAULT 0 CHECK (discount_demand >= 0);

ALTER TABLE public.service_heartbeats
    ADD COLUMN tracing_status text NOT NULL DEFAULT 'disabled'
        CHECK (tracing_status IN ('disabled', 'ok', 'degraded')),
    ADD COLUMN tracing_dropped_span_count bigint NOT NULL DEFAULT 0
        CHECK (tracing_dropped_span_count >= 0),
    ADD COLUMN tracing_export_failure_count bigint NOT NULL DEFAULT 0
        CHECK (tracing_export_failure_count >= 0),
    ADD COLUMN tracing_last_success_at timestamptz,
    ADD COLUMN tracing_last_failure_at timestamptz,
    ADD COLUMN tracing_last_error_code text NOT NULL DEFAULT ''
        CHECK (tracing_last_error_code IN ('', 'timeout', 'canceled', 'transport', 'rejected', 'shutdown'));

ALTER TABLE public.deliveries
    DROP CONSTRAINT deliveries_origin_kind_check,
    ADD CONSTRAINT deliveries_origin_kind_check CHECK (
        origin_kind IN (
            'settlement',
            'settlement_recovery_action',
            'user_key',
            'manual',
            'mail_intake',
            'operations_alert'
        )
    );

CREATE TABLE public.settlement_recoveries (
    id uuid PRIMARY KEY,
    auction_id uuid NOT NULL REFERENCES public.auctions(id) ON DELETE RESTRICT,
    lot_id uuid NOT NULL,
    world_id text NOT NULL REFERENCES public.worlds(id) ON DELETE RESTRICT
        CHECK (world_id IN ('main', 'isekai')),
    equipment_id uuid NOT NULL REFERENCES public.equipment(id) ON DELETE RESTRICT,
    winner_user_id uuid NOT NULL REFERENCES public.users(id) ON DELETE RESTRICT,
    payment_obligation_id uuid NOT NULL UNIQUE
        REFERENCES public.settlement_obligations(id) ON DELETE RESTRICT,
    equipment_obligation_id uuid UNIQUE
        REFERENCES public.settlement_obligations(id) ON DELETE RESTRICT,
    seller_payout_obligation_id uuid NOT NULL UNIQUE
        REFERENCES public.settlement_obligations(id) ON DELETE RESTRICT,
    buyer_refund_obligation_id uuid NOT NULL UNIQUE
        REFERENCES public.settlement_obligations(id) ON DELETE RESTRICT,
    policy_version integer NOT NULL CHECK (policy_version = 1),
    payment_deadline_at timestamptz NOT NULL,
    reminder_limit integer NOT NULL DEFAULT 1 CHECK (reminder_limit = 1),
    reminder_count integer NOT NULL DEFAULT 0
        CHECK (reminder_count BETWEEN 0 AND reminder_limit),
    stage text NOT NULL CHECK (
        stage IN (
            'awaiting_payment',
            'payment_overdue',
            'delivering_equipment',
            'reclaiming',
            'relist_pending',
            'refunding',
            'completed'
        )
    ),
    resolution text CHECK (
        resolution IS NULL
        OR resolution IN ('reclaimed', 'cancelled', 'equipment_recovered')
    ),
    state_version bigint NOT NULL DEFAULT 1 CHECK (state_version > 0),
    return_mail_id uuid UNIQUE REFERENCES public.inbound_mail(id) ON DELETE RESTRICT,
    return_external_mail_id text,
    custody_proof_kind text,
    custody_proof_reference text,
    custody_proof_digest bytea,
    custody_proof_observed_at timestamptz,
    blocking_code text,
    manual_reconcile_delivery_id uuid REFERENCES public.deliveries(id) ON DELETE RESTRICT,
    relisted_auction_id uuid,
    relisted_lot_id uuid,
    next_attempt_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    updated_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    completed_at timestamptz,
    UNIQUE (auction_id, lot_id),
    FOREIGN KEY (auction_id, lot_id)
        REFERENCES public.lots (auction_id, id) ON DELETE RESTRICT,
    FOREIGN KEY (relisted_auction_id, relisted_lot_id)
        REFERENCES public.lots (auction_id, id) ON DELETE RESTRICT,
    CHECK (
        (world_id = 'main' AND equipment_obligation_id IS NULL)
        OR (world_id = 'isekai' AND equipment_obligation_id IS NOT NULL)
    ),
    CHECK (payment_deadline_at >= created_at),
    CHECK (updated_at >= created_at),
    CHECK (next_attempt_at IS NULL OR next_attempt_at >= created_at),
    CHECK (completed_at IS NULL OR completed_at BETWEEN created_at AND updated_at),
    CHECK (
        (return_mail_id IS NULL AND return_external_mail_id IS NULL)
        OR (return_mail_id IS NOT NULL AND return_external_mail_id IS NOT NULL
            AND return_external_mail_id = btrim(return_external_mail_id)
            AND return_external_mail_id <> '')
    ),
    CHECK (
        (custody_proof_kind IS NULL AND custody_proof_reference IS NULL
            AND custody_proof_digest IS NULL AND custody_proof_observed_at IS NULL)
        OR (custody_proof_kind IS NOT NULL AND custody_proof_reference IS NOT NULL
            AND custody_proof_digest IS NOT NULL AND octet_length(custody_proof_digest) = 32
            AND custody_proof_observed_at IS NOT NULL
            AND custody_proof_observed_at BETWEEN created_at AND updated_at)
    ),
    CHECK (
        (relisted_auction_id IS NULL AND relisted_lot_id IS NULL)
        OR (relisted_auction_id IS NOT NULL AND relisted_lot_id IS NOT NULL)
    ),
    CHECK (manual_reconcile_delivery_id IS NULL OR blocking_code IS NOT NULL),
    CHECK ((stage = 'completed') = (completed_at IS NOT NULL)),
    CHECK (resolution IS NULL OR stage IN ('relist_pending', 'refunding', 'completed')),
    CHECK (stage <> 'refunding' OR (world_id = 'isekai' AND resolution = 'equipment_recovered')),
    CHECK (stage <> 'reclaiming' OR return_mail_id IS NOT NULL)
);

CREATE INDEX settlement_recoveries_due_idx
    ON public.settlement_recoveries (payment_deadline_at, created_at, id)
    WHERE stage IN ('awaiting_payment', 'payment_overdue');

CREATE INDEX settlement_recoveries_retry_idx
    ON public.settlement_recoveries (next_attempt_at, created_at, id)
    WHERE stage IN ('reclaiming', 'relist_pending', 'refunding');

CREATE TABLE public.settlement_recovery_actions (
    id uuid PRIMARY KEY,
    recovery_id uuid NOT NULL
        REFERENCES public.settlement_recoveries(id) ON DELETE RESTRICT,
    kind text NOT NULL CHECK (
        kind IN (
            'payment_reminder',
            'reclaim_equipment_take',
            'cancel_carrier_take',
            'recover_equipment_take',
            'buyer_blacklist'
        )
    ),
    ordinal integer NOT NULL CHECK (ordinal >= 0),
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
    source_mail_id uuid REFERENCES public.inbound_mail(id) ON DELETE RESTRICT,
    expected_credits bigint NOT NULL DEFAULT 0 CHECK (expected_credits >= 0),
    expected_cod bigint NOT NULL DEFAULT 0 CHECK (expected_cod >= 0),
    expected_attachment_ids text[] NOT NULL DEFAULT '{}'::text[],
    delivery_id uuid UNIQUE REFERENCES public.deliveries(id) ON DELETE RESTRICT,
    prepare_command_id uuid UNIQUE,
    prepare_request_digest bytea,
    activate_command_id uuid UNIQUE,
    activate_request_digest bytea,
    proof_kind text,
    proof_reference text,
    proof_digest bytea,
    proof_observed_at timestamptz,
    last_error_code text,
    created_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    updated_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    completed_at timestamptz,
    UNIQUE (recovery_id, kind),
    UNIQUE (recovery_id, ordinal),
    CHECK (updated_at >= created_at),
    CHECK (next_attempt_at IS NULL OR next_attempt_at >= created_at),
    CHECK (
        (claim_token IS NULL AND claimed_at IS NULL AND claim_expires_at IS NULL)
        OR (claim_token IS NOT NULL AND claimed_at IS NOT NULL
            AND claim_expires_at IS NOT NULL AND claim_expires_at > claimed_at)
    ),
    CHECK (claim_token IS NULL OR next_attempt_at IS NULL),
    CHECK (prepare_request_digest IS NULL OR octet_length(prepare_request_digest) = 32),
    CHECK (activate_request_digest IS NULL OR octet_length(activate_request_digest) = 32),
    CHECK (proof_digest IS NULL OR octet_length(proof_digest) = 32),
    CHECK (
        (proof_kind IS NULL AND proof_reference IS NULL
            AND proof_digest IS NULL AND proof_observed_at IS NULL)
        OR (proof_kind IS NOT NULL AND proof_reference IS NOT NULL
            AND proof_digest IS NOT NULL AND proof_observed_at IS NOT NULL)
    ),
    CHECK (
        (execution_state IN ('satisfied', 'failed', 'manual_reconcile')
            AND completed_at IS NOT NULL AND next_attempt_at IS NULL)
        OR (execution_state IN ('pending', 'prepared', 'enqueued') AND completed_at IS NULL)
    ),
    CHECK (execution_state <> 'satisfied' OR proof_kind IS NOT NULL),
    CHECK (execution_state NOT IN ('failed', 'manual_reconcile') OR last_error_code IS NOT NULL)
);

CREATE INDEX settlement_recovery_actions_claim_idx
    ON public.settlement_recovery_actions (next_attempt_at, created_at, recovery_id, ordinal, id)
    WHERE execution_state IN ('pending', 'prepared', 'enqueued');

CREATE INDEX settlement_recovery_actions_lease_idx
    ON public.settlement_recovery_actions (claim_expires_at, id)
    WHERE claim_token IS NOT NULL;

CREATE TABLE public.settlement_recovery_commands (
    command_id uuid PRIMARY KEY,
    recovery_id uuid NOT NULL
        REFERENCES public.settlement_recoveries(id) ON DELETE RESTRICT,
    expected_state_version bigint NOT NULL CHECK (expected_state_version > 0),
    action text NOT NULL CHECK (action IN ('reclaimed', 'cancelled', 'equipment_recovered')),
    request_digest bytea NOT NULL CHECK (octet_length(request_digest) = 32),
    result_code text NOT NULL CHECK (result_code = btrim(result_code) AND result_code <> ''),
    result_payload jsonb NOT NULL DEFAULT '{}'::jsonb
        CHECK (jsonb_typeof(result_payload) = 'object'),
    created_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    UNIQUE (recovery_id, action)
);

CREATE TABLE public.settlement_recovery_health (
    world_id text PRIMARY KEY REFERENCES public.worlds(id) ON DELETE RESTRICT
        CHECK (world_id IN ('main', 'isekai')),
    balance_credits bigint NOT NULL DEFAULT 0 CHECK (balance_credits >= 0),
    balance_observed_at timestamptz,
    balance_status text NOT NULL DEFAULT 'unknown'
        CHECK (balance_status IN ('unknown', 'ok', 'low', 'stale', 'unavailable')),
    evidence_last_success_at timestamptz,
    evidence_status text NOT NULL DEFAULT 'unknown'
        CHECK (evidence_status IN ('unknown', 'fresh', 'stale', 'unavailable')),
    payment_overdue_count bigint NOT NULL DEFAULT 0 CHECK (payment_overdue_count >= 0),
    relist_pending_count bigint NOT NULL DEFAULT 0 CHECK (relist_pending_count >= 0),
    failed_action_count bigint NOT NULL DEFAULT 0 CHECK (failed_action_count >= 0),
    manual_reconcile_count bigint NOT NULL DEFAULT 0 CHECK (manual_reconcile_count >= 0),
    oldest_overdue_age_seconds bigint NOT NULL DEFAULT 0 CHECK (oldest_overdue_age_seconds >= 0),
    updated_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    CHECK (balance_observed_at IS NULL OR balance_observed_at <= updated_at),
    CHECK (evidence_last_success_at IS NULL OR evidence_last_success_at <= updated_at)
);

INSERT INTO public.settlement_recovery_health (world_id)
SELECT id FROM public.worlds WHERE id IN ('main', 'isekai')
ON CONFLICT (world_id) DO NOTHING;

CREATE TABLE public.operations_alert_signals (
    fingerprint bytea PRIMARY KEY CHECK (octet_length(fingerprint) = 32),
    kind text NOT NULL CHECK (
        kind IN (
            'service_missing', 'service_degraded', 'audit_failure', 'source_unavailable',
            'delivery_pending', 'delivery_manual_reconcile', 'fulfillment_backlog',
            'fulfillment_failed', 'fulfillment_manual_reconcile', 'recovery_payment_overdue',
            'recovery_relist_pending', 'recovery_failed', 'recovery_manual_reconcile',
            'balance_low', 'evidence_stale'
        )
    ),
    scope text NOT NULL CHECK (
        scope IN ('public', 'admin', 'worker', 'audit', 'process', 'delivery',
            'auction', 'fulfillment', 'recovery', 'main', 'isekai')
    ),
    source text NOT NULL CHECK (source IN ('process', 'delivery', 'fulfillment', 'recovery')),
    active boolean NOT NULL,
    first_seen_at timestamptz NOT NULL,
    last_seen_at timestamptz NOT NULL,
    resolved_at timestamptz,
    last_notified_at timestamptz,
    next_notify_at timestamptz,
    count_value bigint NOT NULL CHECK (count_value >= 0),
    age_seconds bigint NOT NULL CHECK (age_seconds >= 0),
    state_version bigint NOT NULL CHECK (state_version > 0),
    CHECK (last_seen_at >= first_seen_at),
    CHECK (resolved_at IS NULL OR resolved_at >= first_seen_at),
    CHECK (last_notified_at IS NULL OR last_notified_at >= first_seen_at),
    CHECK (next_notify_at IS NULL OR next_notify_at >= first_seen_at),
    CHECK (
        (active AND resolved_at IS NULL AND next_notify_at IS NOT NULL)
        OR (NOT active AND resolved_at IS NOT NULL AND next_notify_at IS NULL)
    )
);

CREATE INDEX operations_alert_signals_due_idx
    ON public.operations_alert_signals (next_notify_at, kind, scope, fingerprint)
    WHERE active;

CREATE TABLE public.operations_alert_batches (
    id uuid PRIMARY KEY,
    command_id uuid NOT NULL UNIQUE,
    delivery_id uuid NOT NULL UNIQUE REFERENCES public.deliveries(id) ON DELETE RESTRICT,
    request_digest bytea NOT NULL CHECK (octet_length(request_digest) = 32),
    status text NOT NULL CHECK (
        status IN ('pending', 'claimed', 'enqueued', 'succeeded', 'failed', 'manual_reconcile')
    ),
    claim_token uuid,
    claim_fencing bigint NOT NULL DEFAULT 0 CHECK (claim_fencing >= 0),
    claimed_at timestamptz,
    claim_expires_at timestamptz,
    next_attempt_at timestamptz,
    attempt_count integer NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    item_count integer NOT NULL CHECK (item_count BETWEEN 1 AND 64),
    body_digest bytea NOT NULL CHECK (octet_length(body_digest) = 32),
    delivery_error_code text NOT NULL DEFAULT '',
    created_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    updated_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    completed_at timestamptz,
    CHECK (updated_at >= created_at),
    CHECK (claim_token IS NULL OR (claimed_at IS NOT NULL AND claim_expires_at > claimed_at)),
    CHECK (status IN ('succeeded', 'failed', 'manual_reconcile') = (completed_at IS NOT NULL)),
    CHECK (status IN ('failed', 'manual_reconcile') = (delivery_error_code <> ''))
);

CREATE INDEX operations_alert_batches_retry_idx
    ON public.operations_alert_batches (next_attempt_at, created_at, id)
    WHERE status = 'pending';

CREATE INDEX operations_alert_batches_lease_idx
    ON public.operations_alert_batches (claim_expires_at, id)
    WHERE status = 'claimed';

CREATE TABLE public.operations_alert_batch_items (
    batch_id uuid NOT NULL REFERENCES public.operations_alert_batches(id) ON DELETE RESTRICT,
    fingerprint bytea NOT NULL
        REFERENCES public.operations_alert_signals(fingerprint) ON DELETE RESTRICT,
    signal_version bigint NOT NULL CHECK (signal_version > 0),
    ordinal integer NOT NULL CHECK (ordinal BETWEEN 1 AND 64),
    rendered_summary text NOT NULL CHECK (
        rendered_summary = btrim(rendered_summary)
        AND rendered_summary <> ''
        AND octet_length(rendered_summary) <= 512
        AND position(E'\n' IN rendered_summary) = 0
        AND position(E'\r' IN rendered_summary) = 0
    ),
    PRIMARY KEY (batch_id, fingerprint),
    UNIQUE (batch_id, ordinal)
);

CREATE TABLE public.realtime_failures (
    id uuid PRIMARY KEY,
    consumer_key text NOT NULL CHECK (consumer_key <> ''),
    event_id uuid NOT NULL REFERENCES public.realtime_events(event_id) ON DELETE RESTRICT,
    failure_code text NOT NULL CHECK (failure_code <> ''),
    safe_detail text NOT NULL CHECK (safe_detail <> ''),
    failed_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    resolved_at timestamptz,
    replay_event_id uuid REFERENCES public.realtime_events(event_id) ON DELETE RESTRICT,
    UNIQUE (consumer_key, event_id),
    CHECK (resolved_at IS NULL OR resolved_at >= failed_at)
);

CREATE TABLE public.auction_forum_sync (
    auction_id uuid PRIMARY KEY REFERENCES public.auctions(id) ON DELETE CASCADE,
    status text NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'publishing', 'published', 'failed')),
    source_revision bigint NOT NULL DEFAULT 0 CHECK (source_revision >= 0),
    attempt_count integer NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    next_attempt_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    lease_expires_at timestamptz,
    thread_id text,
    thread_url text,
    title text,
    body text,
    last_error_code text,
    last_error_detail text,
    created_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    updated_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    CHECK (status <> 'published' OR (thread_id IS NOT NULL AND thread_url IS NOT NULL
        AND title IS NOT NULL AND body IS NOT NULL)),
    CHECK (lease_expires_at IS NULL OR lease_expires_at >= updated_at)
);

CREATE INDEX auction_forum_sync_claim_idx
    ON public.auction_forum_sync (next_attempt_at, auction_id)
    WHERE status IN ('pending', 'failed');

CREATE INDEX auction_forum_sync_lease_idx
    ON public.auction_forum_sync (lease_expires_at, auction_id)
    WHERE status = 'publishing';

CREATE TABLE public.forum_bidder_identities (
    id uuid PRIMARY KEY,
    world_id text NOT NULL REFERENCES public.worlds(id) ON DELETE RESTRICT,
    provider text NOT NULL CHECK (provider = 'ehentai'),
    external_account_id text NOT NULL
        CHECK (octet_length(external_account_id) BETWEEN 1 AND 256
            AND external_account_id = btrim(external_account_id)
            AND external_account_id !~ '[[:cntrl:]]'),
    display_name text NOT NULL
        CHECK (octet_length(display_name) BETWEEN 1 AND 256
            AND display_name = btrim(display_name)
            AND display_name !~ '[[:cntrl:]]'),
    user_id uuid NOT NULL REFERENCES public.users(id) ON DELETE RESTRICT,
    registration_state text NOT NULL DEFAULT 'forum_only'
        CHECK (registration_state IN ('forum_only', 'mm_bound')),
    status text NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'blocked')),
    created_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    updated_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    UNIQUE (world_id, provider, external_account_id)
);

CREATE UNIQUE INDEX forum_bidder_identities_casefold_unique
    ON public.forum_bidder_identities (world_id, provider, lower(external_account_id));

CREATE INDEX forum_bidder_identities_user_idx
    ON public.forum_bidder_identities (user_id, world_id);

CREATE TABLE public.auction_forum_reply_cursors (
    scope text PRIMARY KEY CHECK (scope IN ('main', 'isekai')),
    last_scan_at timestamptz NOT NULL,
    state_version bigint NOT NULL DEFAULT 1 CHECK (state_version > 0),
    updated_at timestamptz NOT NULL DEFAULT transaction_timestamp()
);

INSERT INTO public.auction_forum_reply_cursors (scope, last_scan_at)
VALUES ('main', to_timestamp(0)), ('isekai', to_timestamp(0));

CREATE TABLE public.auction_forum_bid_commands (
    command_id uuid PRIMARY KEY,
    auction_id uuid NOT NULL,
    world_id text NOT NULL REFERENCES public.worlds(id) ON DELETE RESTRICT,
    thread_id text NOT NULL CHECK (thread_id <> ''),
    post_id text NOT NULL CHECK (post_id <> ''),
    line_number integer NOT NULL CHECK (line_number > 0),
    lot_code text NOT NULL CHECK (lot_code <> ''),
    actor text NOT NULL CHECK (actor <> ''),
    command_kind text NOT NULL CHECK (command_kind IN ('bid', 'proxy', 'proxy_lower', 'notify')),
    amount bigint NOT NULL CHECK (amount >= 0),
    status text NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'applied', 'rejected')),
    attempt_count integer NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    last_error_code text,
    last_error_detail text,
    created_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    updated_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    UNIQUE (auction_id, thread_id, post_id, line_number, lot_code, command_kind),
    FOREIGN KEY (auction_id, world_id)
        REFERENCES public.auctions(id, world_id) ON DELETE CASCADE
        DEFERRABLE INITIALLY DEFERRED
);

CREATE INDEX auction_forum_bid_commands_retry_idx
    ON public.auction_forum_bid_commands (status, updated_at, command_id);

CREATE VIEW public.bid_proxies AS
SELECT
    authorization.lot_id,
    authorization.user_id,
    CASE authorization.mode
        WHEN 'fast' THEN 'instant'
        ELSE 'slow'
    END AS mode,
    authorization.maximum_amount,
    CASE authorization.status
        WHEN 'active' THEN 'active'
        WHEN 'final_fast' THEN 'active'
        WHEN 'exhausted' THEN 'exhausted'
        WHEN 'frozen' THEN 'frozen'
        ELSE 'cancelled'
    END AS status,
    authorization.state_version,
    authorization.created_at,
    authorization.updated_at,
    COUNT(outbid_use.use_number)::integer AS outbid_uses
FROM public.bid_proxy_authorizations AS authorization
LEFT JOIN public.bid_proxy_outbid_uses AS outbid_use
    ON outbid_use.authorization_id = authorization.authorization_id
GROUP BY
    authorization.lot_id,
    authorization.user_id,
    authorization.mode,
    authorization.maximum_amount,
    authorization.status,
    authorization.state_version,
    authorization.created_at,
    authorization.updated_at;
