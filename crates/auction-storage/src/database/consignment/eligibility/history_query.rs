//! 拍卖历史证据的只读提取；SQL 核对事实关联，由领域决定资格。

use super::predicates::listing_sql;

/// 用于历史证据读取及其同参数执行计划。
pub(crate) const HISTORY_SQL: &str = concat!(
    "WITH identity_equipment AS MATERIALIZED (
        SELECT e.id, e.world_id FROM public.equipment e WHERE ", listing_sql!(equipment_alias, "$1", "$2"),
    "
    ), alias_evidence AS MATERIALIZED (
        SELECT e.id, e.world_id,
               EXISTS (
                   SELECT 1 FROM identity_equipment other
                   JOIN public.equipment_secrets other_secret ON other_secret.equipment_id = other.id
                   WHERE other.id <> e.id
                     AND secret.encryption_key_id = other_secret.encryption_key_id
                     AND other_secret.fingerprint COLLATE \"C\" ~ '^[0-9a-f]{64}$'
                     AND secret.fingerprint COLLATE \"C\" ~ '^[0-9a-f]{64}$'
                     AND secret.fingerprint <> other_secret.fingerprint
               ) AS conflicting_original,
               (
                   secret.fingerprint COLLATE \"C\" ~ '^[0-9a-f]{64}$'
                   AND NOT EXISTS (
                       SELECT 1 FROM identity_equipment other
                       LEFT JOIN public.equipment_secrets other_secret
                           ON other_secret.equipment_id = other.id
                       WHERE other.id <> e.id AND
                         ROW(secret.encryption_key_id, secret.fingerprint)
                           IS DISTINCT FROM ROW(other_secret.encryption_key_id, other_secret.fingerprint)
                   )
               ) IS TRUE AS comparable_original
        FROM identity_equipment e
        LEFT JOIN public.equipment_secrets secret ON secret.equipment_id = e.id
    ), lot_evidence AS MATERIALIZED (
        SELECT e.id AS equipment_id, e.conflicting_original, e.comparable_original,
               a.id AS lot_id, a.status AS lot_status, a.leader_user_id AS winner_user_id,
               a.created_at AS lot_created_at, a.submission_id,
               au.status AS auction_status, sub.seller AS seller_name, sub.received_at AS submitted_at,
               o.obligation_kind, o.recipient_user_id AS stable_recipient, o.recipient_game_identity,
               d.recipient AS delivery_recipient, f.proof_observed_at AS collected_at,
               (a.status = 'sold' AND recovery.stage = 'completed'
                   AND recovery.resolution IN ('reclaimed', 'equipment_recovered')
                   AND recovery.relisted_lot_id IS NOT NULL) IS TRUE AS completed_relist,
               (o.world_id = e.world_id AND au.world_id = e.world_id
                   AND o.equipment_id = e.id AND o.submission_id IS NOT DISTINCT FROM a.submission_id
                   AND d.origin_kind = 'settlement' AND d.origin_id = o.id::text
                   AND d.payload->>'world' = e.world_id AND d.payload->>'operation' = 'send'
                   AND d.payload->'equipment_secret_refs' = jsonb_build_array(
                      jsonb_build_object('kind', 'auction_equipment_key', 'id', e.id::text)
                   )) IS TRUE AS matches_original,
               (f.execution_state = 'satisfied') IS TRUE AS satisfied,
               (f.delivery_operation = 'send' AND f.dispatch_world = e.world_id
                   AND d.status = 'succeeded') IS TRUE AS delivery_succeeded,
               (f.proof_kind = 'mooglemail_sent_collected'
                   AND f.proof_digest <> decode(repeat('00', 32), 'hex')
                   AND d.external_reference = 'mooglemail:' || e.world_id || ':send:' || f.proof_reference
                   AND f.proof_reference COLLATE \"C\" ~ '^[1-9][0-9]{0,63}$'
                   AND d.completed_at >= a.created_at
                   AND f.proof_observed_at >= d.completed_at
                   AND (
                       public.delivery_has_frozen_sent_proof(d.id, d.result_metadata, d.external_reference)
                       OR (
                           d.result_metadata->>'external_reference_proven' = 'true'
                           AND d.result_metadata->>'operation' = 'send'
                           AND d.result_metadata->>'proof' IN ('sent_exact_payload_v1', 'sent_recovered_reference_v1')
                       )
                   )
                   -- 原成交的领取证明已失效。收回并重列后，即使后续 lot 已交割或返还，第一版也不以其替代原证明。
                   AND recovery.resolution IS NULL
                   AND NOT EXISTS (
                       SELECT 1 FROM public.settlement_equipment_losses loss WHERE loss.delivery_id = d.id
                   )) IS TRUE AS collection_proof,
               EXISTS (
                   SELECT 1 FROM public.settlement_obligations asset
                   JOIN public.settlement_obligation_fulfillments af ON af.obligation_id = asset.id
                   LEFT JOIN public.deliveries ad ON ad.id = af.delivery_id
                   WHERE asset.auction_id = a.auction_id AND asset.lot_id = a.id
                     AND asset.obligation_kind IN (
                         'winner_equipment_cod', 'winner_equipment_handoff', 'unsold_return', 'withdrawn_return'
                     )
                     AND (af.execution_state = 'manual_reconcile' OR ad.status IN ('sending', 'manual_reconcile'))
               ) AS unknown_dispatch,
               (
                   (recovery.stage <> 'completed') IS TRUE OR EXISTS (
                       SELECT 1 FROM public.settlement_obligations asset
                       JOIN public.settlement_return_cases rc ON rc.obligation_id = asset.id
                       WHERE asset.auction_id = a.auction_id AND asset.lot_id = a.id
                         AND asset.obligation_kind IN (
                             'winner_equipment_cod', 'winner_equipment_handoff', 'unsold_return', 'withdrawn_return'
                         )
                         AND rc.status NOT IN ('resent', 'inbox_closed', 'reviewed')
                   )
               ) AS open_case
        FROM alias_evidence e
        LEFT JOIN public.lots a ON a.equipment_id = e.id
        LEFT JOIN public.auctions au ON au.id = a.auction_id
        LEFT JOIN public.auction_submissions sub ON sub.id = a.submission_id
        LEFT JOIN public.settlement_recoveries recovery ON recovery.auction_id = a.auction_id
                                                     AND recovery.lot_id = a.id
        LEFT JOIN public.settlement_obligations o
          ON o.auction_id = a.auction_id AND o.lot_id = a.id AND o.obligation_kind = CASE a.status
              WHEN 'sold' THEN CASE e.world_id WHEN 'main' THEN 'winner_equipment_cod'
                                                ELSE 'winner_equipment_handoff' END
              WHEN 'unsold' THEN 'unsold_return'
              WHEN 'withdrawn' THEN 'withdrawn_return'
          END
        LEFT JOIN public.settlement_obligation_fulfillments f ON f.obligation_id = o.id
        LEFT JOIN public.deliveries d ON d.id = f.delivery_id
    ), names AS MATERIALIZED (
        SELECT seller_name AS name FROM lot_evidence WHERE seller_name IS NOT NULL
        UNION SELECT recipient_game_identity FROM lot_evidence WHERE recipient_game_identity IS NOT NULL
        UNION SELECT delivery_recipient FROM lot_evidence
              WHERE stable_recipient IS NULL AND delivery_recipient IS NOT NULL
    ), identities AS MATERIALIZED (
        SELECT n.name, candidate.user_id FROM names n
        CROSS JOIN LATERAL (
            -- 当前名称和历史改名若共同指向多个用户，不能仅挑一条推断冻结身份。
            SELECT (array_agg(found.user_id))[1] AS user_id
            FROM (
                SELECT u.id AS user_id FROM public.users u WHERE lower(u.username) = lower(n.name)
                UNION
                SELECT r.user_id FROM public.user_renames r
                WHERE lower(r.previous_username) = lower(n.name) OR lower(r.next_username) = lower(n.name)
                UNION
                SELECT h.user_id FROM public.user_game_identity_history h
                WHERE lower(h.previous_username) = lower(n.name) OR lower(h.next_username) = lower(n.name)
                LIMIT 2
            ) found
            HAVING count(*) = 1
        ) candidate
    )
    SELECT le.equipment_id, le.conflicting_original, le.comparable_original,
           le.lot_id, le.lot_status, le.winner_user_id, le.lot_created_at,
           le.submission_id, le.auction_status, le.submitted_at, le.collected_at,
           le.obligation_kind, le.matches_original, le.satisfied, le.delivery_succeeded,
           le.unknown_dispatch, le.open_case, le.completed_relist, seller.user_id AS seller_user_id,
           -- 已绑定的原投递及领取证明沿用义务冻结的 UUID，收件旧名可能已被其他账号使用。
           COALESCE(le.stable_recipient, frozen.user_id) AS recipient_user_id,
           (le.collection_proof AND (
                le.stable_recipient IS NOT NULL
                OR lower(le.delivery_recipient) = lower(le.recipient_game_identity)
                OR delivered.user_id = frozen.user_id
            )) IS TRUE AS matching_collection
    FROM lot_evidence le
    LEFT JOIN identities seller ON seller.name = le.seller_name
    LEFT JOIN identities frozen ON frozen.name = le.recipient_game_identity
    LEFT JOIN identities delivered ON delivered.name = le.delivery_recipient"
);
