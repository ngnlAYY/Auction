PRAGMA foreign_keys = ON;

CREATE TABLE worlds (
    id TEXT PRIMARY KEY,
    enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    availability TEXT NOT NULL DEFAULT 'available'
        CHECK (availability IN ('available', 'degraded', 'unavailable')),
    policy_revision INTEGER NOT NULL DEFAULT 1 CHECK (policy_revision > 0),
    updated_at TEXT NOT NULL
);

INSERT INTO worlds (id, enabled, availability, policy_revision, updated_at)
VALUES ('main', 1, 'available', 1, CURRENT_TIMESTAMP),
       ('isekai', 1, 'available', 1, CURRENT_TIMESTAMP);

CREATE TABLE users (
    id BLOB PRIMARY KEY,
    username TEXT NOT NULL UNIQUE CHECK (trim(username) = username AND username <> ''),
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'blacklisted')),
    public_key_hash BLOB NOT NULL CHECK (length(public_key_hash) = 32),
    game_player_id TEXT,
    key_status TEXT NOT NULL DEFAULT 'active' CHECK (key_status IN ('active', 'revoked')),
    state_version INTEGER NOT NULL DEFAULT 1 CHECK (state_version > 0),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE (public_key_hash),
    UNIQUE (game_player_id)
);

CREATE UNIQUE INDEX users_username_case_insensitive_unique ON users (lower(username));
CREATE UNIQUE INDEX users_game_player_id_case_insensitive_unique
    ON users (lower(game_player_id)) WHERE game_player_id IS NOT NULL;

CREATE TABLE user_blacklist (
    user_id BLOB PRIMARY KEY REFERENCES users(id) ON DELETE RESTRICT,
    public_reason TEXT NOT NULL CHECK (trim(public_reason) = public_reason AND public_reason <> ''),
    actor TEXT NOT NULL CHECK (trim(actor) = actor AND actor <> ''),
    created_at TEXT NOT NULL,
    revoked_at TEXT,
    CHECK (revoked_at IS NULL OR revoked_at >= created_at)
);

CREATE TABLE user_blacklist_history (
    id BLOB PRIMARY KEY,
    user_id BLOB NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    public_reason TEXT NOT NULL CHECK (trim(public_reason) = public_reason AND public_reason <> ''),
    actor TEXT NOT NULL CHECK (trim(actor) = actor AND actor <> ''),
    blacklisted_at TEXT NOT NULL
);

CREATE INDEX user_blacklist_history_user_time_idx
    ON user_blacklist_history (user_id, blacklisted_at DESC, id DESC);

CREATE TRIGGER user_blacklist_history_no_update
BEFORE UPDATE ON user_blacklist_history
BEGIN
    SELECT RAISE(ABORT, 'user_blacklist_history is append-only');
END;

CREATE TRIGGER user_blacklist_history_no_delete
BEFORE DELETE ON user_blacklist_history
BEGIN
    SELECT RAISE(ABORT, 'user_blacklist_history is append-only');
END;

CREATE TABLE user_renames (
    id BLOB PRIMARY KEY,
    user_id BLOB NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    previous_username TEXT NOT NULL CHECK (trim(previous_username) = previous_username AND previous_username <> ''),
    next_username TEXT NOT NULL CHECK (trim(next_username) = next_username AND next_username <> ''),
    actor TEXT NOT NULL CHECK (trim(actor) = actor AND actor <> ''),
    created_at TEXT NOT NULL,
    reverted_at TEXT,
    UNIQUE (user_id, next_username, created_at),
    CHECK (previous_username <> next_username),
    CHECK (reverted_at IS NULL OR reverted_at >= created_at)
);

CREATE TABLE public_sessions (
    token_hash BLOB PRIMARY KEY CHECK (length(token_hash) = 32),
    user_id BLOB NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at TEXT NOT NULL,
    last_activity_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    absolute_expires_at TEXT NOT NULL,
    revoked_at TEXT,
    CHECK (last_activity_at >= created_at),
    CHECK (expires_at > created_at),
    CHECK (absolute_expires_at >= expires_at)
);

CREATE INDEX public_sessions_user_active_idx
    ON public_sessions (user_id, expires_at DESC) WHERE revoked_at IS NULL;

