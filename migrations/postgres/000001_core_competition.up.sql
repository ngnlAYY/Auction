CREATE TABLE public.worlds (
    id text PRIMARY KEY,
    enabled boolean NOT NULL DEFAULT true,
    availability text NOT NULL DEFAULT 'available'
        CHECK (availability IN ('available', 'degraded', 'unavailable')),
    policy_revision bigint NOT NULL DEFAULT 1 CHECK (policy_revision > 0),
    updated_at timestamptz NOT NULL DEFAULT transaction_timestamp()
);

INSERT INTO public.worlds (id, enabled, availability, policy_revision)
VALUES
    ('main', true, 'available', 1),
    ('isekai', true, 'available', 1);

CREATE TABLE public.users (
    id uuid PRIMARY KEY,
    username text NOT NULL UNIQUE CHECK (username = btrim(username) AND username <> ''),
    status text NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'blacklisted')),
    public_key_hash bytea NOT NULL UNIQUE CHECK (octet_length(public_key_hash) = 32),
    game_player_id text UNIQUE,
    created_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    updated_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    CHECK (game_player_id IS NULL OR game_player_id = btrim(game_player_id))
);

CREATE TABLE public.equipment (
    id uuid PRIMARY KEY,
    world_id text NOT NULL REFERENCES public.worlds(id) ON DELETE RESTRICT,
    external_equipment_id text NOT NULL CHECK (external_equipment_id <> ''),
    name text NOT NULL CHECK (name <> ''),
    category text NOT NULL CHECK (category <> ''),
    attributes jsonb NOT NULL DEFAULT '{}'::jsonb
        CHECK (jsonb_typeof(attributes) = 'object'),
    collected_at timestamptz NOT NULL,
    UNIQUE (world_id, external_equipment_id)
);

CREATE TABLE public.auctions (
    id uuid PRIMARY KEY,
    world_id text NOT NULL REFERENCES public.worlds(id) ON DELETE RESTRICT,
    sequence bigint NOT NULL CHECK (sequence > 0),
    title text NOT NULL CHECK (title = btrim(title) AND title <> ''),
    status text NOT NULL CHECK (
        status IN ('draft', 'preview', 'live', 'final', 'settling', 'settled', 'cancelled')
    ),
    default_start_price bigint NOT NULL DEFAULT 50000 CHECK (default_start_price > 0),
    soft_close_window_seconds bigint NOT NULL DEFAULT 900 CHECK (soft_close_window_seconds >= 0),
    soft_close_extension_seconds bigint NOT NULL DEFAULT 900 CHECK (soft_close_extension_seconds > 0),
    cancel_cutoff_seconds bigint NOT NULL DEFAULT 86400 CHECK (cancel_cutoff_seconds >= 0),
    proxy_slow_interval_seconds bigint NOT NULL DEFAULT 600 CHECK (proxy_slow_interval_seconds > 0),
    scheduled_end_at timestamptz,
    effective_end_at timestamptz,
    state_version bigint NOT NULL DEFAULT 1 CHECK (state_version > 0),
    created_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    updated_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    UNIQUE (world_id, sequence),
    CHECK (effective_end_at IS NULL OR scheduled_end_at IS NOT NULL),
    CHECK (effective_end_at IS NULL OR effective_end_at >= scheduled_end_at)
);

CREATE UNIQUE INDEX auctions_one_active_world_idx
    ON public.auctions (world_id)
    WHERE status IN ('preview', 'live', 'final', 'settling');

