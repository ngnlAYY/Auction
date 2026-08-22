ALTER TABLE public.users
    ADD COLUMN key_status text NOT NULL DEFAULT 'active'
        CHECK (key_status IN ('active', 'revoked')),
    ADD COLUMN state_version bigint NOT NULL DEFAULT 1
        CHECK (state_version > 0);

CREATE UNIQUE INDEX users_username_case_insensitive_unique
    ON public.users (lower(username));

CREATE UNIQUE INDEX users_game_player_id_case_insensitive_unique
    ON public.users (lower(game_player_id))
    WHERE game_player_id IS NOT NULL;

CREATE TABLE public.user_blacklist (
    user_id uuid PRIMARY KEY REFERENCES public.users(id) ON DELETE RESTRICT,
    public_reason text NOT NULL
        CHECK (octet_length(public_reason) BETWEEN 1 AND 1024
            AND public_reason = btrim(public_reason)
            AND public_reason !~ '[[:cntrl:]]'),
    actor text NOT NULL
        CHECK (octet_length(actor) BETWEEN 1 AND 256
            AND actor = btrim(actor)
            AND actor !~ '[[:cntrl:]]'),
    created_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    revoked_at timestamptz,
    CHECK (revoked_at IS NULL OR revoked_at >= created_at)
);

CREATE TABLE public.user_renames (
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES public.users(id) ON DELETE RESTRICT,
    previous_username text NOT NULL
        CHECK (previous_username = btrim(previous_username) AND previous_username <> ''),
    next_username text NOT NULL
        CHECK (next_username = btrim(next_username) AND next_username <> ''),
    actor text NOT NULL CHECK (actor = btrim(actor) AND actor <> ''),
    created_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    reverted_at timestamptz,
    UNIQUE (user_id, next_username, created_at),
    CHECK (previous_username <> next_username),
    CHECK (reverted_at IS NULL OR reverted_at >= created_at)
);

CREATE TABLE public.public_sessions (
    token_hash bytea PRIMARY KEY CHECK (octet_length(token_hash) = 32),
    user_id uuid NOT NULL REFERENCES public.users(id) ON DELETE CASCADE,
    created_at timestamptz NOT NULL,
    last_activity_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL,
    absolute_expires_at timestamptz NOT NULL,
    revoked_at timestamptz,
    CHECK (last_activity_at >= created_at),
    CHECK (expires_at > created_at),
    CHECK (absolute_expires_at >= expires_at),
    CHECK (revoked_at IS NULL OR revoked_at >= created_at)
);

CREATE INDEX public_sessions_user_active_idx
    ON public.public_sessions (user_id, expires_at DESC)
    WHERE revoked_at IS NULL;

CREATE TABLE public.admin_sessions (
    token_hash bytea PRIMARY KEY CHECK (octet_length(token_hash) = 32),
    actor text NOT NULL CHECK (actor = btrim(actor) AND actor <> ''),
    csrf_hash bytea NOT NULL CHECK (octet_length(csrf_hash) = 32),
    created_at timestamptz NOT NULL,
    last_activity_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL,
    absolute_expires_at timestamptz NOT NULL,
    revoked_at timestamptz,
    CHECK (last_activity_at >= created_at),
    CHECK (expires_at > created_at),
    CHECK (absolute_expires_at >= expires_at),
    CHECK (revoked_at IS NULL OR revoked_at >= created_at)
);

CREATE UNIQUE INDEX admin_sessions_actor_active_idx
    ON public.admin_sessions (actor)
    WHERE revoked_at IS NULL;

CREATE TABLE public.rules_documents (
    locale text PRIMARY KEY CHECK (locale = btrim(locale) AND locale <> ''),
    revision bigint NOT NULL CHECK (revision > 0),
    title text NOT NULL CHECK (title = btrim(title) AND title <> ''),
    markdown text NOT NULL,
    published_by text NOT NULL CHECK (published_by = btrim(published_by) AND published_by <> ''),
    published_at timestamptz NOT NULL DEFAULT transaction_timestamp()
);

