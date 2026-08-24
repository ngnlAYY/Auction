-- 该脚本只由 legacy_v28 owner 在外部预检、SQLx advisory lock 和 SERIALIZABLE 事务内执行。
SET LOCAL statement_timeout = '60s';
SET LOCAL lock_timeout = '3s';

LOCK TABLE
    public.schema_migrations,
    public.auctions,
    public.lots,
    public.bids,
    public.bid_proxy_authorizations,
    public.bid_proxy_authorization_transitions,
    public.realtime_events,
    public.users,
    public.forum_bidder_identities
IN ACCESS EXCLUSIVE MODE;

DO $$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM pg_catalog.pg_stat_activity
        WHERE datname = current_database()
          AND pid <> pg_backend_pid()
    ) THEN
        RAISE EXCEPTION 'legacy v28 bridge detected another database session after locking'
            USING ERRCODE = '55000';
    END IF;
    IF to_regclass('public._sqlx_migrations') IS NOT NULL THEN
        RAISE EXCEPTION 'legacy v28 bridge refuses an existing SQLx migration history'
            USING ERRCODE = '55000';
    END IF;
    IF (SELECT count(*) FROM public.schema_migrations) <> 1
       OR NOT EXISTS (
           SELECT 1 FROM public.schema_migrations
           WHERE version = 28 AND dirty IS FALSE
       ) THEN
        RAISE EXCEPTION 'legacy v28 migration marker changed after preflight'
            USING ERRCODE = '55000';
    END IF;
END
$$;

-- Rust 生命周期聚合保留若干旧版只存在于 policy_snapshot 的快速读取列。预览前这些时间列
-- 必须继续为空，否则 Auction::open 会把已设置结束时间判为非法重复开放。
ALTER TABLE public.auctions
    ADD COLUMN title text,
    ADD COLUMN default_start_price bigint,
    ADD COLUMN soft_close_window_seconds bigint,
    ADD COLUMN soft_close_extension_seconds bigint,
    ADD COLUMN cancel_cutoff_seconds bigint,
    ADD COLUMN proxy_slow_interval_seconds bigint,
    ADD COLUMN scheduled_end_at timestamptz,
    ADD COLUMN effective_end_at timestamptz;

UPDATE public.auctions AS auction
SET title = COALESCE(
        (
            SELECT NULLIF(btrim(sync.title), '')
            FROM public.auction_forum_sync AS sync
            WHERE sync.auction_id = auction.id
        ),
        format('[Auction] HV#%s %s', auction.sequence, auction.world_id)
    ),
    default_start_price = (auction.policy_snapshot ->> 'start_price')::bigint,
    soft_close_window_seconds =
        (auction.policy_snapshot ->> 'soft_close_window_seconds')::bigint,
    soft_close_extension_seconds =
        (auction.policy_snapshot ->> 'soft_close_extension_seconds')::bigint,
    cancel_cutoff_seconds = 86400,
    proxy_slow_interval_seconds =
        (auction.policy_snapshot ->> 'proxy_slow_interval_seconds')::bigint,
    scheduled_end_at = CASE
        WHEN auction.status IN ('live', 'final', 'settling', 'settled')
            THEN (auction.policy_snapshot ->> 'scheduled_end_at')::timestamptz
        ELSE NULL
    END,
    effective_end_at = CASE
        WHEN auction.status IN ('live', 'final', 'settling', 'settled') THEN GREATEST(
            (auction.policy_snapshot ->> 'scheduled_end_at')::timestamptz,
            COALESCE(
                (
                    SELECT max(lot.effective_end_at)
                    FROM public.lots AS lot
                    WHERE lot.auction_id = auction.id
                ),
                (auction.policy_snapshot ->> 'scheduled_end_at')::timestamptz
            )
        )
        ELSE NULL
    END;

