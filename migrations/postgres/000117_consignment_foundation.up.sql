-- 一口价挂单：登记、解析、历史碰撞证据及全局许可。
-- owner: auction-storage（consignment、maintenance）
-- 不变量 1、3、5、6、7、8、10：全局命令唯一，版本与内部事件同事务，持久许可，
-- 失败关闭，密钥仅以信封保存，历史只追加，挂单身份及计价策略冻结。
-- 等级：breaking。按停机门禁执行；旧程序读写、权限和无 owner 恢复尚待真实 PG 验证。

-- 与 auction-domain::consignment::matches_historical_equipment_alias 的旧 EID 规范化一致。
-- 调用方另比较世界；不限制旧串长度、不转数字，也不把全零当成正 EID。
CREATE FUNCTION public.consignment_eid_alias(raw text) RETURNS text
LANGUAGE sql IMMUTABLE STRICT PARALLEL SAFE SET search_path = pg_catalog
AS $function$
    SELECT CASE WHEN raw COLLATE "C" ~ '^[0-9]+$' THEN nullif(ltrim(raw, '0'), '') END
$function$;

-- 与 auction_domain::valid_text 一致：Rust 的 Unicode White_Space 和 C0/C1 控制字符。
CREATE FUNCTION public.consignment_valid_text(value text, maximum integer) RETURNS boolean
LANGUAGE sql IMMUTABLE STRICT PARALLEL SAFE SET search_path = pg_catalog
AS $function$
    SELECT octet_length(value) BETWEEN 1 AND maximum
       AND value COLLATE "C" !~ U&'[\0001-\001F\007F-\009F]'
       AND value COLLATE "C" !~ (
           '^' || U&'[\0020\00A0\1680\2000-\200A\2028\2029\202F\205F\3000]'
           || '|' || U&'[\0020\00A0\1680\2000-\200A\2028\2029\202F\205F\3000]' || '$'
       )
$function$;

-- 安全观察与应用端口使用相同的单行和装备链接排除规则。
CREATE FUNCTION public.consignment_safe_observation_text(value text, maximum integer) RETURNS boolean
LANGUAGE sql IMMUTABLE STRICT PARALLEL SAFE SET search_path = pg_catalog
AS $function$
    SELECT public.consignment_valid_text(value, maximum)
       AND value COLLATE "C" !~* '(/equip/|/equip[?]|key=)'
       AND value COLLATE "C" !~ U&'[\2028\2029]'
$function$;

CREATE FUNCTION public.consignment_positive_uuid(value text) RETURNS boolean
LANGUAGE sql IMMUTABLE STRICT PARALLEL SAFE SET search_path = pg_catalog
AS $function$
    SELECT value COLLATE "C" ~ '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
       AND value <> '00000000-0000-0000-0000-000000000000'
$function$;

-- 保留 Rust ListingReason 的闭集及有/无可信观察之分。
CREATE FUNCTION public.consignment_reason_state(reason text) RETURNS text
LANGUAGE sql IMMUTABLE STRICT PARALLEL SAFE SET search_path = pg_catalog
AS $function$
    SELECT CASE
        WHEN reason IN ('not_tradeable', 'owner_mismatch', 'history_owner_mismatch',
                        'history_alias_conflict', 'history_unproven', 'equipment_busy')
            THEN 'rejected_with_snapshot'
        WHEN reason IN ('equipment_not_found', 'source_request_rejected', 'source_http_invalid',
                        'source_schema_invalid', 'source_too_large', 'source_shape_unconfirmed',
                        'source_identity_mismatch', 'owner_missing', 'credential_unavailable',
                        'lookup_exhausted') THEN 'rejected'
        WHEN reason IN ('lookup_deadline', 'display_expired', 'deposit_expired') THEN 'expired'
        WHEN reason IN ('auction_lot_created', 'seller_ineligible', 'service_disabled',
                        'seller_cancelled', 'admin_delisted') THEN 'closed'
        END
$function$;

CREATE TABLE public.consignment_listings (
    id uuid PRIMARY KEY CONSTRAINT consignment_listing_id CHECK (id <> uuid '00000000-0000-0000-0000-000000000000'),
    seller_user_id uuid NOT NULL REFERENCES public.users(id) ON DELETE RESTRICT
        CONSTRAINT consignment_listing_seller_user_id CHECK (seller_user_id <> uuid '00000000-0000-0000-0000-000000000000'),
    world_id text NOT NULL REFERENCES public.worlds(id) ON DELETE RESTRICT
        CONSTRAINT consignment_listing_world_id CHECK (world_id IN ('main', 'isekai')),
    external_equipment_id text NOT NULL
        CONSTRAINT consignment_listing_external_equipment_id CHECK (external_equipment_id COLLATE "C" ~ '^[1-9][0-9]{0,31}$'),
    mode text NOT NULL CONSTRAINT consignment_listing_mode CHECK (mode IN ('direct', 'consigned')),
    state text NOT NULL,
    price bigint NOT NULL CONSTRAINT consignment_listing_price CHECK (price BETWEEN 1 AND 1000000000),
    pricing_version smallint NOT NULL CONSTRAINT consignment_listing_pricing_version CHECK (pricing_version = 1),
    fee_bps smallint NOT NULL
        CONSTRAINT consignment_listing_fee_bps CHECK (fee_bps BETWEEN 0 AND 9999 AND (mode <> 'direct' OR fee_bps = 0)),
    state_version bigint NOT NULL
        CONSTRAINT consignment_listing_state_version CHECK (state_version BETWEEN 1 AND 9007199254740991),
    service_epoch bigint NOT NULL
        CONSTRAINT consignment_listing_service_epoch CHECK (service_epoch BETWEEN 1 AND 9007199254740991),
    blacklist_count_at_register bigint NOT NULL
        CONSTRAINT consignment_listing_blacklist_count_at_register CHECK (blacklist_count_at_register >= 0),
    current_snapshot_id uuid
        CONSTRAINT consignment_listing_current_snapshot_id CHECK (current_snapshot_id <> uuid '00000000-0000-0000-0000-000000000000'),
    reason_code text,
    created_at timestamptz NOT NULL CONSTRAINT consignment_listing_created_at CHECK (isfinite(created_at)),
    updated_at timestamptz NOT NULL
        CONSTRAINT consignment_listing_updated_at CHECK (isfinite(updated_at) AND updated_at >= created_at),
    expires_at timestamptz NOT NULL
        CONSTRAINT consignment_listing_expires_at CHECK (isfinite(expires_at) AND expires_at > created_at),
    -- 内部登记事务标记由 INSERT 触发器写入全宽 XID，避免仅比较 xmin 的回绕歧义。
    registration_xact_id xid8 NOT NULL,
    CONSTRAINT consignment_listing_state_shape CHECK ((
        CASE state
            WHEN 'resolving' THEN reason_code IS NULL AND current_snapshot_id IS NULL
            WHEN 'displayed' THEN reason_code IS NULL AND mode = 'direct' AND current_snapshot_id IS NOT NULL
            WHEN 'awaiting_deposit' THEN reason_code IS NULL AND mode = 'consigned' AND current_snapshot_id IS NOT NULL
            WHEN 'rejected' THEN public.consignment_reason_state(reason_code) =
                CASE WHEN current_snapshot_id IS NULL THEN 'rejected' ELSE 'rejected_with_snapshot' END
            WHEN 'expired' THEN public.consignment_reason_state(reason_code) = 'expired'
            WHEN 'closed' THEN public.consignment_reason_state(reason_code) = 'closed'
        END
    ) IS TRUE)
);