CREATE TABLE public.service_heartbeats (
    service_name text NOT NULL
        CHECK (service_name IN ('auction-public', 'auction-admin', 'auction-worker')),
    process_role text NOT NULL CHECK (process_role IN ('public', 'admin', 'worker')),
    instance_id text NOT NULL
        CHECK (instance_id = btrim(instance_id) AND instance_id <> ''),
    version text NOT NULL CHECK (version = btrim(version) AND version <> ''),
    started_at timestamptz NOT NULL,
    status text NOT NULL CHECK (status IN ('starting', 'ready', 'degraded', 'stopping')),
    readiness boolean NOT NULL,
    audit_failure_count bigint NOT NULL CHECK (audit_failure_count >= 0),
    observation_sequence bigint NOT NULL CHECK (observation_sequence > 0),
    observed_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL,
    details jsonb NOT NULL DEFAULT '{}'::jsonb
        CHECK (jsonb_typeof(details) = 'object'),
    PRIMARY KEY (service_name, instance_id),
    CHECK (
        (service_name = 'auction-public' AND process_role = 'public')
        OR (service_name = 'auction-admin' AND process_role = 'admin')
        OR (service_name = 'auction-worker' AND process_role = 'worker')
    ),
    CHECK (observed_at >= started_at),
    CHECK (expires_at > observed_at)
);

CREATE INDEX service_heartbeats_expiry_idx
    ON public.service_heartbeats (expires_at);

CREATE TABLE public.credential_keys (
    id uuid PRIMARY KEY,
    credential_kind text NOT NULL CHECK (credential_kind IN ('mail_cookie', 'external_api')),
    scope text NOT NULL CHECK (scope = btrim(scope) AND scope <> ''),
    label text NOT NULL CHECK (label = btrim(label) AND label <> ''),
    ciphertext bytea NOT NULL CHECK (octet_length(ciphertext) > 0),
    nonce bytea NOT NULL CHECK (octet_length(nonce) = 12),
    encryption_key_id text NOT NULL
        CHECK (encryption_key_id = btrim(encryption_key_id) AND encryption_key_id <> ''),
    fingerprint text NOT NULL
        CHECK (fingerprint = btrim(fingerprint) AND fingerprint <> ''),
    status text NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'disabled', 'invalid')),
    state_version bigint NOT NULL DEFAULT 1 CHECK (state_version > 0),
    created_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    rotated_at timestamptz,
    updated_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    UNIQUE (credential_kind, scope, fingerprint),
    CHECK (rotated_at IS NULL OR rotated_at >= created_at),
    CHECK (updated_at >= created_at)
);

CREATE UNIQUE INDEX credential_keys_one_active_label_idx
    ON public.credential_keys (credential_kind, scope, label)
    WHERE status = 'active';

CREATE INDEX credential_keys_active_scope_idx
    ON public.credential_keys (credential_kind, scope, created_at, id)
    WHERE status = 'active';

CREATE TABLE public.mail_commands (
    command_id uuid PRIMARY KEY,
    request_digest bytea NOT NULL CHECK (octet_length(request_digest) = 32),
    mail_id uuid NOT NULL REFERENCES public.inbound_mail(id) ON DELETE RESTRICT,
    result_payload jsonb NOT NULL CHECK (jsonb_typeof(result_payload) = 'object'),
    created_at timestamptz NOT NULL DEFAULT transaction_timestamp()
);

CREATE TABLE public.mail_poll_state (
    mailbox_key text PRIMARY KEY
        CHECK (mailbox_key = btrim(mailbox_key) AND mailbox_key <> ''),
    cursor_value text,
    last_poll_at timestamptz NOT NULL,
    next_poll_at timestamptz NOT NULL,
    consecutive_failures integer NOT NULL DEFAULT 0 CHECK (consecutive_failures >= 0),
    paused boolean NOT NULL DEFAULT false,
    state_version bigint NOT NULL CHECK (state_version > 0),
    updated_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    CHECK (
        cursor_value IS NULL
        OR (cursor_value = btrim(cursor_value) AND cursor_value <> '')
    ),
    CHECK (next_poll_at >= last_poll_at)
);