CREATE TABLE admin_sessions (
    token_hash BLOB PRIMARY KEY CHECK (length(token_hash) = 32),
    actor TEXT NOT NULL CHECK (trim(actor) = actor AND actor <> ''),
    csrf_hash BLOB NOT NULL CHECK (length(csrf_hash) = 32),
    created_at TEXT NOT NULL,
    last_activity_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    absolute_expires_at TEXT NOT NULL,
    revoked_at TEXT,
    CHECK (last_activity_at >= created_at),
    CHECK (expires_at > created_at),
    CHECK (absolute_expires_at >= expires_at)
);

CREATE UNIQUE INDEX admin_sessions_actor_active_idx
    ON admin_sessions (actor) WHERE revoked_at IS NULL;

CREATE TABLE rules_documents (
    locale TEXT PRIMARY KEY CHECK (trim(locale) = locale AND locale <> ''),
    revision INTEGER NOT NULL CHECK (revision > 0),
    title TEXT NOT NULL CHECK (trim(title) = title AND title <> ''),
    markdown TEXT NOT NULL,
    published_by TEXT NOT NULL CHECK (trim(published_by) = published_by AND published_by <> ''),
    published_at TEXT NOT NULL
);

CREATE TABLE service_heartbeats (
    service_name TEXT NOT NULL
        CHECK (service_name IN ('auction-public', 'auction-admin', 'auction-worker')),
    process_role TEXT NOT NULL CHECK (process_role IN ('public', 'admin', 'worker')),
    instance_id TEXT NOT NULL CHECK (trim(instance_id) = instance_id AND instance_id <> ''),
    version TEXT NOT NULL CHECK (trim(version) = version AND version <> ''),
    started_at TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('starting', 'ready', 'degraded', 'stopping')),
    readiness INTEGER NOT NULL CHECK (readiness IN (0, 1)),
    audit_failure_count INTEGER NOT NULL CHECK (audit_failure_count >= 0),
    observation_sequence INTEGER NOT NULL CHECK (observation_sequence > 0),
    observed_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    details TEXT NOT NULL DEFAULT '{}',
    tracing_status TEXT NOT NULL DEFAULT 'disabled'
        CHECK (tracing_status IN ('disabled', 'ok', 'degraded')),
    tracing_dropped_span_count INTEGER NOT NULL DEFAULT 0 CHECK (tracing_dropped_span_count >= 0),
    tracing_export_failure_count INTEGER NOT NULL DEFAULT 0 CHECK (tracing_export_failure_count >= 0),
    tracing_last_success_at TEXT,
    tracing_last_failure_at TEXT,
    tracing_last_error_code TEXT NOT NULL DEFAULT '',
    PRIMARY KEY (service_name, instance_id),
    CHECK (
        (service_name = 'auction-public' AND process_role = 'public')
        OR (service_name = 'auction-admin' AND process_role = 'admin')
        OR (service_name = 'auction-worker' AND process_role = 'worker')
    ),
    CHECK (observed_at >= started_at AND expires_at > observed_at)
);

CREATE TABLE equipment (
    id BLOB PRIMARY KEY,
    world_id TEXT NOT NULL REFERENCES worlds(id) ON DELETE RESTRICT,
    external_equipment_id TEXT NOT NULL CHECK (external_equipment_id <> ''),
    name TEXT NOT NULL CHECK (name <> ''),
    category TEXT NOT NULL CHECK (category <> ''),
    attributes TEXT NOT NULL DEFAULT '{}',
    collected_at TEXT NOT NULL,
    UNIQUE (world_id, external_equipment_id)
);

CREATE TABLE auctions (
    id BLOB PRIMARY KEY,
    world_id TEXT NOT NULL REFERENCES worlds(id) ON DELETE RESTRICT,
    sequence INTEGER NOT NULL CHECK (sequence > 0),
    title TEXT NOT NULL CHECK (trim(title) = title AND title <> ''),
    status TEXT NOT NULL CHECK (status IN ('draft', 'preview', 'live', 'final', 'settling', 'settled', 'cancelled')),
    default_start_price INTEGER NOT NULL DEFAULT 50000 CHECK (default_start_price > 0),
    soft_close_window_seconds INTEGER NOT NULL DEFAULT 900 CHECK (soft_close_window_seconds >= 0),
    soft_close_extension_seconds INTEGER NOT NULL DEFAULT 900 CHECK (soft_close_extension_seconds > 0),
    cancel_cutoff_seconds INTEGER NOT NULL DEFAULT 86400 CHECK (cancel_cutoff_seconds >= 0),
    proxy_slow_interval_seconds INTEGER NOT NULL DEFAULT 600 CHECK (proxy_slow_interval_seconds > 0),
    policy_snapshot TEXT NOT NULL,
    preview_scheduled_at TEXT NOT NULL,
    scheduled_open_at TEXT NOT NULL,
    scheduled_end_at TEXT,
    effective_end_at TEXT,
    settled_at TEXT,
    state_version INTEGER NOT NULL DEFAULT 1 CHECK (state_version > 0),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE (world_id, sequence),
    UNIQUE (id, world_id),
    UNIQUE (id, world_id, status),
    CHECK (effective_end_at IS NULL OR scheduled_end_at IS NOT NULL),
    CHECK (effective_end_at IS NULL OR effective_end_at >= scheduled_end_at),
    CHECK (preview_scheduled_at <= scheduled_open_at)
);