CREATE INDEX consignment_listings_seller_idx
    ON public.consignment_listings (seller_user_id, created_at DESC, id DESC);
CREATE INDEX consignment_listings_state_idx
    ON public.consignment_listings (state, updated_at DESC, id DESC);
CREATE INDEX consignment_listings_world_mode_idx
    ON public.consignment_listings (world_id, mode, updated_at DESC, id DESC);
CREATE INDEX consignment_listings_expiry_idx ON public.consignment_listings (expires_at, id)
    WHERE state IN ('resolving', 'displayed', 'awaiting_deposit');
CREATE UNIQUE INDEX consignment_listings_seller_active_idx
    ON public.consignment_listings (seller_user_id, world_id, external_equipment_id)
    WHERE state IN ('resolving', 'displayed', 'awaiting_deposit');
CREATE UNIQUE INDEX consignment_listings_equipment_published_idx
    ON public.consignment_listings (world_id, external_equipment_id)
    WHERE state IN ('displayed', 'awaiting_deposit');

-- 快照采用安全投影数组。上游对象里的未知字段不会穿过这条预算及字段白名单。
CREATE FUNCTION public.consignment_valid_stat_groups(groups jsonb) RETURNS boolean
LANGUAGE plpgsql IMMUTABLE STRICT PARALLEL SAFE SET search_path = pg_catalog, public
AS $function$
DECLARE
    stat_group jsonb;
    stat jsonb;
    total integer := 0;
    group_names text[] := ARRAY[]::text[];
    stat_names text[];
BEGIN
    IF jsonb_typeof(groups) <> 'array' THEN RETURN false; END IF;
    IF jsonb_array_length(groups) > 128 THEN RETURN false; END IF;
    FOR stat_group IN SELECT value FROM jsonb_array_elements(groups) LOOP
        IF jsonb_typeof(stat_group) <> 'object' THEN RETURN false; END IF;
        IF NOT (
            stat_group - ARRAY['name', 'stats'] = '{}'::jsonb
            AND jsonb_typeof(stat_group->'name') = 'string'
            AND public.consignment_safe_observation_text(stat_group->>'name', 128)
            AND jsonb_typeof(stat_group->'stats') = 'array'
            AND NOT ((stat_group->>'name') COLLATE "C" = ANY (group_names))
        ) IS TRUE THEN RETURN false; END IF;
        group_names := array_append(group_names, stat_group->>'name');
        total := total + jsonb_array_length(stat_group->'stats');
        IF total > 256 OR jsonb_array_length(stat_group->'stats') > 128 THEN RETURN false; END IF;
        stat_names := ARRAY[]::text[];
        FOR stat IN SELECT value FROM jsonb_array_elements(stat_group->'stats') LOOP
            IF jsonb_typeof(stat) <> 'object' THEN RETURN false; END IF;
            IF NOT (
                stat - ARRAY['name', 'value', 'base'] = '{}'::jsonb
                AND jsonb_typeof(stat->'name') = 'string'
                AND public.consignment_safe_observation_text(stat->>'name', 128)
                AND NOT ((stat->>'name') COLLATE "C" = ANY (stat_names))
                AND CASE WHEN jsonb_typeof(stat->'value') = 'number'
                         THEN abs((stat->>'value')::numeric) <= 1000000000 ELSE false END
                AND CASE jsonb_typeof(stat->'base')
                        WHEN 'null' THEN true
                        WHEN 'number' THEN abs((stat->>'base')::numeric) <= 1000000000
                        ELSE false END
            ) IS TRUE THEN RETURN false; END IF;
            stat_names := array_append(stat_names, stat->>'name');
        END LOOP;
    END LOOP;
    RETURN true;
END
$function$;

CREATE TABLE public.consignment_snapshots (
    id uuid PRIMARY KEY CONSTRAINT consignment_snapshot_id CHECK (id <> uuid '00000000-0000-0000-0000-000000000000'),
    listing_id uuid NOT NULL REFERENCES public.consignment_listings(id) ON DELETE RESTRICT,
    generation bigint NOT NULL
        CONSTRAINT consignment_snapshot_generation CHECK (generation BETWEEN 1 AND 9007199254740991),
    adapter text NOT NULL CONSTRAINT consignment_snapshot_adapter CHECK (adapter = 'hvdata-equip-v1'),
    observed_at timestamptz NOT NULL CONSTRAINT consignment_snapshot_observed_at CHECK (isfinite(observed_at)),
    digest bytea NOT NULL CONSTRAINT consignment_snapshot_digest CHECK (octet_length(digest) = 32),
    world_id text NOT NULL CONSTRAINT consignment_snapshot_world_id CHECK (world_id IN ('main', 'isekai')),
    external_equipment_id text NOT NULL
        CONSTRAINT consignment_snapshot_external_equipment_id CHECK (external_equipment_id COLLATE "C" ~ '^[1-9][0-9]{0,31}$'),
    name text NOT NULL CONSTRAINT consignment_snapshot_name CHECK (public.consignment_safe_observation_text(name, 512)),
    category text NOT NULL
        CONSTRAINT consignment_snapshot_category CHECK (public.consignment_safe_observation_text(category, 128)),
    alt_name text
        CONSTRAINT consignment_snapshot_alt_name CHECK (public.consignment_safe_observation_text(alt_name, 512)),
    level_kind text NOT NULL,
    level_number integer,
    tradeable boolean NOT NULL,
    owner_name text NOT NULL
        CONSTRAINT consignment_snapshot_owner_name CHECK (public.consignment_safe_observation_text(owner_name, 256)),
    stat_groups jsonb NOT NULL
        CONSTRAINT consignment_snapshot_stat_groups CHECK (public.consignment_valid_stat_groups(stat_groups)),
    UNIQUE (listing_id, id),
    UNIQUE (listing_id, generation),
    CONSTRAINT consignment_snapshot_level CHECK ((
        CASE level_kind
            WHEN 'soulbound' THEN level_number IS NULL AND NOT tradeable
            WHEN 'unassigned' THEN level_number IS NULL
            WHEN 'assigned' THEN level_number BETWEEN 1 AND 100000
        END
    ) IS TRUE)
);

ALTER TABLE public.consignment_listings
    ADD CONSTRAINT consignment_listing_current_snapshot_fk FOREIGN KEY (id, current_snapshot_id)
    REFERENCES public.consignment_snapshots(listing_id, id) ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED;

CREATE TABLE public.consignment_registration_secrets (
    listing_id uuid PRIMARY KEY REFERENCES public.consignment_listings(id) ON DELETE RESTRICT,
    aad_equipment_id uuid NOT NULL UNIQUE
        CONSTRAINT consignment_registration_aad_equipment_id CHECK (aad_equipment_id <> uuid '00000000-0000-0000-0000-000000000000'),
    ciphertext bytea,
    nonce bytea,
    encryption_key_id text,
    fingerprint text,
    created_at timestamptz NOT NULL CONSTRAINT consignment_registration_created_at CHECK (isfinite(created_at)),
    updated_at timestamptz NOT NULL
        CONSTRAINT consignment_registration_updated_at CHECK (isfinite(updated_at) AND updated_at >= created_at),
    retired_at timestamptz CONSTRAINT consignment_registration_retired_at CHECK (isfinite(retired_at)),
    CONSTRAINT consignment_registration_envelope CHECK ((
        CASE WHEN retired_at IS NULL THEN
            octet_length(ciphertext) > 0 AND octet_length(nonce) = 12
            AND encryption_key_id COLLATE "C" ~ '^[a-zA-Z0-9][a-zA-Z0-9._-]{0,63}$'
            AND fingerprint COLLATE "C" ~ '^[a-f0-9]{64}$'
        ELSE ciphertext IS NULL AND nonce IS NULL AND encryption_key_id IS NULL AND fingerprint IS NULL
             AND retired_at = updated_at END
    ) IS TRUE)
);