CREATE TABLE public.lots (
    id uuid PRIMARY KEY,
    auction_id uuid NOT NULL REFERENCES public.auctions(id) ON DELETE CASCADE,
    equipment_id uuid NOT NULL REFERENCES public.equipment(id) ON DELETE RESTRICT,
    lot_number integer NOT NULL CHECK (lot_number > 0),
    status text NOT NULL CHECK (status IN ('pending', 'live', 'sold', 'unsold', 'withdrawn')),
    start_price bigint NOT NULL CHECK (start_price > 0),
    current_price bigint CHECK (current_price IS NULL OR current_price >= start_price),
    leader_user_id uuid REFERENCES public.users(id) ON DELETE RESTRICT,
    leader_proxy_last boolean NOT NULL DEFAULT false,
    effective_end_at timestamptz,
    slow_authorization_id uuid,
    slow_due_at timestamptz,
    slow_generation bigint NOT NULL DEFAULT 0 CHECK (slow_generation >= 0),
    last_proxy_sequence bigint NOT NULL DEFAULT 0 CHECK (last_proxy_sequence >= 0),
    state_version bigint NOT NULL DEFAULT 1 CHECK (state_version > 0),
    material boolean NOT NULL DEFAULT false,
    created_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    updated_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    UNIQUE (auction_id, lot_number),
    UNIQUE (auction_id, equipment_id),
    CHECK (
        (slow_authorization_id IS NULL AND slow_due_at IS NULL)
        OR (slow_authorization_id IS NOT NULL AND slow_due_at IS NOT NULL AND slow_generation > 0)
    )
);

CREATE INDEX lots_auction_status_idx
    ON public.lots (auction_id, status, lot_number);

CREATE INDEX lots_due_slow_idx
    ON public.lots (slow_due_at, id)
    WHERE status = 'live' AND slow_due_at IS NOT NULL;

CREATE TABLE public.auction_commands (
    command_id uuid PRIMARY KEY,
    command_kind text NOT NULL CHECK (command_kind <> ''),
    aggregate_id uuid NOT NULL,
    request_hash bytea NOT NULL CHECK (octet_length(request_hash) = 32),
    result_code text NOT NULL CHECK (result_code <> ''),
    result_payload jsonb NOT NULL DEFAULT '{}'::jsonb
        CHECK (jsonb_typeof(result_payload) = 'object'),
    created_at timestamptz NOT NULL DEFAULT transaction_timestamp()
);

CREATE TABLE public.bids (
    id uuid PRIMARY KEY,
    command_id uuid NOT NULL
        REFERENCES public.auction_commands(command_id) ON DELETE RESTRICT,
    fact_index integer NOT NULL CHECK (fact_index > 0),
    lot_id uuid NOT NULL REFERENCES public.lots(id) ON DELETE RESTRICT,
    bidder_user_id uuid NOT NULL REFERENCES public.users(id) ON DELETE RESTRICT,
    amount bigint NOT NULL CHECK (amount > 0),
    mechanism text NOT NULL CHECK (mechanism IN ('ordinary', 'proxy')),
    effective_end_at timestamptz NOT NULL,
    accepted_state_version bigint NOT NULL CHECK (accepted_state_version > 0),
    created_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    UNIQUE (command_id, fact_index),
    UNIQUE (lot_id, accepted_state_version),
    UNIQUE (id, command_id, fact_index, accepted_state_version)
);

CREATE INDEX bids_lot_order_idx
    ON public.bids (lot_id, accepted_state_version, fact_index, id);

CREATE FUNCTION public.enforce_bid_fact_sequence()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    expected_fact_index integer;
BEGIN
    PERFORM 1
    FROM public.auction_commands
    WHERE command_id = NEW.command_id
    FOR UPDATE;

    IF NOT FOUND THEN
        RAISE EXCEPTION 'auction command does not exist for bid fact'
            USING ERRCODE = '23503';
    END IF;

    SELECT COALESCE(max(fact_index), 0) + 1
    INTO expected_fact_index
    FROM public.bids
    WHERE command_id = NEW.command_id;

    IF NEW.fact_index <> expected_fact_index THEN
        RAISE EXCEPTION 'bid fact sequence must be contiguous'
            USING ERRCODE = '23514';
    END IF;

    RETURN NEW;
END;
$$;

CREATE TRIGGER bids_contiguous_fact_index
BEFORE INSERT ON public.bids
FOR EACH ROW EXECUTE FUNCTION public.enforce_bid_fact_sequence();

CREATE FUNCTION public.reject_bid_fact_mutation()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    RAISE EXCEPTION 'accepted bid facts are append-only'
        USING ERRCODE = '55000';
END;
$$;

CREATE TRIGGER bids_append_only
BEFORE UPDATE OR DELETE ON public.bids
FOR EACH ROW EXECUTE FUNCTION public.reject_bid_fact_mutation();