CREATE UNIQUE INDEX auctions_one_active_world_idx
    ON auctions (world_id) WHERE status IN ('preview', 'live', 'final', 'settling');
CREATE UNIQUE INDEX auctions_one_draft_world_idx
    ON auctions (world_id) WHERE status = 'draft';

CREATE TABLE auction_preview_items (
    auction_id BLOB NOT NULL REFERENCES auctions(id) ON DELETE CASCADE,
    equipment_id BLOB NOT NULL REFERENCES equipment(id) ON DELETE RESTRICT,
    display_order INTEGER NOT NULL CHECK (display_order >= 0),
    start_price INTEGER NOT NULL CHECK (start_price > 0),
    source_revision INTEGER NOT NULL CHECK (source_revision > 0),
    collected_at TEXT NOT NULL,
    PRIMARY KEY (auction_id, equipment_id),
    UNIQUE (auction_id, display_order)
);

CREATE TABLE auction_submissions (
    id BLOB PRIMARY KEY,
    source_command_id BLOB NOT NULL UNIQUE REFERENCES auction_commands(command_id) ON DELETE RESTRICT,
    auction_id BLOB NOT NULL REFERENCES auctions(id) ON DELETE RESTRICT,
    source_kind TEXT NOT NULL DEFAULT 'mail' CHECK (source_kind IN ('mail', 'admin')),
    mailbox_key TEXT,
    external_mail_id TEXT,
    source_reference TEXT,
    source_fingerprint BLOB,
    seller TEXT NOT NULL CHECK (trim(seller) = seller AND seller <> ''),
    received_at TEXT NOT NULL,
    created_at TEXT NOT NULL,
    CHECK (
        (source_kind = 'mail' AND mailbox_key IS NOT NULL AND external_mail_id IS NOT NULL
            AND source_reference IS NULL AND source_fingerprint IS NULL)
        OR (source_kind = 'admin' AND mailbox_key IS NULL AND external_mail_id IS NULL
            AND source_reference IS NOT NULL AND source_fingerprint IS NOT NULL
            AND length(source_fingerprint) = 32)
    )
);

CREATE UNIQUE INDEX auction_submissions_mail_identity_idx
    ON auction_submissions (mailbox_key, external_mail_id) WHERE source_kind = 'mail';
CREATE UNIQUE INDEX auction_submissions_admin_fingerprint_idx
    ON auction_submissions (source_fingerprint) WHERE source_kind = 'admin';

CREATE TABLE lots (
    id BLOB PRIMARY KEY,
    auction_id BLOB NOT NULL REFERENCES auctions(id) ON DELETE CASCADE,
    equipment_id BLOB NOT NULL REFERENCES equipment(id) ON DELETE RESTRICT,
    lot_number INTEGER NOT NULL CHECK (lot_number > 0),
    status TEXT NOT NULL CHECK (status IN ('pending', 'live', 'sold', 'unsold', 'withdrawn')),
    start_price INTEGER NOT NULL CHECK (start_price > 0),
    current_price INTEGER CHECK (current_price IS NULL OR current_price >= start_price),
    leader_user_id BLOB REFERENCES users(id) ON DELETE RESTRICT,
    leader_since TEXT,
    leader_proxy_last INTEGER NOT NULL DEFAULT 0 CHECK (leader_proxy_last IN (0, 1)),
    effective_end_at TEXT,
    submission_id BLOB REFERENCES auction_submissions(id) ON DELETE RESTRICT,
    slow_authorization_id BLOB,
    slow_due_at TEXT,
    slow_generation INTEGER NOT NULL DEFAULT 0 CHECK (slow_generation >= 0),
    last_proxy_sequence INTEGER NOT NULL DEFAULT 0 CHECK (last_proxy_sequence >= 0),
    state_version INTEGER NOT NULL DEFAULT 1 CHECK (state_version > 0),
    material INTEGER NOT NULL DEFAULT 0 CHECK (material IN (0, 1)),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE (auction_id, lot_number),
    UNIQUE (auction_id, equipment_id),
    UNIQUE (auction_id, id),
    CHECK (status <> 'pending' OR submission_id IS NOT NULL),
    CHECK (leader_since IS NULL OR leader_user_id IS NOT NULL),
    CHECK ((slow_authorization_id IS NULL AND slow_due_at IS NULL)
        OR (slow_authorization_id IS NOT NULL AND slow_due_at IS NOT NULL AND slow_generation > 0))
);