CREATE INDEX consignment_registration_active_key_idx ON public.consignment_registration_secrets
    (encryption_key_id, listing_id) WHERE retired_at IS NULL;

CREATE TABLE public.consignment_replay_keys (
    id uuid PRIMARY KEY CONSTRAINT consignment_replay_key_id CHECK (id <> uuid '00000000-0000-0000-0000-000000000000'),
    write_current boolean NOT NULL,
    -- 32 字节比较密钥和 AES-GCM 的 16 字节认证标签。
    wrapped_key bytea NOT NULL CONSTRAINT consignment_replay_key_wrapped_key CHECK (octet_length(wrapped_key) = 48),
    nonce bytea NOT NULL CONSTRAINT consignment_replay_key_nonce CHECK (octet_length(nonce) = 12),
    encryption_key_id text NOT NULL
        CONSTRAINT consignment_replay_key_encryption_key_id CHECK (encryption_key_id COLLATE "C" ~ '^[a-zA-Z0-9][a-zA-Z0-9._-]{0,63}$'),
    created_at timestamptz NOT NULL CONSTRAINT consignment_replay_key_created_at CHECK (isfinite(created_at)),
    updated_at timestamptz NOT NULL
        CONSTRAINT consignment_replay_key_updated_at CHECK (isfinite(updated_at) AND updated_at >= created_at)
);

CREATE UNIQUE INDEX consignment_replay_keys_write_current_idx ON public.consignment_replay_keys
    (write_current) WHERE write_current;
CREATE INDEX consignment_replay_keys_encryption_key_idx ON public.consignment_replay_keys
    (encryption_key_id, id);

-- 链接目标、身份及数值从原命令列和结构化回执交叉核对；没有任意文本字段。
-- 合同只定义了玩家/Admin 回执。内部成功回执补充端口重放所需的冻结时刻与安全原因。
-- UTC 文本固定形状后再校验真实日期，排除易变的时区别名、相对日期和无穷时刻。
CREATE FUNCTION public.consignment_result_time(value text) RETURNS boolean
LANGUAGE sql STABLE STRICT PARALLEL SAFE SET search_path = pg_catalog
AS $function$
    SELECT value COLLATE "C" ~ '^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}([.][0-9]{1,6})?Z$'
       AND pg_input_is_valid(value, 'timestamp with time zone')
$function$;

CREATE FUNCTION public.consignment_valid_receipt(
    body jsonb, command_id uuid, operation text, listing_id uuid, status smallint
) RETURNS boolean
LANGUAGE sql STABLE PARALLEL SAFE SET search_path = pg_catalog
AS $function$
    SELECT (
        CASE WHEN jsonb_typeof(body) = 'object' THEN
            body - ARRAY['command_id', 'operation', 'listing_id', 'listing_version',
                         'state', 'disposition', 'price']
                - CASE
                    WHEN status = 200 AND operation = 'apply_lookup' AND body->>'disposition' = 'rescheduled'
                        THEN ARRAY['next_attempt_at', 'last_reason']
                    WHEN status = 200 AND operation IN ('apply_lookup', 'auto_close')
                        AND body->>'disposition' IN ('published', 'rejected', 'closed', 'expired')
                        THEN ARRAY['expires_at', 'reason_code']
                    ELSE ARRAY[]::text[]
                  END = '{}'::jsonb
            AND body->'command_id' = to_jsonb(command_id)
            AND body->'operation' = to_jsonb(operation)
            AND body->'listing_id' = coalesce(to_jsonb(listing_id), 'null'::jsonb)
            AND CASE jsonb_typeof(body->'listing_version')
                    WHEN 'null' THEN true
                    WHEN 'number' THEN (body->>'listing_version') COLLATE "C" ~ '^[1-9][0-9]{0,15}$'
                        AND (body->>'listing_version')::numeric <= 9007199254740991
                    ELSE false END
            AND (body->'state' = 'null'::jsonb OR body->>'state' IN (
                'resolving', 'displayed', 'awaiting_deposit', 'rejected', 'expired', 'closed'
            ))
            AND CASE jsonb_typeof(body->'price')
                    WHEN 'null' THEN true
                    WHEN 'number' THEN (body->>'price') COLLATE "C" ~ '^[1-9][0-9]{0,9}$'
                        AND (body->>'price')::numeric <= 1000000000
                    ELSE false END
            AND CASE
                WHEN status >= 400 THEN body->>'disposition' = 'rejected'
                WHEN operation = 'register' THEN body->>'disposition' = 'registered'
                    AND listing_id IS NOT NULL AND body->>'listing_version' = '1'
                    AND body->>'state' = 'resolving' AND body->'price' <> 'null'::jsonb
                WHEN operation = 'reprice' THEN body->>'disposition' IN ('repriced', 'unchanged')
                    AND listing_id IS NOT NULL AND body->'listing_version' <> 'null'::jsonb
                    AND body->>'state' IN ('resolving', 'displayed', 'awaiting_deposit')
                    AND body->'price' <> 'null'::jsonb
                WHEN operation IN ('withdraw', 'admin_delist') THEN body->>'disposition' = 'closed'
                    AND listing_id IS NOT NULL AND body->'listing_version' <> 'null'::jsonb
                    AND body->>'state' = 'closed'
                WHEN operation = 'apply_lookup' AND body->>'disposition' = 'rescheduled' THEN
                    listing_id IS NOT NULL AND body->'state' = 'null'::jsonb
                    AND body->'listing_version' = 'null'::jsonb AND body->'price' = 'null'::jsonb
                    AND public.consignment_result_time(body->>'next_attempt_at')
                    AND body->>'last_reason' IN ('source_unavailable', 'source_timeout', 'source_rate_limited')
                WHEN operation = 'auto_close' AND body->>'disposition' IN ('unchanged', 'conflicts_recorded') THEN
                    listing_id IS NOT NULL AND body->'state' = 'null'::jsonb
                    AND body->'listing_version' = 'null'::jsonb AND body->'price' = 'null'::jsonb
                WHEN operation IN ('apply_lookup', 'auto_close') THEN
                    listing_id IS NOT NULL AND body->'listing_version' <> 'null'::jsonb
                    AND body->'price' <> 'null'::jsonb
                    AND public.consignment_result_time(body->>'expires_at')
                    AND CASE body->>'disposition'
                        WHEN 'published' THEN operation = 'apply_lookup'
                            AND body->>'state' IN ('displayed', 'awaiting_deposit')
                            AND body->'reason_code' = 'null'::jsonb
                        WHEN 'rejected' THEN operation = 'apply_lookup' AND body->>'state' = 'rejected'
                            AND public.consignment_reason_state(body->>'reason_code') IN ('rejected', 'rejected_with_snapshot')
                        WHEN 'closed' THEN body->>'state' = 'closed'
                            AND public.consignment_reason_state(body->>'reason_code') = 'closed'
                        WHEN 'expired' THEN body->>'state' = 'expired'
                            AND public.consignment_reason_state(body->>'reason_code') = 'expired'
                        END
            END
        END
    ) IS TRUE
$function$;