CREATE TABLE public.user_login_key_envelopes (
    user_id uuid PRIMARY KEY REFERENCES public.users(id) ON DELETE RESTRICT,
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

CREATE TABLE public.user_mail_commands (
    command_id uuid PRIMARY KEY,
    command_kind text NOT NULL CHECK (command_kind IN ('register', 'rebind')),
    request_hash bytea NOT NULL CHECK (octet_length(request_hash) = 32),
    user_id uuid REFERENCES public.users(id) ON DELETE RESTRICT,
    result_code text NOT NULL CHECK (result_code = btrim(result_code) AND result_code <> ''),
    result_metadata jsonb NOT NULL DEFAULT '{}'::jsonb
        CHECK (jsonb_typeof(result_metadata) = 'object'),
    created_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    CHECK (
        (command_kind = 'register' AND result_code IN (
            'registered', 'recovered', 'blacklisted', 'identity_conflict'
        ))
        OR (command_kind = 'rebind' AND result_code IN (
            'rebound', 'blacklisted', 'identity_conflict', 'unchanged', 'invalid_key'
        ))
    ),
    CHECK (
        user_id IS NOT NULL
        OR (command_kind = 'rebind' AND result_code = 'invalid_key')
        OR (command_kind = 'register' AND result_code = 'identity_conflict')
    )
);

CREATE INDEX user_mail_commands_user_idx
    ON public.user_mail_commands (user_id, created_at, command_id);

CREATE TABLE public.user_game_identity_history (
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES public.users(id) ON DELETE RESTRICT,
    source_command_id uuid NOT NULL UNIQUE
        REFERENCES public.user_mail_commands(command_id) ON DELETE RESTRICT
        DEFERRABLE INITIALLY DEFERRED,
    previous_username text NOT NULL
        CHECK (previous_username = btrim(previous_username) AND previous_username <> ''),
    next_username text NOT NULL
        CHECK (next_username = btrim(next_username) AND next_username <> ''),
    previous_game_player_id text,
    next_game_player_id text NOT NULL
        CHECK (next_game_player_id = btrim(next_game_player_id) AND next_game_player_id <> ''),
    created_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    CHECK (
        previous_game_player_id IS NULL
        OR (previous_game_player_id = btrim(previous_game_player_id)
            AND previous_game_player_id <> '')
    ),
    CHECK (
        previous_username <> next_username
        OR previous_game_player_id IS DISTINCT FROM next_game_player_id
    )
);

CREATE INDEX user_game_identity_history_user_idx
    ON public.user_game_identity_history (user_id, created_at, id);

CREATE TABLE public.user_blacklist_history (
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES public.users(id) ON DELETE RESTRICT,
    public_reason text NOT NULL
        CHECK (octet_length(public_reason) BETWEEN 1 AND 1024
            AND public_reason = btrim(public_reason)
            AND public_reason !~ '[[:cntrl:]]'),
    actor text NOT NULL
        CHECK (octet_length(actor) BETWEEN 1 AND 256
            AND actor = btrim(actor)
            AND actor !~ '[[:cntrl:]]'),
    blacklisted_at timestamptz NOT NULL
);

CREATE INDEX user_blacklist_history_user_time_idx
    ON public.user_blacklist_history (user_id, blacklisted_at DESC, id DESC);

CREATE FUNCTION public.reject_user_blacklist_history_mutation()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    RAISE EXCEPTION 'user_blacklist_history is append-only'
        USING ERRCODE = '55000';
END;
$$;

CREATE TRIGGER user_blacklist_history_append_only
BEFORE UPDATE OR DELETE ON public.user_blacklist_history
FOR EACH ROW EXECUTE FUNCTION public.reject_user_blacklist_history_mutation();

CREATE TABLE public.user_admin_commands (
    command_id uuid PRIMARY KEY,
    command_kind text NOT NULL CHECK (command_kind IN (
        'create', 'rename', 'rename_revert', 'set_game_player_id',
        'revoke_key', 'reissue_key', 'blacklist', 'unblacklist'
    )),
    request_digest bytea NOT NULL CHECK (octet_length(request_digest) = 32),
    user_id uuid NOT NULL REFERENCES public.users(id) ON DELETE RESTRICT,
    result_payload jsonb NOT NULL CHECK (jsonb_typeof(result_payload) = 'object'),
    response_ciphertext bytea,
    response_nonce bytea,
    response_encryption_key_id text,
    response_fingerprint text,
    created_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    CHECK (num_nonnulls(
        response_ciphertext,
        response_nonce,
        response_encryption_key_id,
        response_fingerprint
    ) IN (0, 4)),
    CHECK (
        response_ciphertext IS NULL
        OR (
            octet_length(response_ciphertext) > 0
            AND octet_length(response_nonce) = 12
            AND response_encryption_key_id = btrim(response_encryption_key_id)
            AND response_encryption_key_id <> ''
            AND response_fingerprint = btrim(response_fingerprint)
            AND response_fingerprint <> ''
        )
    )
);

CREATE INDEX user_admin_commands_user_idx
    ON public.user_admin_commands (user_id, created_at, command_id);

CREATE TABLE public.credential_admin_commands (
    command_id uuid PRIMARY KEY,
    command_kind text NOT NULL
        CHECK (command_kind IN ('import', 'rename', 'set_status', 'delete')),
    request_digest bytea NOT NULL CHECK (octet_length(request_digest) = 32),
    credential_id uuid NOT NULL REFERENCES public.credential_keys(id) ON DELETE RESTRICT,
    result_payload jsonb NOT NULL CHECK (jsonb_typeof(result_payload) = 'object'),
    created_at timestamptz NOT NULL DEFAULT transaction_timestamp()
);

CREATE INDEX credential_admin_commands_credential_idx
    ON public.credential_admin_commands (credential_id, created_at, command_id);

CREATE TABLE public.rules_admin_commands (
    command_id uuid PRIMARY KEY,
    command_kind text NOT NULL CHECK (command_kind = 'publish'),
    request_digest bytea NOT NULL CHECK (octet_length(request_digest) = 32),
    locale text NOT NULL REFERENCES public.rules_documents(locale) ON DELETE RESTRICT,
    result_payload jsonb NOT NULL CHECK (jsonb_typeof(result_payload) = 'object'),
    created_at timestamptz NOT NULL DEFAULT transaction_timestamp()
);

CREATE TABLE public.mail_admin_commands (
    command_id uuid PRIMARY KEY,
    command_kind text NOT NULL CHECK (command_kind = 'set_polling_paused'),
    request_digest bytea NOT NULL CHECK (octet_length(request_digest) = 32),
    mailbox_key text NOT NULL REFERENCES public.mail_poll_state(mailbox_key) ON DELETE RESTRICT,
    result_payload jsonb NOT NULL CHECK (jsonb_typeof(result_payload) = 'object'),
    created_at timestamptz NOT NULL DEFAULT transaction_timestamp()
);

CREATE TABLE public.delivery_admin_commands (
    command_id uuid PRIMARY KEY,
    command_kind text NOT NULL CHECK (command_kind IN ('release', 'reconcile', 'retry_failed')),
    request_digest bytea NOT NULL CHECK (octet_length(request_digest) = 32),
    delivery_id uuid NOT NULL REFERENCES public.deliveries(id) ON DELETE RESTRICT,
    result_payload jsonb NOT NULL CHECK (jsonb_typeof(result_payload) = 'object'),
    created_at timestamptz NOT NULL DEFAULT transaction_timestamp()
);

CREATE TABLE public.runtime_config_admin_commands (
    command_id uuid PRIMARY KEY,
    digest_version smallint NOT NULL CHECK (digest_version = 1),
    request_digest bytea NOT NULL CHECK (octet_length(request_digest) = 32),
    state text NOT NULL CHECK (state IN ('pending', 'applied', 'unknown')),
    expected_fingerprint text NOT NULL
        CHECK (expected_fingerprint ~ '^sha256:[0-9a-f]{64}$'
            AND expected_fingerprint = lower(expected_fingerprint)),
    old_fingerprint text NOT NULL
        CHECK (old_fingerprint ~ '^sha256:[0-9a-f]{64}$'
            AND old_fingerprint = lower(old_fingerprint)),
    new_fingerprint text NOT NULL
        CHECK (new_fingerprint ~ '^sha256:[0-9a-f]{64}$'
            AND new_fingerprint = lower(new_fingerprint)),
    candidate_document bytea NOT NULL CHECK (octet_length(candidate_document) BETWEEN 2 AND 4194304),
    restart_required boolean NOT NULL CHECK (restart_required IS TRUE),
    audit_id uuid NOT NULL UNIQUE REFERENCES public.audit_records(id) ON DELETE RESTRICT,
    actor_id text NOT NULL CHECK (actor_id = btrim(actor_id) AND actor_id <> ''),
    request_id text CHECK (request_id IS NULL OR (request_id = btrim(request_id) AND request_id <> '')),
    occurred_at timestamptz NOT NULL,
    unknown_category text NOT NULL DEFAULT '' CHECK (
        unknown_category IN ('', 'current_unavailable', 'current_corrupt', 'replace_unknown',
            'result_corrupt', 'file_conflict')
    ),
    created_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    updated_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    applied_at timestamptz,
    CHECK (expected_fingerprint = old_fingerprint),
    CHECK (updated_at >= created_at),
    CHECK (applied_at IS NULL OR applied_at >= created_at),
    CHECK (
        (state = 'pending' AND applied_at IS NULL AND unknown_category = '')
        OR (state = 'applied' AND applied_at IS NOT NULL AND unknown_category = '')
        OR (state = 'unknown' AND applied_at IS NULL AND unknown_category <> '')
    )
);

CREATE UNIQUE INDEX runtime_config_admin_commands_unresolved_idx
    ON public.runtime_config_admin_commands ((1))
    WHERE state IN ('pending', 'unknown');

CREATE INDEX runtime_config_admin_commands_state_updated_idx
    ON public.runtime_config_admin_commands (state, updated_at, command_id);
