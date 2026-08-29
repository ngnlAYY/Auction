//! v28 来源 schema、策略、lot 和 realtime 事实检查。

use super::mapping::*;
use super::*;

pub(super) async fn critical_source_shape_issues(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<i64, PostgresLegacyV28Error> {
    sqlx::query_scalar(
        "WITH expected_column(table_name, column_name, udt_name) AS (
             VALUES
               ('auctions', 'id', 'uuid'),
               ('auctions', 'world_id', 'text'),
               ('auctions', 'sequence', 'int8'),
               ('auctions', 'status', 'text'),
               ('auctions', 'preview_scheduled_at', 'timestamptz'),
               ('auctions', 'scheduled_open_at', 'timestamptz'),
               ('auctions', 'policy_snapshot', 'jsonb'),
               ('auctions', 'state_version', 'int8'),
               ('lots', 'id', 'uuid'),
               ('lots', 'auction_id', 'uuid'),
               ('lots', 'status', 'text'),
               ('lots', 'start_price', 'int8'),
               ('lots', 'current_price', 'int8'),
               ('lots', 'leader_user_id', 'uuid'),
               ('lots', 'leader_since', 'timestamptz'),
               ('lots', 'effective_end_at', 'timestamptz'),
               ('lots', 'slow_authorization_id', 'uuid'),
               ('lots', 'slow_due_at', 'timestamptz'),
               ('lots', 'slow_generation', 'int8'),
               ('lots', 'last_proxy_sequence', 'int8'),
               ('bids', 'id', 'uuid'),
               ('bids', 'command_id', 'uuid'),
               ('bids', 'fact_index', 'int4'),
               ('bids', 'lot_id', 'uuid'),
               ('bids', 'bidder_user_id', 'uuid'),
               ('bids', 'amount', 'int8'),
               ('bids', 'mechanism', 'text'),
               ('bids', 'accepted_state_version', 'int8'),
               ('bid_proxy_authorizations', 'authorization_id', 'uuid'),
               ('bid_proxy_authorizations', 'lot_id', 'uuid'),
               ('bid_proxy_authorizations', 'usage_number', 'int8'),
               ('bid_proxy_authorizations', 'mode', 'text'),
               ('bid_proxy_authorizations', 'status', 'text'),
               ('bid_proxy_authorization_transitions', 'transition_kind', 'text'),
               ('bid_proxy_authorization_transitions', 'old_status', 'text'),
               ('bid_proxy_authorization_transitions', 'new_status', 'text'),
               ('realtime_events', 'origin_bid_id', 'uuid'),
               ('realtime_events', 'origin_fact_index', 'int4'),
               ('realtime_events', 'public_payload', 'jsonb'),
               ('realtime_events', 'replay_of_event_id', 'uuid')
         ), missing_column AS (
             SELECT expected_column.column_name
             FROM expected_column
             LEFT JOIN information_schema.columns AS actual_column
               ON actual_column.table_schema = 'public'
              AND actual_column.table_name = expected_column.table_name
              AND actual_column.column_name = expected_column.column_name
              AND actual_column.udt_name = expected_column.udt_name
             WHERE actual_column.column_name IS NULL
         ), expected_constraint(table_name, constraint_name) AS (
             VALUES ('lots', 'lots_bidding_projection_v25_check')
         ), missing_constraint AS (
             SELECT expected_constraint.constraint_name
             FROM expected_constraint
             WHERE NOT EXISTS (
                 SELECT 1
                 FROM pg_catalog.pg_constraint AS actual_constraint
                 JOIN pg_catalog.pg_class AS actual_class
                   ON actual_class.oid = actual_constraint.conrelid
                 JOIN pg_catalog.pg_namespace AS actual_namespace
                   ON actual_namespace.oid = actual_class.relnamespace
                 WHERE actual_namespace.nspname = 'public'
                   AND actual_class.relname = expected_constraint.table_name
                   AND actual_constraint.conname = expected_constraint.constraint_name
             )
         ), expected_index(index_name) AS (
             VALUES
               ('bid_proxy_authorizations_one_active_lot_idx'),
               ('bid_proxy_authorizations_lot_rank_idx'),
               ('users_username_case_insensitive_unique'),
               ('users_game_player_id_case_insensitive_unique'),
               ('forum_bidder_identities_casefold_unique')
         ), missing_index AS (
             SELECT expected_index.index_name
             FROM expected_index
             WHERE to_regclass(format('public.%I', expected_index.index_name)) IS NULL
         ), expected_trigger(table_name, trigger_name) AS (
             VALUES ('bids', 'bids_append_only')
         ), missing_trigger AS (
             SELECT expected_trigger.trigger_name
             FROM expected_trigger
             WHERE NOT EXISTS (
                 SELECT 1
                 FROM pg_catalog.pg_trigger AS actual_trigger
                 JOIN pg_catalog.pg_class AS actual_class
                   ON actual_class.oid = actual_trigger.tgrelid
                 JOIN pg_catalog.pg_namespace AS actual_namespace
                   ON actual_namespace.oid = actual_class.relnamespace
                 WHERE actual_namespace.nspname = 'public'
                   AND actual_class.relname = expected_trigger.table_name
                   AND actual_trigger.tgname = expected_trigger.trigger_name
                   AND NOT actual_trigger.tgisinternal
                   AND actual_trigger.tgenabled = 'O'
             )
         )
         SELECT (
             (SELECT COUNT(*) FROM missing_column)
             + (SELECT COUNT(*) FROM missing_constraint)
             + (SELECT COUNT(*) FROM missing_index)
             + (SELECT COUNT(*) FROM missing_trigger)
         )::bigint",
    )
    .fetch_one(&mut **transaction)
    .await
    .map_err(|source| query_error("critical_source_shape", source))
}