CREATE TABLE public.consignment_commands (
    command_id uuid PRIMARY KEY
        CONSTRAINT consignment_command_command_id CHECK (command_id <> uuid '00000000-0000-0000-0000-000000000000'),
    actor_kind text NOT NULL,
    actor_ref text NOT NULL,
    operation text NOT NULL,
    request_target text NOT NULL,
    request_digest bytea NOT NULL
        CONSTRAINT consignment_command_request_digest CHECK (octet_length(request_digest) = 32),
    replay_key_id uuid REFERENCES public.consignment_replay_keys(id) ON DELETE RESTRICT,
    listing_id uuid REFERENCES public.consignment_listings(id) ON DELETE RESTRICT,
    result_status smallint NOT NULL,
    result_code text NOT NULL,
    result_body jsonb NOT NULL,
    created_at timestamptz NOT NULL CONSTRAINT consignment_command_created_at CHECK (isfinite(created_at)),
    CONSTRAINT consignment_command_actor CHECK ((
        CASE actor_kind
            WHEN 'public_user' THEN operation IN ('register', 'reprice', 'withdraw')
                AND public.consignment_positive_uuid(actor_ref)
            WHEN 'admin' THEN operation = 'admin_delist'
                AND public.consignment_valid_text(actor_ref, 256) AND strpos(actor_ref, ':') = 0
            WHEN 'worker' THEN operation = 'apply_lookup' AND public.consignment_valid_text(actor_ref, 256)
            WHEN 'system' THEN operation = 'auto_close' AND public.consignment_valid_text(actor_ref, 256)
        END
    ) IS TRUE),
    CONSTRAINT consignment_command_target CHECK (
        (operation = 'register' AND request_target = 'new' AND replay_key_id IS NOT NULL
            AND (result_status = 202) = (listing_id IS NOT NULL))
        OR (operation <> 'register' AND public.consignment_positive_uuid(request_target)
            AND replay_key_id IS NULL AND (listing_id IS NULL OR listing_id::text = request_target))
    ),
    CONSTRAINT consignment_command_result_code CHECK (
        (result_status = 202 AND operation = 'register' AND result_code = 'accepted')
        OR (result_status = 200 AND operation <> 'register' AND result_code = 'ok')
        OR (operation = 'register' AND (
            (result_status = 403 AND result_code = 'participant_ineligible')
            OR (result_status = 409 AND result_code IN (
                'already_open', 'equipment_busy', 'history_unproven', 'history_owner_mismatch',
                'history_alias_conflict', 'fee_changed', 'world_unavailable'
            ))
            OR (result_status = 429 AND result_code IN ('registration_rate_limited', 'listing_limit_reached'))
        ))
        OR (operation IN ('reprice', 'withdraw', 'admin_delist') AND (
            (result_status = 404 AND result_code = 'listing_not_found')
            OR (result_status = 409 AND result_code IN ('stale_version', 'listing_not_active'))
        ))
    ),
    CONSTRAINT consignment_command_receipt CHECK (
        public.consignment_valid_receipt(result_body, command_id, operation, listing_id, result_status)
    )
);

CREATE INDEX consignment_commands_actor_idx ON public.consignment_commands (actor_kind, actor_ref, created_at);
CREATE INDEX consignment_commands_listing_idx ON public.consignment_commands (listing_id);
CREATE INDEX consignment_commands_replay_key_idx ON public.consignment_commands (replay_key_id);

CREATE TABLE public.consignment_events (
    listing_id uuid NOT NULL REFERENCES public.consignment_listings(id) ON DELETE RESTRICT,
    state_version bigint NOT NULL
        CONSTRAINT consignment_event_state_version CHECK (state_version BETWEEN 1 AND 9007199254740991),
    command_id uuid NOT NULL REFERENCES public.consignment_commands(command_id) ON DELETE RESTRICT,
    actor_kind text NOT NULL,
    event_kind text NOT NULL,
    from_state text,
    to_state text NOT NULL,
    reason_code text,
    price bigint NOT NULL CONSTRAINT consignment_event_price CHECK (price BETWEEN 1 AND 1000000000),
    occurred_at timestamptz NOT NULL CONSTRAINT consignment_event_occurred_at CHECK (isfinite(occurred_at)),
    PRIMARY KEY (listing_id, state_version),
    CONSTRAINT consignment_event_edge CHECK ((
        CASE
            WHEN state_version = 1 THEN event_kind = 'registered' AND actor_kind = 'public_user'
                AND from_state IS NULL AND to_state = 'resolving' AND reason_code IS NULL
            WHEN from_state IN ('resolving', 'displayed', 'awaiting_deposit') THEN
                CASE event_kind
                    WHEN 'repriced' THEN actor_kind = 'public_user' AND to_state = from_state AND reason_code IS NULL
                    WHEN 'published' THEN actor_kind = 'worker' AND from_state = 'resolving'
                        AND to_state IN ('displayed', 'awaiting_deposit') AND reason_code IS NULL
                    WHEN 'rejected' THEN actor_kind = 'worker' AND from_state = 'resolving' AND to_state = 'rejected'
                        AND public.consignment_reason_state(reason_code) IN ('rejected', 'rejected_with_snapshot')
                    WHEN 'expired' THEN actor_kind = 'system' AND to_state = 'expired' AND reason_code =
                        CASE from_state WHEN 'resolving' THEN 'lookup_deadline'
                            WHEN 'displayed' THEN 'display_expired' ELSE 'deposit_expired' END
                    WHEN 'closed' THEN to_state = 'closed' AND
                        CASE actor_kind
                            WHEN 'public_user' THEN reason_code = 'seller_cancelled'
                            WHEN 'admin' THEN reason_code = 'admin_delisted'
                            WHEN 'system' THEN reason_code IN ('auction_lot_created', 'seller_ineligible', 'service_disabled')
                        END
                END
        END
    ) IS TRUE)
);

CREATE INDEX consignment_events_command_idx ON public.consignment_events (command_id);

CREATE TABLE public.consignment_listing_lot_baselines (
    listing_id uuid NOT NULL REFERENCES public.consignment_listings(id) ON DELETE RESTRICT,
    lot_id uuid NOT NULL REFERENCES public.lots(id) ON DELETE RESTRICT
        CONSTRAINT consignment_baseline_lot_id CHECK (lot_id <> uuid '00000000-0000-0000-0000-000000000000'),
    PRIMARY KEY (listing_id, lot_id)
);
CREATE INDEX consignment_listing_lot_baselines_lot_idx
    ON public.consignment_listing_lot_baselines (lot_id, listing_id);

CREATE TABLE public.consignment_listing_auction_conflicts (
    listing_id uuid NOT NULL REFERENCES public.consignment_listings(id) ON DELETE RESTRICT,
    lot_id uuid NOT NULL REFERENCES public.lots(id) ON DELETE RESTRICT
        CONSTRAINT consignment_conflict_lot_id CHECK (lot_id <> uuid '00000000-0000-0000-0000-000000000000'),
    detected_at timestamptz NOT NULL CONSTRAINT consignment_conflict_detected_at CHECK (isfinite(detected_at)),
    command_id uuid NOT NULL REFERENCES public.consignment_commands(command_id) ON DELETE RESTRICT,
    PRIMARY KEY (listing_id, lot_id)
);
CREATE INDEX consignment_listing_auction_conflicts_lot_idx
    ON public.consignment_listing_auction_conflicts (lot_id, listing_id);
CREATE INDEX consignment_listing_auction_conflicts_command_idx
    ON public.consignment_listing_auction_conflicts (command_id);