CREATE TABLE public.bid_proxy_authorizations (
    authorization_id uuid PRIMARY KEY,
    lot_id uuid NOT NULL REFERENCES public.lots(id) ON DELETE RESTRICT,
    user_id uuid NOT NULL REFERENCES public.users(id) ON DELETE RESTRICT,
    authorization_sequence bigint NOT NULL CHECK (authorization_sequence > 0),
    usage_number bigint NOT NULL CHECK (usage_number > 0),
    maximum_amount bigint NOT NULL CHECK (maximum_amount > 0),
    mode text NOT NULL CHECK (mode IN ('fast', 'slow')),
    status text NOT NULL CHECK (status IN ('active', 'final_fast', 'exhausted', 'frozen', 'closed')),
    terminal_reason text CHECK (
        terminal_reason IS NULL
        OR terminal_reason IN (
            'strictly_defeated',
            'ranked_below',
            'eligibility_lost',
            'competition_closed'
        )
    ),
    state_version bigint NOT NULL CHECK (state_version > 0),
    creation_command_id uuid NOT NULL UNIQUE
        REFERENCES public.auction_commands(command_id)
        DEFERRABLE INITIALLY DEFERRED,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    UNIQUE (authorization_id, lot_id),
    UNIQUE (lot_id, authorization_sequence),
    UNIQUE (lot_id, user_id, usage_number),
    CHECK (updated_at >= created_at),
    CHECK (
        (status IN ('active', 'final_fast') AND terminal_reason IS NULL)
        OR (status = 'exhausted' AND terminal_reason IN ('strictly_defeated', 'ranked_below'))
        OR (status = 'frozen' AND terminal_reason = 'eligibility_lost')
        OR (status = 'closed' AND terminal_reason = 'competition_closed')
    )
);

CREATE UNIQUE INDEX bid_proxy_authorizations_one_active_lot_idx
    ON public.bid_proxy_authorizations (lot_id)
    WHERE status IN ('active', 'final_fast');

CREATE INDEX bid_proxy_authorizations_lot_rank_idx
    ON public.bid_proxy_authorizations (
        lot_id,
        maximum_amount DESC,
        authorization_sequence,
        authorization_id
    )
    WHERE status IN ('active', 'final_fast');

ALTER TABLE public.lots
    ADD CONSTRAINT lots_slow_authorization_fk
    FOREIGN KEY (slow_authorization_id)
    REFERENCES public.bid_proxy_authorizations(authorization_id)
    ON DELETE RESTRICT
    DEFERRABLE INITIALLY DEFERRED;

CREATE TABLE public.bid_proxy_authorization_transitions (
    authorization_id uuid NOT NULL
        REFERENCES public.bid_proxy_authorizations(authorization_id) ON DELETE RESTRICT,
    transition_number bigint NOT NULL CHECK (transition_number > 0),
    transition_kind text NOT NULL CHECK (
        transition_kind IN ('created', 'lowered', 'final_fast', 'exhausted', 'frozen', 'closed')
    ),
    origin_command_id uuid NOT NULL
        REFERENCES public.auction_commands(command_id)
        DEFERRABLE INITIALLY DEFERRED,
    old_maximum_amount bigint CHECK (old_maximum_amount IS NULL OR old_maximum_amount > 0),
    new_maximum_amount bigint NOT NULL CHECK (new_maximum_amount > 0),
    old_mode text CHECK (old_mode IS NULL OR old_mode IN ('fast', 'slow')),
    new_mode text NOT NULL CHECK (new_mode IN ('fast', 'slow')),
    old_status text CHECK (
        old_status IS NULL OR old_status IN ('active', 'final_fast', 'exhausted', 'frozen', 'closed')
    ),
    new_status text NOT NULL CHECK (
        new_status IN ('active', 'final_fast', 'exhausted', 'frozen', 'closed')
    ),
    reason text CHECK (
        reason IS NULL
        OR reason IN (
            'strictly_defeated',
            'ranked_below',
            'eligibility_lost',
            'competition_closed'
        )
    ),
    resulting_state_version bigint NOT NULL CHECK (resulting_state_version > 0),
    created_at timestamptz NOT NULL,
    PRIMARY KEY (authorization_id, transition_number),
    UNIQUE (authorization_id, resulting_state_version),
    CHECK (
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
        OR transition_kind <> 'created'
    )
);