CREATE INDEX lots_auction_status_idx ON lots (auction_id, status, lot_number);
CREATE INDEX lots_due_slow_idx ON lots (slow_due_at, id)
    WHERE status = 'live' AND slow_due_at IS NOT NULL;

CREATE TABLE auction_commands (
    command_id BLOB PRIMARY KEY,
    command_kind TEXT NOT NULL CHECK (command_kind <> ''),
    aggregate_id BLOB NOT NULL,
    request_hash BLOB NOT NULL CHECK (length(request_hash) = 32),
    result_code TEXT NOT NULL CHECK (result_code <> ''),
    result_payload TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL
);

CREATE TABLE bids (
    id BLOB PRIMARY KEY,
    command_id BLOB NOT NULL REFERENCES auction_commands(command_id) ON DELETE RESTRICT,
    fact_index INTEGER NOT NULL CHECK (fact_index > 0),
    lot_id BLOB NOT NULL REFERENCES lots(id) ON DELETE RESTRICT,
    bidder_user_id BLOB NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    amount INTEGER NOT NULL CHECK (amount > 0),
    mechanism TEXT NOT NULL CHECK (mechanism IN ('ordinary', 'proxy')),
    effective_end_at TEXT NOT NULL,
    accepted_state_version INTEGER NOT NULL CHECK (accepted_state_version > 0),
    created_at TEXT NOT NULL,
    UNIQUE (command_id, fact_index),
    UNIQUE (lot_id, accepted_state_version),
    UNIQUE (id, command_id, fact_index, accepted_state_version)
);

CREATE INDEX bids_lot_order_idx ON bids (lot_id, accepted_state_version, fact_index, id);

CREATE TRIGGER bids_contiguous_fact_index
BEFORE INSERT ON bids
BEGIN
    SELECT RAISE(ABORT, 'bid fact sequence must be contiguous')
    WHERE NEW.fact_index <> COALESCE((SELECT MAX(fact_index) + 1 FROM bids WHERE command_id = NEW.command_id), 1);
END;

CREATE TRIGGER bids_no_update
BEFORE UPDATE ON bids
BEGIN
    SELECT RAISE(ABORT, 'accepted bid facts are append-only');
END;

CREATE TRIGGER bids_no_delete
BEFORE DELETE ON bids
BEGIN
    SELECT RAISE(ABORT, 'accepted bid facts are append-only');
END;

CREATE TABLE bid_proxy_authorizations (
    authorization_id BLOB PRIMARY KEY,
    lot_id BLOB NOT NULL REFERENCES lots(id) ON DELETE RESTRICT,
    user_id BLOB NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    authorization_sequence INTEGER NOT NULL CHECK (authorization_sequence > 0),
    usage_number INTEGER NOT NULL CHECK (usage_number > 0),
    maximum_amount INTEGER NOT NULL CHECK (maximum_amount > 0),
    mode TEXT NOT NULL CHECK (mode IN ('fast', 'slow')),
    status TEXT NOT NULL CHECK (status IN ('active', 'final_fast', 'exhausted', 'frozen', 'closed')),
    terminal_reason TEXT,
    state_version INTEGER NOT NULL CHECK (state_version > 0),
    creation_command_id BLOB NOT NULL UNIQUE REFERENCES auction_commands(command_id) ON DELETE RESTRICT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE (authorization_id, lot_id),
    UNIQUE (lot_id, authorization_sequence),
    UNIQUE (lot_id, user_id, usage_number)
);