CREATE TABLE public.consignment_lookup_work (
    id uuid PRIMARY KEY CONSTRAINT consignment_lookup_work_id CHECK (id <> uuid '00000000-0000-0000-0000-000000000000'),
    listing_id uuid NOT NULL REFERENCES public.consignment_listings(id) ON DELETE RESTRICT,
    generation bigint NOT NULL
        CONSTRAINT consignment_lookup_work_generation CHECK (generation BETWEEN 1 AND 9007199254740991),
    state text NOT NULL,
    attempt_count smallint NOT NULL
        CONSTRAINT consignment_lookup_work_attempt_count CHECK (attempt_count BETWEEN 0 AND 6),
    fence bigint NOT NULL CONSTRAINT consignment_lookup_work_fence CHECK (fence >= 0),
    next_attempt_at timestamptz CONSTRAINT consignment_lookup_work_next_attempt_at CHECK (isfinite(next_attempt_at)),
    lease_owner text,
    token uuid CONSTRAINT consignment_lookup_work_token CHECK (token <> uuid '00000000-0000-0000-0000-000000000000'),
    lease_until timestamptz CONSTRAINT consignment_lookup_work_lease_until CHECK (isfinite(lease_until)),
    network_deadline timestamptz,
    last_reason text
        CONSTRAINT consignment_lookup_work_last_reason CHECK (last_reason IN ('source_unavailable', 'source_timeout', 'source_rate_limited')),
    completed_at timestamptz CONSTRAINT consignment_lookup_work_completed_at CHECK (isfinite(completed_at)),
    created_at timestamptz NOT NULL CONSTRAINT consignment_lookup_work_created_at CHECK (isfinite(created_at)),
    updated_at timestamptz NOT NULL
        CONSTRAINT consignment_lookup_work_updated_at CHECK (isfinite(updated_at) AND updated_at >= created_at),
    UNIQUE (listing_id, generation),
    CONSTRAINT consignment_lookup_work_state CHECK ((
        CASE state
            WHEN 'queued' THEN next_attempt_at IS NOT NULL AND completed_at IS NULL
            WHEN 'leased' THEN next_attempt_at IS NULL AND completed_at IS NULL
                AND fence > 0 AND attempt_count > 0
            WHEN 'done' THEN next_attempt_at IS NULL AND completed_at IS NOT NULL
            WHEN 'cancelled' THEN next_attempt_at IS NULL AND completed_at IS NOT NULL
        END
        AND CASE WHEN state = 'leased' THEN
            public.consignment_valid_text(lease_owner, 256) AND token IS NOT NULL AND lease_until IS NOT NULL
            AND network_deadline IS NOT NULL AND isfinite(network_deadline) AND network_deadline <= lease_until
        ELSE lease_owner IS NULL AND token IS NULL AND lease_until IS NULL AND network_deadline IS NULL END
    ) IS TRUE)
);
CREATE INDEX consignment_lookup_work_queued_idx ON public.consignment_lookup_work
    (next_attempt_at, id) WHERE state = 'queued';
CREATE INDEX consignment_lookup_work_leased_idx ON public.consignment_lookup_work
    (lease_until, id) WHERE state = 'leased';

CREATE TABLE public.consignment_lookup_control (
    id smallint PRIMARY KEY CONSTRAINT consignment_lookup_control_id CHECK (id = 1),
    enabled_epoch bigint NOT NULL
        CONSTRAINT consignment_lookup_control_enabled_epoch CHECK (enabled_epoch BETWEEN 1 AND 9007199254740991),
    admitting boolean NOT NULL,
    next_allowed_at timestamptz NOT NULL
        CONSTRAINT consignment_lookup_control_next_allowed_at CHECK (isfinite(next_allowed_at)),
    cooldown_until timestamptz CONSTRAINT consignment_lookup_control_cooldown_until CHECK (isfinite(cooldown_until)),
    holder_work_id uuid REFERENCES public.consignment_lookup_work(id) ON DELETE RESTRICT,
    holder_fence bigint CONSTRAINT consignment_lookup_control_holder_fence CHECK (holder_fence > 0),
    holder_token uuid
        CONSTRAINT consignment_lookup_control_holder_token CHECK (holder_token <> uuid '00000000-0000-0000-0000-000000000000'),
    permit_until timestamptz CONSTRAINT consignment_lookup_control_permit_until CHECK (isfinite(permit_until)),
    worker_ready_until timestamptz
        CONSTRAINT consignment_lookup_control_worker_ready_until CHECK (isfinite(worker_ready_until)),
    scan_after uuid
        CONSTRAINT consignment_lookup_control_scan_after CHECK (scan_after <> uuid '00000000-0000-0000-0000-000000000000'),
    CONSTRAINT consignment_lookup_control_holder CHECK (
        num_nonnulls(holder_work_id, holder_fence, holder_token, permit_until) IN (0, 4)
    )
);

INSERT INTO public.consignment_lookup_control (
    id, enabled_epoch, admitting, next_allowed_at, cooldown_until, holder_work_id,
    holder_fence, holder_token, permit_until, worker_ready_until, scan_after
) VALUES (1, 1, false, transaction_timestamp(), NULL, NULL, NULL, NULL, NULL, NULL, NULL);

-- 不变量 6、10：版本、身份与冻结策略受保护；关闭/过期保留最后的观察和期限。
CREATE FUNCTION public.consignment_guard_listing() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
BEGIN
    IF TG_OP = 'INSERT' THEN
        NEW.registration_xact_id := pg_current_xact_id();
        IF NEW.state_version <> 1 OR NEW.state <> 'resolving' THEN
            RAISE EXCEPTION '寄售挂单必须从登记版本开始'
                USING ERRCODE = '23514', CONSTRAINT = 'consignment_listing_transition';
        END IF;
        RETURN NEW;
    END IF;

    IF ROW(OLD.id, OLD.seller_user_id, OLD.world_id, OLD.external_equipment_id, OLD.mode,
           OLD.pricing_version, OLD.fee_bps, OLD.service_epoch, OLD.blacklist_count_at_register,
           OLD.created_at, OLD.registration_xact_id)
       IS DISTINCT FROM
       ROW(NEW.id, NEW.seller_user_id, NEW.world_id, NEW.external_equipment_id, NEW.mode,
           NEW.pricing_version, NEW.fee_bps, NEW.service_epoch, NEW.blacklist_count_at_register,
           NEW.created_at, NEW.registration_xact_id)
       OR OLD.state NOT IN ('resolving', 'displayed', 'awaiting_deposit')
       OR NEW.state_version <> OLD.state_version + 1 OR NEW.updated_at < OLD.updated_at
       -- 不变量 6：用本次数据库写入时刻比较旧截止；系统强制关闭先于到期。
       OR (NEW.updated_at >= OLD.expires_at
           AND NEW.state <> 'expired'
           AND NOT (NEW.state = 'closed'
               AND NEW.reason_code IN ('auction_lot_created', 'seller_ineligible', 'service_disabled')))
       OR (CASE
            WHEN NEW.state = OLD.state THEN NEW.price = OLD.price
                OR NEW.current_snapshot_id IS DISTINCT FROM OLD.current_snapshot_id
                OR NEW.expires_at <> OLD.expires_at
            WHEN NEW.state IN ('displayed', 'awaiting_deposit') THEN OLD.state <> 'resolving'
                OR NEW.price <> OLD.price OR NEW.expires_at <= NEW.updated_at
            WHEN NEW.state = 'rejected' THEN OLD.state <> 'resolving'
                OR NEW.price <> OLD.price OR NEW.expires_at <> OLD.expires_at
            WHEN NEW.state IN ('expired', 'closed') THEN NEW.price <> OLD.price
                OR NEW.current_snapshot_id IS DISTINCT FROM OLD.current_snapshot_id
                OR NEW.expires_at <> OLD.expires_at
                OR (NEW.state = 'expired' AND NEW.updated_at < OLD.expires_at)
            ELSE true END)
    THEN
        RAISE EXCEPTION '寄售挂单状态、版本或冻结字段变化不合法'
            USING ERRCODE = '23514', CONSTRAINT = 'consignment_listing_transition';
    END IF;
    RETURN NEW;