CREATE TABLE public.audit_records (
    id uuid PRIMARY KEY,
    occurred_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    actor_type text NOT NULL CHECK (actor_type IN ('public_user', 'admin', 'worker', 'system')),
    actor_id text NOT NULL CHECK (actor_id <> ''),
    action text NOT NULL CHECK (action <> ''),
    entity_type text NOT NULL CHECK (entity_type <> ''),
    entity_id text NOT NULL CHECK (entity_id <> ''),
    result text NOT NULL CHECK (result IN ('success', 'rejected', 'failure')),
    request_id text,
    source text NOT NULL CHECK (source IN ('public', 'admin', 'worker', 'system')),
    metadata jsonb NOT NULL DEFAULT '{}'::jsonb
        CHECK (jsonb_typeof(metadata) = 'object')
);

CREATE INDEX audit_records_entity_idx
    ON public.audit_records (entity_type, entity_id, occurred_at DESC);

CREATE FUNCTION public.reject_audit_mutation()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    RAISE EXCEPTION 'audit_records is append-only'
        USING ERRCODE = '55000';
END;
$$;

CREATE TRIGGER audit_records_append_only
BEFORE UPDATE OR DELETE ON public.audit_records
FOR EACH ROW EXECUTE FUNCTION public.reject_audit_mutation();

CREATE TABLE public.realtime_streams (
    stream_key text PRIMARY KEY CHECK (stream_key <> ''),
    next_sequence bigint NOT NULL DEFAULT 1 CHECK (next_sequence > 0),
    updated_at timestamptz NOT NULL DEFAULT transaction_timestamp()
);

CREATE TABLE public.realtime_events (
    event_id uuid PRIMARY KEY,
    stream_key text NOT NULL REFERENCES public.realtime_streams(stream_key) ON DELETE RESTRICT,
    stream_sequence bigint NOT NULL CHECK (stream_sequence > 0),
    kind text NOT NULL CHECK (kind <> ''),
    schema_version integer NOT NULL CHECK (schema_version > 0),
    public_payload jsonb NOT NULL CHECK (jsonb_typeof(public_payload) = 'object'),
    state_version bigint CHECK (state_version IS NULL OR state_version > 0),
    origin_command_id uuid,
    origin_bid_id uuid,
    origin_fact_index integer CHECK (origin_fact_index IS NULL OR origin_fact_index > 0),
    replay_of_event_id uuid REFERENCES public.realtime_events(event_id) ON DELETE RESTRICT,
    created_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    UNIQUE (stream_key, stream_sequence),
    UNIQUE (origin_bid_id, origin_fact_index)
        DEFERRABLE INITIALLY DEFERRED,
    CHECK (replay_of_event_id IS NULL OR replay_of_event_id <> event_id),
    CHECK (
        (
            kind = 'lot.bid_accepted'
            AND origin_command_id IS NOT NULL
            AND origin_bid_id IS NOT NULL
            AND origin_fact_index IS NOT NULL
            AND state_version IS NOT NULL
        )
        OR kind <> 'lot.bid_accepted'
    )
);

ALTER TABLE public.realtime_events
    ADD CONSTRAINT realtime_events_bid_fact_fk
    FOREIGN KEY (origin_bid_id, origin_command_id, origin_fact_index, state_version)
    REFERENCES public.bids (id, command_id, fact_index, accepted_state_version)
    ON DELETE RESTRICT
    DEFERRABLE INITIALLY DEFERRED;

CREATE INDEX realtime_events_stream_idx
    ON public.realtime_events (stream_key, stream_sequence);

CREATE TABLE public.realtime_consumer_offsets (
    consumer_key text NOT NULL CHECK (consumer_key <> ''),
    stream_key text NOT NULL REFERENCES public.realtime_streams(stream_key) ON DELETE CASCADE,
    last_sequence bigint NOT NULL DEFAULT 0 CHECK (last_sequence >= 0),
    updated_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    PRIMARY KEY (consumer_key, stream_key)
);

CREATE TABLE public.auction_schema_identity (
    singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    application text NOT NULL,
    baseline_version bigint NOT NULL CHECK (baseline_version > 0),
    installed_at timestamptz NOT NULL DEFAULT transaction_timestamp()
);

INSERT INTO public.auction_schema_identity (singleton, application, baseline_version)
VALUES (true, 'auction', 1);