pub(super) async fn bridge_target_object_count(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<i64, PostgresLegacyV28Error> {
    sqlx::query_scalar(
        "SELECT
             (SELECT COUNT(*)::bigint
              FROM information_schema.columns
              WHERE table_schema = 'public'
                AND (table_name, column_name) IN (
                    ('auctions', 'title'),
                    ('auctions', 'default_start_price'),
                    ('auctions', 'soft_close_window_seconds'),
                    ('auctions', 'soft_close_extension_seconds'),
                    ('auctions', 'cancel_cutoff_seconds'),
                    ('auctions', 'proxy_slow_interval_seconds'),
                    ('auctions', 'scheduled_end_at'),
                    ('auctions', 'effective_end_at'),
                    ('lots', 'leader_proxy_last'),
                    ('lots', 'material'),
                    ('bids', 'effective_end_at')
                ))
             + CASE WHEN to_regclass('public.bid_proxy_attempts') IS NULL THEN 0 ELSE 1 END
             + CASE WHEN to_regclass('public.bid_proxy_outbid_uses') IS NULL THEN 0 ELSE 1 END
             + CASE WHEN to_regclass('public.bid_proxies') IS NULL THEN 0 ELSE 1 END",
    )
    .fetch_one(&mut **transaction)
    .await
    .map_err(|source| query_error("bridge_target_objects", source))
}

pub(super) async fn policy_issue_count(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<i64, PostgresLegacyV28Error> {
    sqlx::query_scalar(
        "WITH candidate AS (
             SELECT
                 jsonb_typeof(policy_snapshot) = 'object' AS object_shape,
                 policy_snapshot ->> 'start_price' AS start_price,
                 policy_snapshot ->> 'soft_close_window_seconds' AS soft_close_window,
                 policy_snapshot ->> 'soft_close_extension_seconds' AS soft_close_extension,
                 policy_snapshot ->> 'proxy_slow_interval_seconds' AS proxy_slow_interval,
                 policy_snapshot ->> 'scheduled_end_at' AS scheduled_end_at
             FROM public.auctions
         ), validated AS (
             SELECT *,
                 COALESCE(pg_input_is_valid(start_price, 'bigint'), false) AS start_valid,
                 COALESCE(pg_input_is_valid(soft_close_window, 'bigint'), false)
                     AS window_valid,
                 COALESCE(pg_input_is_valid(soft_close_extension, 'bigint'), false)
                     AS extension_valid,
                 COALESCE(pg_input_is_valid(proxy_slow_interval, 'bigint'), false)
                     AS interval_valid,
                 COALESCE(pg_input_is_valid(scheduled_end_at, 'timestamptz'), false)
                     AS end_valid
             FROM candidate
         )
         SELECT COUNT(*)::bigint
         FROM validated
         WHERE object_shape IS DISTINCT FROM true
            OR NOT start_valid
            OR NOT window_valid
            OR NOT extension_valid
            OR NOT interval_valid
            OR NOT end_valid
            OR CASE WHEN start_valid THEN start_price::bigint <= 0 ELSE false END
            OR CASE WHEN window_valid THEN soft_close_window::bigint < 0 ELSE false END
            OR CASE WHEN extension_valid THEN soft_close_extension::bigint <= 0 ELSE false END
            OR CASE WHEN interval_valid THEN proxy_slow_interval::bigint <= 0 ELSE false END",
    )
    .fetch_one(&mut **transaction)
    .await
    .map_err(|source| query_error("policy_backfill", source))
}

pub(super) async fn bid_event_issue_count(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<i64, PostgresLegacyV28Error> {
    sqlx::query_scalar(
        "SELECT COUNT(*)::bigint
         FROM public.bids AS bid
         LEFT JOIN LATERAL (
             SELECT COUNT(*)::bigint AS event_count,
                    BOOL_AND(
                        jsonb_typeof(event.public_payload -> 'effective_end') = 'string'
                        AND pg_input_is_valid(
                            event.public_payload ->> 'effective_end', 'timestamptz')
                    ) AS valid_effective_end
             FROM public.realtime_events AS event
             WHERE event.kind = 'lot.bid_accepted'
               AND event.origin_bid_id = bid.id
               AND event.origin_fact_index = bid.fact_index
               AND event.replay_of_event_id IS NULL
         ) AS evidence ON true
         WHERE evidence.event_count <> 1
            OR evidence.valid_effective_end IS DISTINCT FROM true",
    )
    .fetch_one(&mut **transaction)
    .await
    .map_err(|source| query_error("bid_effective_end_evidence", source))
}

pub(super) async fn lot_projection_issue_count(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<i64, PostgresLegacyV28Error> {
    sqlx::query_scalar(
        "SELECT COUNT(*)::bigint
         FROM public.lots AS lot
         LEFT JOIN LATERAL (
             SELECT bid.bidder_user_id, bid.amount
             FROM public.bids AS bid
             WHERE bid.lot_id = lot.id
             ORDER BY bid.accepted_state_version DESC, bid.fact_index DESC, bid.id DESC
             LIMIT 1
         ) AS latest_bid ON true
         WHERE lot.current_price IS NULL
            OR lot.current_price < lot.start_price
            OR (
                lot.leader_user_id IS NULL
                AND (
                    lot.current_price <> lot.start_price
                    OR latest_bid.bidder_user_id IS NOT NULL
                )
            )
            OR (
                lot.leader_user_id IS NOT NULL
                AND (
                    latest_bid.bidder_user_id IS DISTINCT FROM lot.leader_user_id
                    OR latest_bid.amount IS DISTINCT FROM lot.current_price
                )
            )",
    )
    .fetch_one(&mut **transaction)
    .await
    .map_err(|source| query_error("lot_projection_evidence", source))
}