END
$function$;

CREATE TRIGGER consignment_guard_listing BEFORE INSERT OR UPDATE ON public.consignment_listings
FOR EACH ROW EXECUTE FUNCTION public.consignment_guard_listing();

-- 不变量 3、8：每次投影写入必须在提交前拥有同版本事实及本事务的新命令。
CREATE FUNCTION public.consignment_check_listing_event() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
DECLARE
    previous_state text;
BEGIN
    IF TG_OP = 'UPDATE' THEN previous_state := OLD.state; END IF;
    IF NOT EXISTS (
        SELECT 1 FROM public.consignment_events AS e
        JOIN public.consignment_commands AS c ON c.command_id = e.command_id
        WHERE e.listing_id = NEW.id AND e.state_version = NEW.state_version
          AND e.from_state IS NOT DISTINCT FROM previous_state AND e.to_state = NEW.state
          AND e.reason_code IS NOT DISTINCT FROM NEW.reason_code AND e.price = NEW.price
          AND e.occurred_at = NEW.updated_at
          -- 寄售在顶层事务写入；保存点内新行的 xmin 是子事务号，会失败关闭。
          AND c.xmin = pg_current_xact_id()::xid
          AND (
            (c.listing_id = NEW.id
              AND (c.result_status >= 400 OR (
                  c.result_body->>'listing_version' = NEW.state_version::text
                  AND c.result_body->>'state' = NEW.state
                  AND (c.result_body->'price' = 'null'::jsonb OR c.result_body->>'price' = NEW.price::text)
                  AND (c.operation NOT IN ('apply_lookup', 'auto_close') OR (
                      (c.result_body->>'expires_at')::timestamptz = NEW.expires_at
                      AND c.result_body->>'reason_code' IS NOT DISTINCT FROM NEW.reason_code
                      AND c.result_body->>'disposition' = e.event_kind
                  ))
              ))
              AND CASE e.event_kind
                    WHEN 'registered' THEN c.operation = 'register' AND c.result_status = 202
                        AND c.actor_ref = NEW.seller_user_id::text
                        AND c.result_body->>'price' = NEW.price::text
                        AND EXISTS (
                            SELECT 1 FROM public.consignment_replay_keys AS k
                            WHERE k.id = c.replay_key_id AND k.write_current
                        )
                    WHEN 'repriced' THEN c.operation = 'reprice' AND c.actor_ref = NEW.seller_user_id::text
                        AND c.result_body->>'disposition' = 'repriced'
                    WHEN 'published' THEN c.operation = 'apply_lookup' AND c.result_status = 200 AND EXISTS (
                        SELECT 1 FROM public.consignment_snapshots AS s
                        WHERE s.id = NEW.current_snapshot_id AND s.tradeable
                    )
                    WHEN 'rejected' THEN c.operation = 'apply_lookup' AND c.result_status = 200
                    WHEN 'expired' THEN
                        c.operation IN ('auto_close', 'apply_lookup') OR c.result_code = 'listing_not_active'
                    WHEN 'closed' THEN CASE e.actor_kind
                        WHEN 'public_user' THEN c.operation = 'withdraw' AND c.result_status = 200
                            AND c.actor_ref = NEW.seller_user_id::text
                        WHEN 'admin' THEN c.operation = 'admin_delist' AND c.result_status = 200
                        WHEN 'system' THEN
                            c.operation IN ('auto_close', 'apply_lookup') OR c.result_code = 'listing_not_active'
                        ELSE false END
                    ELSE false END
            )
            -- 登记或发布可在同一装备锁内先清理其他失效挂单，系统事件关联外层命令。
            OR (e.actor_kind = 'system' AND e.event_kind IN ('expired', 'closed')
                AND c.operation IN ('register', 'apply_lookup')
                AND c.listing_id IS DISTINCT FROM NEW.id
                AND (
                    -- 拒绝登记的请求目标固定为 new，账本不给它伪造挂单外键。
                    (c.listing_id IS NULL AND c.operation = 'register' AND c.result_status >= 400)
                    OR EXISTS (
                        SELECT 1 FROM public.consignment_listings AS target
                        WHERE target.id = c.listing_id AND target.world_id = NEW.world_id
                          AND target.external_equipment_id = NEW.external_equipment_id
                    )
                )
            )
          )
    ) THEN
        RAISE EXCEPTION '寄售挂单版本缺少同事务的匹配事件或命令'
            USING ERRCODE = '23514', CONSTRAINT = 'consignment_listing_version_event';
    END IF;
    RETURN NULL;
END
$function$;

CREATE CONSTRAINT TRIGGER consignment_listing_version_event
AFTER INSERT OR UPDATE ON public.consignment_listings DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION public.consignment_check_listing_event();

-- 不变量 1、3、8：声明产生挂单版本的成功回执只能与该版本的事件一起提交。
CREATE FUNCTION public.consignment_check_command_event() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM public.consignment_events AS e
        WHERE e.command_id = NEW.command_id AND e.listing_id = NEW.listing_id
          AND e.state_version = (NEW.result_body->>'listing_version')::bigint
          AND e.event_kind = NEW.result_body->>'disposition'
          -- 寄售在顶层事务写入；保存点内事件的 xmin 是子事务号，会失败关闭。
          AND e.xmin = pg_current_xact_id()::xid
    ) THEN
        RAISE EXCEPTION '寄售成功回执缺少同事务的匹配事件'
            USING ERRCODE = '23514', CONSTRAINT = 'consignment_command_version_event';
    END IF;
    RETURN NULL;
END
$function$;

CREATE CONSTRAINT TRIGGER consignment_command_version_event
AFTER INSERT ON public.consignment_commands DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW
WHEN (NEW.result_status < 400 AND NEW.result_body->>'disposition' IN
    ('registered', 'repriced', 'closed', 'published', 'rejected', 'expired'))
EXECUTE FUNCTION public.consignment_check_command_event();

CREATE FUNCTION public.consignment_check_event_version() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM public.consignment_listings AS l
        WHERE l.id = NEW.listing_id AND l.state_version >= NEW.state_version
    ) THEN
        RAISE EXCEPTION '寄售事件没有对应的挂单版本'
            USING ERRCODE = '23514', CONSTRAINT = 'consignment_event_has_version';
    END IF;
    RETURN NULL;
END
$function$;

CREATE CONSTRAINT TRIGGER consignment_event_has_version
AFTER INSERT ON public.consignment_events DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION public.consignment_check_event_version();

CREATE FUNCTION public.consignment_check_snapshot_identity() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM public.consignment_listings AS l
        WHERE l.id = NEW.listing_id AND l.world_id = NEW.world_id
          AND l.external_equipment_id = NEW.external_equipment_id
    ) THEN
        RAISE EXCEPTION '寄售快照与挂单装备身份不一致'
            USING ERRCODE = '23514', CONSTRAINT = 'consignment_snapshot_identity';
    END IF;
    RETURN NEW;
END
$function$;

CREATE TRIGGER consignment_snapshot_identity AFTER INSERT ON public.consignment_snapshots
FOR EACH ROW EXECUTE FUNCTION public.consignment_check_snapshot_identity();