ALTER TABLE public.auctions
    ALTER COLUMN title SET NOT NULL,
    ALTER COLUMN default_start_price SET NOT NULL,
    ALTER COLUMN default_start_price SET DEFAULT 50000,
    ALTER COLUMN soft_close_window_seconds SET NOT NULL,
    ALTER COLUMN soft_close_window_seconds SET DEFAULT 900,
    ALTER COLUMN soft_close_extension_seconds SET NOT NULL,
    ALTER COLUMN soft_close_extension_seconds SET DEFAULT 900,
    ALTER COLUMN cancel_cutoff_seconds SET NOT NULL,
    ALTER COLUMN cancel_cutoff_seconds SET DEFAULT 86400,
    ALTER COLUMN proxy_slow_interval_seconds SET NOT NULL,
    ALTER COLUMN proxy_slow_interval_seconds SET DEFAULT 600,
    ADD CONSTRAINT auctions_legacy_bridge_title_check
        CHECK (title = btrim(title) AND title <> ''),
    ADD CONSTRAINT auctions_legacy_bridge_default_start_price_check
        CHECK (default_start_price > 0),
    ADD CONSTRAINT auctions_legacy_bridge_soft_close_window_check
        CHECK (soft_close_window_seconds >= 0),
    ADD CONSTRAINT auctions_legacy_bridge_soft_close_extension_check
        CHECK (soft_close_extension_seconds > 0),
    ADD CONSTRAINT auctions_legacy_bridge_cancel_cutoff_check
        CHECK (cancel_cutoff_seconds >= 0),
    ADD CONSTRAINT auctions_legacy_bridge_proxy_slow_interval_check
        CHECK (proxy_slow_interval_seconds > 0),
    ADD CONSTRAINT auctions_legacy_bridge_effective_end_check
        CHECK (effective_end_at IS NULL OR (
            scheduled_end_at IS NOT NULL AND effective_end_at >= scheduled_end_at
        ));

-- v28 用 `current_price=start_price` 表示尚无领先者，Rust 则用 NULL；先移除只适用于旧投影
-- 的约束再转换。`leader_proxy_last` 可由最后一条追加 bid fact 精确恢复。
ALTER TABLE public.lots
    DROP CONSTRAINT lots_bidding_projection_v25_check,
    ALTER COLUMN current_price DROP NOT NULL,
    ADD COLUMN leader_proxy_last boolean NOT NULL DEFAULT false,
    ADD COLUMN material boolean NOT NULL DEFAULT false;

UPDATE public.lots AS lot
SET current_price = CASE
        WHEN lot.leader_user_id IS NULL THEN NULL
        ELSE lot.current_price
    END,
    leader_proxy_last = CASE
        WHEN lot.leader_user_id IS NULL THEN false
        ELSE (
            SELECT bid.mechanism = 'proxy'
            FROM public.bids AS bid
            WHERE bid.lot_id = lot.id
            ORDER BY bid.accepted_state_version DESC, bid.fact_index DESC, bid.id DESC
            LIMIT 1
        )
    END;

-- v25 之后每条原始 bid event 都以 FK 绑定 bid fact，并保存当次软截止结果；禁止用 lot 最终
-- 截止时间覆盖历史事实。
ALTER TABLE public.bids ADD COLUMN effective_end_at timestamptz;

-- 追加式事实触发器会正确拒绝普通 UPDATE；接管事务只在预检确认触发器为默认启用状态后，
-- 于锁内暂时关闭它完成确定性列回填，并在设置 NOT NULL 前恢复保护。
ALTER TABLE public.bids DISABLE TRIGGER bids_append_only;

UPDATE public.bids AS bid
SET effective_end_at = evidence.effective_end_at
FROM (
    SELECT
        event.origin_bid_id AS bid_id,
        event.origin_fact_index AS fact_index,
        (event.public_payload ->> 'effective_end')::timestamptz AS effective_end_at
    FROM public.realtime_events AS event
    WHERE event.kind = 'lot.bid_accepted'
      AND event.replay_of_event_id IS NULL
) AS evidence
WHERE evidence.bid_id = bid.id
  AND evidence.fact_index = bid.fact_index;

ALTER TABLE public.bids ENABLE TRIGGER bids_append_only;
ALTER TABLE public.bids ALTER COLUMN effective_end_at SET NOT NULL;

-- v28 已有 replay-safe 的原始事实唯一索引，但列顺序与 Rust v7 不同；保留旧索引并补建
-- 当前索引，然后把 v7 记录为等价基线，避免执行只适用于 fresh schema 的 DROP CONSTRAINT。
CREATE UNIQUE INDEX realtime_events_origin_bid_fact_idx
    ON public.realtime_events (origin_bid_id, origin_fact_index)
    WHERE replay_of_event_id IS NULL
      AND origin_bid_id IS NOT NULL
      AND origin_fact_index IS NOT NULL;

-- Rust 将旧版 policy/账户兼容投影保留为独立历史表；v28 在 bidding semantic cutover 时已
-- 删除它们，因此接管时建立空表，权威 quota 仍来自 authorization.usage_number。
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

-- 旧 Final 把 slow→fast 记录为 transition_kind=final_fast，但可变 authorization.status 仍是
-- active。历史 transition 不可修改；bridge 只升级可变投影，并让约束同时接受旧证据形状与
-- Rust 后续写入的显式 final_fast 状态。
DO $$
DECLARE
    constraint_name text;