CREATE UNIQUE INDEX bid_proxy_authorizations_one_active_lot_idx
    ON bid_proxy_authorizations (lot_id) WHERE status IN ('active', 'final_fast');

CREATE TABLE bid_proxy_authorization_transitions (
    authorization_id BLOB NOT NULL REFERENCES bid_proxy_authorizations(authorization_id) ON DELETE RESTRICT,
    transition_number INTEGER NOT NULL CHECK (transition_number > 0),
    transition_kind TEXT NOT NULL,
    origin_command_id BLOB NOT NULL REFERENCES auction_commands(command_id) ON DELETE RESTRICT,
    old_maximum_amount INTEGER,
    new_maximum_amount INTEGER NOT NULL CHECK (new_maximum_amount > 0),
    old_mode TEXT,
    new_mode TEXT NOT NULL CHECK (new_mode IN ('fast', 'slow')),
    old_status TEXT,
    new_status TEXT NOT NULL,
    reason TEXT,
    resulting_state_version INTEGER NOT NULL CHECK (resulting_state_version > 0),
    created_at TEXT NOT NULL,
    PRIMARY KEY (authorization_id, transition_number),
    UNIQUE (authorization_id, resulting_state_version)
);

CREATE TRIGGER bid_proxy_authorization_transitions_contiguous
BEFORE INSERT ON bid_proxy_authorization_transitions
BEGIN
    SELECT RAISE(ABORT, 'proxy authorization transition sequence must be contiguous')
    WHERE NEW.transition_number <> COALESCE((SELECT MAX(transition_number) + 1
        FROM bid_proxy_authorization_transitions
        WHERE authorization_id = NEW.authorization_id), 1);
END;

CREATE TRIGGER bid_proxy_authorization_transitions_no_update
BEFORE UPDATE ON bid_proxy_authorization_transitions
BEGIN
    SELECT RAISE(ABORT, 'proxy authorization transitions are append-only');
END;

CREATE TRIGGER bid_proxy_authorization_transitions_no_delete
BEFORE DELETE ON bid_proxy_authorization_transitions
BEGIN
    SELECT RAISE(ABORT, 'proxy authorization transitions are append-only');
END;

CREATE TABLE bid_proxy_attempts (
    authorization_id BLOB NOT NULL REFERENCES bid_proxy_authorizations(authorization_id) ON DELETE RESTRICT,
    attempt_number INTEGER NOT NULL CHECK (attempt_number > 0),
    maximum_amount INTEGER NOT NULL CHECK (maximum_amount > 0),
    mode TEXT NOT NULL CHECK (mode IN ('fast', 'slow')),
    command_id BLOB NOT NULL UNIQUE REFERENCES auction_commands(command_id) ON DELETE RESTRICT,
    created_at TEXT NOT NULL,
    PRIMARY KEY (authorization_id, attempt_number)
);

CREATE TABLE bid_proxy_outbid_uses (
    authorization_id BLOB NOT NULL REFERENCES bid_proxy_authorizations(authorization_id) ON DELETE RESTRICT,
    use_number INTEGER NOT NULL CHECK (use_number > 0),
    command_id BLOB NOT NULL UNIQUE REFERENCES auction_commands(command_id) ON DELETE RESTRICT,
    created_at TEXT NOT NULL,
    PRIMARY KEY (authorization_id, use_number)
);

CREATE VIEW bid_proxies AS
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
    COUNT(outbid_use.use_number) AS outbid_uses
FROM bid_proxy_authorizations AS authorization
LEFT JOIN bid_proxy_outbid_uses AS outbid_use
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

CREATE TABLE audit_records (
    id BLOB PRIMARY KEY,
    occurred_at TEXT NOT NULL,
    actor_type TEXT NOT NULL CHECK (actor_type IN ('public_user', 'admin', 'worker', 'system')),
    actor_id TEXT NOT NULL CHECK (actor_id <> ''),
    action TEXT NOT NULL CHECK (action <> ''),
    entity_type TEXT NOT NULL CHECK (entity_type <> ''),
    entity_id TEXT NOT NULL CHECK (entity_id <> ''),
    result TEXT NOT NULL CHECK (result IN ('success', 'rejected', 'failure')),
    request_id TEXT,
    source TEXT NOT NULL CHECK (source IN ('public', 'admin', 'worker', 'system')),
    metadata TEXT NOT NULL DEFAULT '{}'
);