-- 不变量 6、8：登记集合只能在首条登记命令的事务内建立，事后碰撞不能被补成历史。
CREATE FUNCTION public.consignment_check_lot_link() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM public.consignment_listings AS l
        CROSS JOIN public.lots AS a
        JOIN public.equipment AS equip ON equip.id = a.equipment_id
        WHERE l.id = NEW.listing_id AND a.id = NEW.lot_id AND equip.world_id = l.world_id
          AND public.consignment_eid_alias(equip.external_equipment_id) = l.external_equipment_id
          -- 寄售写入使用顶层事务；拍卖收件在独立顶层事务建立 lot。
          AND a.xmin <> pg_current_xact_id()::xid
    ) THEN
        RAISE EXCEPTION '寄售关联 lot 与挂单装备身份或提交快照不一致'
            USING ERRCODE = '23514', CONSTRAINT = 'consignment_lot_identity';
    END IF;

    IF TG_TABLE_NAME = 'consignment_listing_lot_baselines' THEN
        IF NOT EXISTS (
            SELECT 1 FROM public.consignment_listings AS l
            JOIN public.consignment_events AS e ON e.listing_id = l.id AND e.state_version = 1
            JOIN public.consignment_commands AS c ON c.command_id = e.command_id
            WHERE l.id = NEW.listing_id AND l.registration_xact_id = pg_current_xact_id()
              -- 寄售登记必须在顶层事务写挂单；保存点行的 xmin 不匹配。
              AND l.xmin = pg_current_xact_id()::xid
              AND c.operation = 'register' AND c.result_status = 202 AND c.listing_id = l.id
              -- 寄售登记命令也必须在同一个顶层事务写入。
              AND c.xmin = pg_current_xact_id()::xid
        ) THEN
            RAISE EXCEPTION '寄售基线必须随首次登记提交'
                USING ERRCODE = '23514', CONSTRAINT = 'consignment_baseline_registration';
        END IF;
        IF NOT EXISTS (
            SELECT 1 FROM public.consignment_listing_auction_conflicts AS c
            WHERE c.listing_id = NEW.listing_id AND c.lot_id = NEW.lot_id
        ) THEN RETURN NULL; END IF;
    ELSE
        IF NOT EXISTS (
            SELECT 1 FROM public.consignment_listing_lot_baselines AS b
            WHERE b.listing_id = NEW.listing_id AND b.lot_id = NEW.lot_id
        ) THEN RETURN NULL; END IF;
    END IF;
    RAISE EXCEPTION '寄售登记基线与碰撞事实重叠'
        USING ERRCODE = '23514', CONSTRAINT = 'consignment_lot_baseline_disjoint';
END
$function$;

CREATE CONSTRAINT TRIGGER consignment_baseline_lot_identity
AFTER INSERT ON public.consignment_listing_lot_baselines DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION public.consignment_check_lot_link();
CREATE CONSTRAINT TRIGGER consignment_conflict_lot_identity
AFTER INSERT ON public.consignment_listing_auction_conflicts DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION public.consignment_check_lot_link();

-- 不变量 7：退休只清除信封，轮换不得恢复已退休的登记凭证。
CREATE FUNCTION public.consignment_guard_retired_envelope() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
BEGIN
    IF OLD.retired_at IS NOT NULL AND OLD IS DISTINCT FROM NEW THEN
        RAISE EXCEPTION '已退休的寄售信封不能修改'
            USING ERRCODE = '23514', CONSTRAINT = 'consignment_registration_retirement';
    END IF;
    RETURN NEW;
END
$function$;

CREATE TRIGGER consignment_registration_retirement BEFORE UPDATE ON public.consignment_registration_secrets
FOR EACH ROW EXECUTE FUNCTION public.consignment_guard_retired_envelope();

-- 不变量 1、3、8：事实只追加，维护账号也不能经普通 SQL 改写。
CREATE FUNCTION public.consignment_reject_fact_mutation() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
BEGIN
    RAISE EXCEPTION '寄售历史事实只允许追加' USING ERRCODE = '55000';
END
$function$;

CREATE TRIGGER consignment_commands_append_only BEFORE UPDATE OR DELETE ON public.consignment_commands
FOR EACH ROW EXECUTE FUNCTION public.consignment_reject_fact_mutation();
CREATE TRIGGER consignment_commands_no_truncate BEFORE TRUNCATE ON public.consignment_commands
FOR EACH STATEMENT EXECUTE FUNCTION public.consignment_reject_fact_mutation();
CREATE TRIGGER consignment_events_append_only BEFORE UPDATE OR DELETE ON public.consignment_events
FOR EACH ROW EXECUTE FUNCTION public.consignment_reject_fact_mutation();
CREATE TRIGGER consignment_events_no_truncate BEFORE TRUNCATE ON public.consignment_events
FOR EACH STATEMENT EXECUTE FUNCTION public.consignment_reject_fact_mutation();
CREATE TRIGGER consignment_snapshots_append_only BEFORE UPDATE OR DELETE ON public.consignment_snapshots
FOR EACH ROW EXECUTE FUNCTION public.consignment_reject_fact_mutation();
CREATE TRIGGER consignment_snapshots_no_truncate BEFORE TRUNCATE ON public.consignment_snapshots
FOR EACH STATEMENT EXECUTE FUNCTION public.consignment_reject_fact_mutation();
CREATE TRIGGER consignment_baselines_append_only BEFORE UPDATE OR DELETE ON public.consignment_listing_lot_baselines
FOR EACH ROW EXECUTE FUNCTION public.consignment_reject_fact_mutation();
CREATE TRIGGER consignment_baselines_no_truncate BEFORE TRUNCATE ON public.consignment_listing_lot_baselines
FOR EACH STATEMENT EXECUTE FUNCTION public.consignment_reject_fact_mutation();
CREATE TRIGGER consignment_conflicts_append_only BEFORE UPDATE OR DELETE ON public.consignment_listing_auction_conflicts
FOR EACH ROW EXECUTE FUNCTION public.consignment_reject_fact_mutation();
CREATE TRIGGER consignment_conflicts_no_truncate BEFORE TRUNCATE ON public.consignment_listing_auction_conflicts
FOR EACH STATEMENT EXECUTE FUNCTION public.consignment_reject_fact_mutation();

-- 恢复重授权沿用原函数。寄售授权在通用撤权和 SELECT 之后单列，不进入普通可变表名单。
CREATE OR REPLACE FUNCTION public.apply_auction_runtime_privileges() RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $function$
DECLARE
    runtime_role name := 'auction_runtime';
    runtime_oid oid;