BEGIN
    FOR constraint_name IN
        SELECT constraint.conname
        FROM pg_catalog.pg_constraint AS constraint
        WHERE constraint.conrelid = 'public.bid_proxy_authorizations'::regclass
          AND constraint.contype = 'c'
          AND pg_get_constraintdef(constraint.oid) ~ '\mstatus\M'
    LOOP
        EXECUTE format(
            'ALTER TABLE public.bid_proxy_authorizations DROP CONSTRAINT %I',
            constraint_name
        );
    END LOOP;

    FOR constraint_name IN
        SELECT constraint.conname
        FROM pg_catalog.pg_constraint AS constraint
        WHERE constraint.conrelid = 'public.bid_proxy_authorization_transitions'::regclass
          AND constraint.contype = 'c'
          AND pg_get_constraintdef(constraint.oid) ~
              '(transition_kind|old_status|new_status)'
    LOOP
        EXECUTE format(
            'ALTER TABLE public.bid_proxy_authorization_transitions DROP CONSTRAINT %I',
            constraint_name
        );
    END LOOP;
END
$$;

ALTER TABLE public.bid_proxy_authorizations
    ADD CONSTRAINT bid_proxy_authorizations_legacy_bridge_status_check
        CHECK (status IN ('active', 'final_fast', 'exhausted', 'frozen', 'closed')),
    ADD CONSTRAINT bid_proxy_authorizations_legacy_bridge_shape_check CHECK (
        (status IN ('active', 'final_fast') AND terminal_reason IS NULL)
        OR (status = 'exhausted' AND terminal_reason IN ('strictly_defeated', 'ranked_below'))
        OR (status = 'frozen' AND terminal_reason = 'eligibility_lost')
        OR (status = 'closed' AND terminal_reason = 'competition_closed')
    );

ALTER TABLE public.bid_proxy_authorization_transitions
    ADD CONSTRAINT bid_proxy_authorization_transitions_legacy_bridge_kind_check
        CHECK (transition_kind IN (
            'created', 'lowered', 'final_fast', 'exhausted', 'frozen', 'closed'
        )),
    ADD CONSTRAINT bid_proxy_authorization_transitions_legacy_bridge_old_status_check
        CHECK (old_status IS NULL OR old_status IN (
            'active', 'final_fast', 'exhausted', 'frozen', 'closed'
        )),
    ADD CONSTRAINT bid_proxy_authorization_transitions_legacy_bridge_new_status_check
        CHECK (new_status IN ('active', 'final_fast', 'exhausted', 'frozen', 'closed')),
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
            AND new_status IN ('active', 'final_fast')
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

UPDATE public.bid_proxy_authorizations AS authorization
SET status = 'final_fast',
    updated_at = GREATEST(authorization.updated_at, transition.created_at)
FROM (
    SELECT history.authorization_id, max(history.created_at) AS created_at
    FROM public.bid_proxy_authorization_transitions AS history
    WHERE history.transition_kind = 'final_fast'
    GROUP BY history.authorization_id
) AS transition
WHERE transition.authorization_id = authorization.authorization_id
  AND authorization.status = 'active';

DROP INDEX public.bid_proxy_authorizations_one_active_lot_idx;
CREATE UNIQUE INDEX bid_proxy_authorizations_one_active_lot_idx
    ON public.bid_proxy_authorizations (lot_id)
    WHERE status IN ('active', 'final_fast');

DROP INDEX public.bid_proxy_authorizations_lot_rank_idx;
CREATE INDEX bid_proxy_authorizations_lot_rank_idx
    ON public.bid_proxy_authorizations (
        lot_id,
        maximum_amount DESC,
        authorization_sequence,
        authorization_id
    )
    WHERE status IN ('active', 'final_fast');

-- 保留 PG18 casefold 唯一索引作为更强的 Unicode 证据，同时补建 Rust lower() 查询可直接使用
-- 的索引。重命名只改变目录名称，不改变唯一性或历史数据。
ALTER INDEX public.users_username_case_insensitive_unique
    RENAME TO users_username_pg18_casefold_unique;
CREATE UNIQUE INDEX users_username_case_insensitive_unique
    ON public.users (lower(username));

ALTER INDEX public.users_game_player_id_case_insensitive_unique
    RENAME TO users_game_player_id_pg18_casefold_unique;
CREATE UNIQUE INDEX users_game_player_id_case_insensitive_unique
    ON public.users (lower(game_player_id))
    WHERE game_player_id IS NOT NULL;

ALTER INDEX public.forum_bidder_identities_casefold_unique
    RENAME TO forum_bidder_identities_pg18_casefold_unique;
CREATE UNIQUE INDEX forum_bidder_identities_casefold_unique
    ON public.forum_bidder_identities (world_id, provider, lower(external_account_id));

CREATE VIEW public.bid_proxies AS
SELECT
    authorization.lot_id,
    authorization.user_id,
    CASE authorization.mode WHEN 'fast' THEN 'instant' ELSE 'slow' END AS mode,
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