CREATE INDEX audit_records_entity_idx ON audit_records (entity_type, entity_id, occurred_at DESC);

CREATE TRIGGER audit_records_no_update
BEFORE UPDATE ON audit_records
BEGIN
    SELECT RAISE(ABORT, 'audit_records is append-only');
END;

CREATE TRIGGER audit_records_no_delete
BEFORE DELETE ON audit_records
BEGIN
    SELECT RAISE(ABORT, 'audit_records is append-only');
END;

CREATE TABLE realtime_streams (
    stream_key TEXT PRIMARY KEY CHECK (stream_key <> ''),
    next_sequence INTEGER NOT NULL DEFAULT 1 CHECK (next_sequence > 0),
    updated_at TEXT NOT NULL
);

CREATE TABLE realtime_events (
    event_id BLOB PRIMARY KEY,
    stream_key TEXT NOT NULL REFERENCES realtime_streams(stream_key) ON DELETE RESTRICT,
    stream_sequence INTEGER NOT NULL CHECK (stream_sequence > 0),
    kind TEXT NOT NULL CHECK (kind <> ''),
    schema_version INTEGER NOT NULL CHECK (schema_version > 0),
    public_payload TEXT NOT NULL DEFAULT '{}',
    state_version INTEGER,
    origin_command_id BLOB,
    origin_bid_id BLOB,
    origin_fact_index INTEGER,
    replay_of_event_id BLOB REFERENCES realtime_events(event_id) ON DELETE RESTRICT,
    created_at TEXT NOT NULL,
    UNIQUE (stream_key, stream_sequence),
    UNIQUE (origin_bid_id, origin_fact_index)
);

CREATE INDEX realtime_events_stream_idx ON realtime_events (stream_key, stream_sequence);

CREATE TABLE realtime_consumer_offsets (
    consumer_key TEXT NOT NULL CHECK (consumer_key <> ''),
    stream_key TEXT NOT NULL REFERENCES realtime_streams(stream_key) ON DELETE CASCADE,
    last_sequence INTEGER NOT NULL DEFAULT 0 CHECK (last_sequence >= 0),
    updated_at TEXT NOT NULL,
    PRIMARY KEY (consumer_key, stream_key)
);

CREATE TABLE realtime_failures (
    id BLOB PRIMARY KEY,
    consumer_key TEXT NOT NULL CHECK (consumer_key <> ''),
    event_id BLOB NOT NULL REFERENCES realtime_events(event_id) ON DELETE RESTRICT,
    failure_code TEXT NOT NULL CHECK (failure_code <> ''),
    safe_detail TEXT NOT NULL CHECK (safe_detail <> ''),
    failed_at TEXT NOT NULL,
    resolved_at TEXT,
    replay_event_id BLOB REFERENCES realtime_events(event_id) ON DELETE RESTRICT,
    UNIQUE (consumer_key, event_id)
);

CREATE TABLE scheduled_rounds (
    id BLOB PRIMARY KEY,
    world_id TEXT NOT NULL REFERENCES worlds(id) ON DELETE RESTRICT,
    local_week TEXT NOT NULL,
    auction_id BLOB UNIQUE REFERENCES auctions(id) ON DELETE RESTRICT,
    status TEXT NOT NULL CHECK (status IN ('active', 'missed')),
    preview_due_at TEXT NOT NULL,
    open_due_at TEXT NOT NULL,
    scheduled_end_at TEXT NOT NULL,
    create_command_id BLOB NOT NULL UNIQUE REFERENCES auction_commands(command_id) ON DELETE RESTRICT,
    preview_command_id BLOB UNIQUE REFERENCES auction_commands(command_id) ON DELETE RESTRICT,
    open_command_id BLOB UNIQUE REFERENCES auction_commands(command_id) ON DELETE RESTRICT,
    preview_completed_at TEXT,
    open_completed_at TEXT,
    state_version INTEGER NOT NULL DEFAULT 1 CHECK (state_version > 0),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE (world_id, local_week),
    CHECK (preview_due_at < open_due_at AND open_due_at < scheduled_end_at)
);

CREATE TABLE auction_schema_identity (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    application TEXT NOT NULL,
    baseline_version INTEGER NOT NULL CHECK (baseline_version > 0),
    installed_at TEXT NOT NULL
);

INSERT INTO auction_schema_identity (singleton, application, baseline_version, installed_at)
VALUES (1, 'auction', 1, CURRENT_TIMESTAMP);