BEGIN
    SELECT role.oid INTO runtime_oid
    FROM pg_catalog.pg_roles AS role
    WHERE role.rolname = runtime_role;

    IF runtime_oid IS NULL THEN
        RAISE EXCEPTION
            '运行时角色 % 不存在：请先创建该 LOGIN 账号并改为三角色 [database].username 后重试',
            runtime_role;
    END IF;
    IF runtime_role = current_user THEN
        RAISE EXCEPTION
            '迁移 owner 不能同时是运行时角色 %：服务与迁移必须使用不同账号', runtime_role;
    END IF;
    IF EXISTS (
        SELECT 1 FROM pg_catalog.pg_roles AS role
        WHERE role.oid = runtime_oid
          AND (role.rolsuper OR role.rolcreatedb OR role.rolcreaterole
               OR role.rolbypassrls OR role.rolreplication)
    ) THEN
        RAISE EXCEPTION
            '运行时角色 % 不得具有 superuser、CREATEDB、CREATEROLE、BYPASSRLS 或 REPLICATION 属性',
            runtime_role;
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM pg_catalog.pg_roles AS role
        WHERE role.oid = runtime_oid AND role.rolcanlogin
    ) THEN
        RAISE EXCEPTION '运行时角色 % 必须允许登录（LOGIN）', runtime_role;
    END IF;
    -- 对象 owner 天然拥有全部 DDL 与 TRUNCATE 权限，不能靠 GRANT/REVOKE 收口。
    IF EXISTS (
        SELECT 1 FROM pg_catalog.pg_class AS relation
        WHERE relation.relowner = runtime_oid
          AND relation.relnamespace = 'public'::pg_catalog.regnamespace
    ) OR EXISTS (
        SELECT 1 FROM pg_catalog.pg_namespace AS namespace
        WHERE namespace.nspname = 'public' AND namespace.nspowner = runtime_oid
    ) THEN
        RAISE EXCEPTION '运行时角色 % 不得拥有 public schema 或其对象', runtime_role;
    END IF;
    -- 继承 owner 或 superuser 同样得到 DDL；仅检查成员关系，不改动角色定义。
    IF EXISTS (
        SELECT 1
        FROM pg_catalog.pg_roles AS granted_role
        WHERE granted_role.oid <> runtime_oid
          AND pg_catalog.pg_has_role(runtime_oid, granted_role.oid, 'MEMBER')
          AND (
              granted_role.rolsuper
              OR granted_role.oid = (SELECT datdba FROM pg_catalog.pg_database
                                     WHERE datname = current_database())
              OR EXISTS (
                  SELECT 1 FROM pg_catalog.pg_class AS relation
                  WHERE relation.relowner = granted_role.oid
                    AND relation.relnamespace = 'public'::pg_catalog.regnamespace
              )
          )
    ) THEN
        RAISE EXCEPTION
            '运行时角色 % 不得继承迁移 owner、数据库 owner 或 superuser 角色', runtime_role;
    END IF;

    -- schema 面：只读使用，不允许运行账号建立任何 schema 对象。
    REVOKE ALL ON SCHEMA public FROM auction_runtime;
    GRANT USAGE ON SCHEMA public TO auction_runtime;

    -- 从空权限集合开始重建：先撤销该 owner 曾经授予的全部对象权限（含 TRUNCATE）。
    REVOKE ALL ON ALL TABLES IN SCHEMA public FROM auction_runtime;
    REVOKE ALL ON ALL SEQUENCES IN SCHEMA public FROM auction_runtime;

    -- 默认读取面：普通表、只追加表、视图与 _sqlx_migrations 统一只读（后续不加任何写入授权）。
    GRANT SELECT ON ALL TABLES IN SCHEMA public TO auction_runtime;

    -- 可变表：完整读写，但仍无 TRUNCATE 与 DDL。
    GRANT INSERT, UPDATE, DELETE ON TABLE
        admin_sessions, auction_commands, auction_forum_bid_commands,
        auction_forum_catalog_pages, auction_forum_sync, auction_intake_targets,
        auction_preview_items, auction_schema_identity, auction_submissions, auctions,
        bid_proxy_authorizations, compensation_pool, credential_admin_commands, credential_keys,
        deliveries, delivery_admin_commands, delivery_send_cadence, equipment, equipment_secrets,
        forum_bidder_identities, forum_identity_history, forum_sync_admin_commands, inbound_mail,
        lot_watch_events,
        lot_watch_leadership, lot_watch_notifications, lot_watch_pending_notifications,
        lot_watch_pm_deliveries, lot_watches, lots, mail_admin_commands, mail_commands,
        mail_poll_pending_reads, mail_poll_state, mail_runtime_generation, mailbox_sweep_cursors,
        mail_scan_checkpoints,
        operations_alert_batch_items, operations_alert_batches, operations_alert_runtime_state,
        operations_alert_signals, operations_alert_webhook_deliveries, public_sessions,
        realtime_consumer_offsets, realtime_events,
        realtime_failures, realtime_streams, rules_admin_commands, rules_documents,
        runtime_config_admin_commands, scheduled_rounds, service_heartbeats,
        settlement_funding_accounts, settlement_funding_allocations, settlement_funding_commands,
        settlement_funding_entries, settlement_funding_lots, settlement_funding_receipts,
        settlement_funding_scope, settlement_funding_sources,
        settlement_internal_commands, settlement_obligation_fulfillments, settlement_obligations,
        settlement_recoveries, settlement_recovery_actions, settlement_recovery_commands,
        settlement_recovery_health, settlement_return_actions, settlement_return_cases,
        user_admin_commands, user_blacklist,
        user_game_identity_history, user_login_key_envelopes, user_mail_commands, user_renames,
        users, worlds
    TO auction_runtime;

    -- 只追加事实：触发器已拒绝 UPDATE/DELETE，权限层同时只给 INSERT。
    GRANT INSERT ON TABLE
        auction_settlements, audit_records, bid_proxy_authorization_transitions, bids,
        compensation_ledger, compensation_pool_reversals, delivery_dispositions,
        delivery_external_quarantines, operations_alert_acknowledgements,
        delivery_send_attempts, delivery_sent_bindings, delivery_sent_confirmations,
        delivery_take_attempts,
        lot_settlements, mail_template_events, runtime_switch_events,
        settlement_funding_delivery_bindings, settlement_funding_returns,
        settlement_fulfillment_dispatches,
        settlement_equipment_losses,
        settlement_legacy_bot_seller_claims, settlement_legacy_self_reconciliations,
        settlement_recovery_return_consumptions, settlement_return_commands,
        settlement_return_events, settlement_return_evidence, settlement_return_takes,
        settlement_return_verifications,
        user_blacklist_history
    TO auction_runtime;

    GRANT INSERT ON TABLE
        consignment_commands, consignment_events, consignment_snapshots,
        consignment_listing_lot_baselines, consignment_listing_auction_conflicts,
        consignment_listings, consignment_registration_secrets, consignment_replay_keys,
        consignment_lookup_work TO auction_runtime;

    GRANT UPDATE (state, price, state_version, current_snapshot_id, reason_code, updated_at, expires_at)
        ON public.consignment_listings TO auction_runtime;
    GRANT UPDATE (ciphertext, nonce, encryption_key_id, fingerprint, updated_at, retired_at)
        ON public.consignment_registration_secrets TO auction_runtime;
    GRANT UPDATE (write_current, wrapped_key, nonce, encryption_key_id, updated_at)
        ON public.consignment_replay_keys TO auction_runtime;
    GRANT UPDATE (state, attempt_count, fence, next_attempt_at, lease_owner, token, lease_until, network_deadline,
                  last_reason, completed_at, updated_at)
        ON public.consignment_lookup_work TO auction_runtime;
    GRANT UPDATE (enabled_epoch, admitting, next_allowed_at, cooldown_until, holder_work_id,
                  holder_fence, holder_token, permit_until, worker_ready_until, scan_after)
        ON public.consignment_lookup_control TO auction_runtime;

    -- 冻结证据与迁移登记只读：settlement_pools（071 冻结）、schema_migration_compatibility。
    -- （SELECT 已在上面统一授予，这里不追加任何写入权限。）

    -- 序列只允许取号，不允许 setval。
    GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA public TO auction_runtime;

    -- 默认权限：未来由本 owner 新建的对象仍只授予读取面，写入面必须由新迁移显式 GRANT。
    ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT SELECT ON TABLES TO auction_runtime;
    ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT USAGE, SELECT ON SEQUENCES TO auction_runtime;

    -- 该函数只应由迁移 owner 执行；恢复后默认 ACL 会被重置，函数自身再次收口。
    REVOKE ALL ON FUNCTION public.apply_auction_runtime_privileges() FROM PUBLIC;
END
$function$;


SELECT public.apply_auction_runtime_privileges();

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (117, 'breaking');
