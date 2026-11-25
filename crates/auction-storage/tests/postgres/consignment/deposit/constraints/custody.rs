//! 预约、实物占用、代管与交接表的命名约束违反行。

use super::support::Case;

const LISTING: &str = "20000000-0000-0000-0000-000000000001";
const RESERVATION: &str = "82000000-0000-0000-0000-000000000001";
const ASSET: &str = "85000000-0000-0000-0000-000000000001";
const OBSERVATION: &str = "84000000-0000-0000-0000-000000000001";
const ENVELOPE: &str = "83000000-0000-0000-0000-000000000001";
const ITEM: &str = "88000000-0000-0000-0000-000000000001";

fn check(table: &'static str, constraint: &'static str, sql: &'static str) -> Case {
    Case { table, constraint, state: "23514", sql }
}

fn unique(table: &'static str, constraint: &'static str, sql: &'static str) -> Case {
    Case { table, constraint, state: "23505", sql }
}

fn fk(table: &'static str, constraint: &'static str, sql: &'static str) -> Case {
    Case { table, constraint, state: "23503", sql }
}

pub fn cases() -> Vec<Case> {
    let mut cases = vec![
        fk(
            "consignment_deposit_reservations",
            "c4_res_listing_fk",
            "UPDATE public.consignment_deposit_reservations SET listing_id='deadbeef-0000-4000-8000-000000000001' WHERE id='82000000-0000-0000-0000-000000000001';",
        ),
        fk(
            "consignment_deposit_reservations",
            "c4_res_session_fk",
            "UPDATE public.consignment_deposit_reservations SET session_id='deadbeef-0000-4000-8000-000000000002' WHERE id='82000000-0000-0000-0000-000000000001';",
        ),
        check(
            "consignment_deposit_reservations",
            "c4_res_id_ck",
            "UPDATE public.consignment_deposit_reservations SET id='00000000-0000-0000-0000-000000000000' WHERE id='82000000-0000-0000-0000-000000000001';",
        ),
        check(
            "consignment_deposit_reservations",
            "c4_res_bot_uid_ck",
            "UPDATE public.consignment_deposit_reservations SET bot_uid='0' WHERE id='82000000-0000-0000-0000-000000000001';",
        ),
        check(
            "consignment_deposit_reservations",
            "c4_res_eid_ck",
            "UPDATE public.consignment_deposit_reservations SET canonical_eid='0' WHERE id='82000000-0000-0000-0000-000000000001';",
        ),
        check(
            "consignment_deposit_reservations",
            "c4_res_text_ck",
            "UPDATE public.consignment_deposit_reservations SET verified_sender='' WHERE id='82000000-0000-0000-0000-000000000001';",
        ),
        check(
            "consignment_deposit_reservations",
            "c4_res_versions_ck",
            "UPDATE public.consignment_deposit_reservations SET generation=0 WHERE id='82000000-0000-0000-0000-000000000001';",
        ),
        check(
            "consignment_deposit_reservations",
            "c4_res_digest_ck",
            "UPDATE public.consignment_deposit_reservations SET token_digest=decode('11', 'hex') WHERE id='82000000-0000-0000-0000-000000000001';",
        ),
        check(
            "consignment_deposit_reservations",
            "c4_res_time_ck",
            "UPDATE public.consignment_deposit_reservations SET expires_at=issued_at WHERE id='82000000-0000-0000-0000-000000000001';",
        ),
        check(
            "consignment_deposit_reservations",
            "c4_res_state_ck",
            "UPDATE public.consignment_deposit_reservations SET state='bogus' WHERE id='82000000-0000-0000-0000-000000000001';",
        ),
        check(
            "consignment_deposit_reservations",
            "c4_res_season_shape_ck",
            "UPDATE public.consignment_deposit_reservations SET season_window_id='8d000000-0000-0000-0000-000000000001' WHERE id='82000000-0000-0000-0000-000000000001';",
        ),
        unique(
            "consignment_deposit_reservations",
            "c4_res_token_uq",
            "INSERT INTO public.consignment_deposit_reservations
                (id, listing_id, seller_user_id, verified_sender, bot_uid, world_id,
                 canonical_eid, season_window_id, generation, service_epoch, session_id,
                 token_digest, issued_at, expires_at, state, last_command_id, retired_at, reason)
             SELECT '82000000-0000-0000-0000-000000000098', listing_id, seller_user_id,
                    verified_sender, bot_uid, world_id, canonical_eid, season_window_id, 2,
                    service_epoch, session_id, token_digest, issued_at, expires_at, 'active',
                    last_command_id, NULL, NULL
             FROM public.consignment_deposit_reservations WHERE id='82000000-0000-0000-0000-000000000001';",
        ),
        unique(
            "consignment_deposit_reservations",
            "c4_res_generation_uq",
            "INSERT INTO public.consignment_deposit_reservations
                (id, listing_id, seller_user_id, verified_sender, bot_uid, world_id,
                 canonical_eid, season_window_id, generation, service_epoch, session_id,
                 token_digest, issued_at, expires_at, state, last_command_id, retired_at, reason)
             SELECT '82000000-0000-0000-0000-000000000097', listing_id, seller_user_id,
                    verified_sender, bot_uid, world_id, canonical_eid, season_window_id,
                    generation, service_epoch, session_id, decode(repeat('5a', 32), 'hex'),
                    issued_at, expires_at, 'active', last_command_id, NULL, NULL
             FROM public.consignment_deposit_reservations WHERE id='82000000-0000-0000-0000-000000000001';",
        ),
        fk(
            "consignment_reservation_secrets",
            "c4_res_secret_identity_fk",
            "UPDATE public.consignment_reservation_secrets SET generation=999 WHERE reservation_id='82000000-0000-0000-0000-000000000001';",
        ),
        check(
            "consignment_reservation_secrets",
            "c4_res_secret_generation_ck",
            "UPDATE public.consignment_reservation_secrets SET generation=0 WHERE reservation_id='82000000-0000-0000-0000-000000000001';",
        ),
        check(
            "consignment_reservation_secrets",
            "c4_res_secret_time_ck",
            "UPDATE public.consignment_reservation_secrets SET updated_at='2025-12-31T00:00:00Z' WHERE reservation_id='82000000-0000-0000-0000-000000000001';",
        ),
        check(
            "consignment_reservation_secrets",
            "c4_res_secret_shape_ck",
            "UPDATE public.consignment_reservation_secrets SET nonce=decode('11', 'hex') WHERE reservation_id='82000000-0000-0000-0000-000000000001';",
        ),
        check(
            "consignment_custody_assets",
            "c4_asset_id_ck",
            "UPDATE public.consignment_custody_assets SET equipment_id='00000000-0000-0000-0000-000000000000' WHERE equipment_id='85000000-0000-0000-0000-000000000001';",
        ),
        check(
            "consignment_custody_assets",
            "c4_asset_eid_ck",
            "UPDATE public.consignment_custody_assets SET canonical_eid='0' WHERE equipment_id='85000000-0000-0000-0000-000000000001';",
        ),
        check(
            "consignment_custody_assets",
            "c4_asset_season_shape_ck",
            "UPDATE public.consignment_custody_assets SET season_id='frozen-season' WHERE equipment_id='85000000-0000-0000-0000-000000000001';",
        ),
        unique(
            "consignment_custody_assets",
            "c4_asset_identity_uq",
            "INSERT INTO public.consignment_custody_assets
                (equipment_id, world_id, canonical_eid, season_id, first_observation_id,
                 auction_equipment_id)
             SELECT '85000000-0000-0000-0000-000000000099', world_id, canonical_eid,
                    season_id, first_observation_id, auction_equipment_id
             FROM public.consignment_custody_assets WHERE equipment_id='85000000-0000-0000-0000-000000000001';",
        ),
        check(
            "consignment_equipment_claims",
            "c4_claim_world_ck",
            "UPDATE public.consignment_equipment_claims SET world_id='bogus' WHERE world_id='main' AND canonical_eid='42';",
        ),
        check(
            "consignment_equipment_claims",
            "c4_claim_eid_ck",
            "UPDATE public.consignment_equipment_claims SET canonical_eid='0' WHERE world_id='main' AND canonical_eid='42';",
        ),
        check(
            "consignment_equipment_claims",
            "c4_claim_revision_ck",
            "UPDATE public.consignment_equipment_claims SET revision=0 WHERE world_id='main' AND canonical_eid='42';",
        ),
        check(
            "consignment_equipment_claims",
            "c4_claim_time_ck",
            "UPDATE public.consignment_equipment_claims SET changed_at='infinity' WHERE world_id='main' AND canonical_eid='42';",
        ),
        check(
            "consignment_equipment_claims",
            "c4_claim_fact_ck",
            "UPDATE public.consignment_equipment_claims SET last_fact_id='00000000-0000-0000-0000-000000000000' WHERE world_id='main' AND canonical_eid='42';",
        ),
        check(
            "consignment_equipment_claims",
            "c4_claim_holder_ck",
            "UPDATE public.consignment_equipment_claims
             SET reservation_id='82000000-0000-0000-0000-000000000001'
             WHERE world_id='main' AND canonical_eid='42';",
        ),
        Case {
            table: "consignment_equipment_claims",
            constraint: "c4_claim_position_fk",
            state: "23503",
            sql: "UPDATE public.consignment_equipment_claims
                  SET position_id='deadbeef-0000-4000-8000-000000000004'
                  WHERE world_id='main' AND canonical_eid='42';",
        },
        check(
            "consignment_deposit_items",
            "c4_item_id_ck",
            "UPDATE public.consignment_deposit_items SET id='00000000-0000-0000-0000-000000000000' WHERE id='88000000-0000-0000-0000-000000000001';",
        ),
        check(
            "consignment_deposit_items",
            "c4_item_eid_ck",
            "UPDATE public.consignment_deposit_items SET canonical_eid='0' WHERE id='88000000-0000-0000-0000-000000000001';",
        ),
        check(
            "consignment_deposit_items",
            "c4_item_ordinal_ck",
            "UPDATE public.consignment_deposit_items SET ordinal=0 WHERE id='88000000-0000-0000-0000-000000000001';",
        ),
        check(
            "consignment_deposit_items",
            "c4_item_versions_ck",
            "UPDATE public.consignment_deposit_items SET accepted_price=0 WHERE id='88000000-0000-0000-0000-000000000001';",
        ),
        fk(
            "consignment_deposit_items",
            "c4_item_observation_fk",
            "UPDATE public.consignment_deposit_items SET observation_id='deadbeef-0000-4000-8000-000000000005' WHERE id='88000000-0000-0000-0000-000000000001';",
        ),
        fk(
            "consignment_deposit_items",
            "c4_item_decision_fk",
            "UPDATE public.consignment_deposit_items SET decision_id='deadbeef-0000-4000-8000-000000000006' WHERE id='88000000-0000-0000-0000-000000000001';",
        ),
        fk(
            "consignment_deposit_items",
            "c4_item_reservation_fk",
            "UPDATE public.consignment_deposit_items SET reservation_id='deadbeef-0000-4000-8000-000000000007' WHERE id='88000000-0000-0000-0000-000000000001';",
        ),
        check(
            "consignment_custody_positions",
            "c4_position_id_ck",
            "UPDATE public.consignment_custody_positions SET id='00000000-0000-0000-0000-000000000000' WHERE id='89000000-0000-0000-0000-000000000001';",
        ),
        check(
            "consignment_custody_positions",
            "c4_position_state_ck",
            "UPDATE public.consignment_custody_positions SET state='bogus' WHERE id='89000000-0000-0000-0000-000000000001';",
        ),
        check(
            "consignment_custody_positions",
            "c4_position_version_ck",
            "UPDATE public.consignment_custody_positions SET version=0 WHERE id='89000000-0000-0000-0000-000000000001';",
        ),
        check(
            "consignment_custody_positions",
            "c4_position_generation_ck",
            "UPDATE public.consignment_custody_positions SET current_generation=0 WHERE id='89000000-0000-0000-0000-000000000001';",
        ),
        check(
            "consignment_custody_positions",
            "c4_position_time_ck",
            "UPDATE public.consignment_custody_positions SET updated_at='infinity' WHERE id='89000000-0000-0000-0000-000000000001';",
        ),
        check(
            "consignment_custody_positions",
            "c4_position_season_shape_ck",
            "UPDATE public.consignment_custody_positions SET season_window_id='8d000000-0000-0000-0000-000000000001' WHERE id='89000000-0000-0000-0000-000000000001';",
        ),
        fk(
            "consignment_custody_positions",
            "c4_position_item_fk",
            "UPDATE public.consignment_custody_positions SET item_id='deadbeef-0000-4000-8000-00000000000a' WHERE id='89000000-0000-0000-0000-000000000001';",
        ),
        Case {
            table: "consignment_custody_positions",
            constraint: "c4_position_generation_fk",
            state: "23503",
            sql: "UPDATE public.consignment_custody_positions SET current_generation=99 WHERE id='89000000-0000-0000-0000-000000000001';",
        },
        unique(
            "consignment_custody_positions",
            "c4_position_item_uq",
            "INSERT INTO public.consignment_custody_positions
                (id, item_id, listing_id, seller_user_id, equipment_id, world_id,
                 canonical_eid, season_window_id, state, version, current_generation, updated_at)
             SELECT '89000000-0000-0000-0000-000000000099', item_id, listing_id,
                    seller_user_id, equipment_id, world_id, canonical_eid, season_window_id,
                    state, version, current_generation, updated_at
             FROM public.consignment_custody_positions WHERE id='89000000-0000-0000-0000-000000000001';",
        ),
        check(
            "consignment_disposition_generations",
            "c4_generation_purpose_ck",
            "UPDATE public.consignment_disposition_generations SET purpose='' WHERE position_id='89000000-0000-0000-0000-000000000001' AND generation=1;",
        ),
        check(
            "consignment_disposition_generations",
            "c4_generation_number_ck",
            "UPDATE public.consignment_disposition_generations SET predecessor=1 WHERE position_id='89000000-0000-0000-0000-000000000001' AND generation=1;",
        ),
        check(
            "consignment_disposition_generations",
            "c4_generation_epoch_ck",
            "UPDATE public.consignment_disposition_generations SET service_epoch=0 WHERE position_id='89000000-0000-0000-0000-000000000001' AND generation=1;",
        ),
        check(
            "consignment_disposition_generations",
            "c4_generation_blacklist_ck",
            "UPDATE public.consignment_disposition_generations SET blacklist_baseline_ids=ARRAY[NULL]::uuid[] WHERE position_id='89000000-0000-0000-0000-000000000001' AND generation=1;",
        ),
        check(
            "consignment_disposition_generations",
            "c4_generation_time_ck",
            "UPDATE public.consignment_disposition_generations SET created_at='infinity' WHERE position_id='89000000-0000-0000-0000-000000000001' AND generation=1;",
        ),
        fk(
            "consignment_disposition_generations",
            "c4_generation_season_fk",
            "UPDATE public.consignment_disposition_generations SET season_window_id='8d000000-0000-0000-0000-000000000001' WHERE position_id='89000000-0000-0000-0000-000000000001' AND generation=1;",
        ),
        Case {
            table: "consignment_disposition_generations",
            constraint: "c4_generation_proof_fk",
            state: "23503",
            sql: "UPDATE public.consignment_disposition_generations SET proof_id='deadbeef-0000-4000-8000-00000000000b' WHERE position_id='89000000-0000-0000-0000-000000000001' AND generation=1;",
        },
        check(
            "consignment_custody_events",
            "c4_custody_event_kind_ck",
            "UPDATE public.consignment_custody_events SET event_kind='bogus' WHERE position_id='89000000-0000-0000-0000-000000000001' AND version=2;",
        ),
        check(
            "consignment_custody_events",
            "c4_custody_event_state_ck",
            "UPDATE public.consignment_custody_events SET to_state='bogus' WHERE position_id='89000000-0000-0000-0000-000000000001' AND version=2;",
        ),
        check(
            "consignment_custody_events",
            "c4_custody_event_version_ck",
            "UPDATE public.consignment_custody_events SET version=0 WHERE position_id='89000000-0000-0000-0000-000000000001' AND version=2;",
        ),
        check(
            "consignment_custody_events",
            "c4_custody_event_listing_version_ck",
            "UPDATE public.consignment_custody_events SET listing_version=0 WHERE position_id='89000000-0000-0000-0000-000000000001' AND version=2;",
        ),
        check(
            "consignment_custody_events",
            "c4_custody_event_proof_ck",
            "UPDATE public.consignment_custody_events SET proof_id=NULL WHERE position_id='89000000-0000-0000-0000-000000000001' AND version=2;",
        ),
        check(
            "consignment_custody_events",
            "c4_custody_event_time_ck",
            "UPDATE public.consignment_custody_events SET occurred_at='infinity' WHERE position_id='89000000-0000-0000-0000-000000000001' AND version=2;",
        ),
        check(
            "consignment_equipment_handoffs",
            "c4_handoff_edge_id_ck",
            "UPDATE public.consignment_equipment_handoffs SET id='00000000-0000-0000-0000-000000000000' WHERE id='a3000000-0000-4000-8000-000000000001';",
        ),
        check(
            "consignment_equipment_handoffs",
            "c4_handoff_edge_kind_ck",
            "UPDATE public.consignment_equipment_handoffs SET kind='bogus' WHERE id='a3000000-0000-4000-8000-000000000001';",
        ),
        // 000136 起 consignment_delivered 合法（挂在 position 上的交接边），原 phase4 拒绝用例随之移除。
        check(
            "consignment_equipment_handoffs",
            "c4_handoff_edge_source_ck",
            "UPDATE public.consignment_equipment_handoffs SET auction_lot_id=NULL, position_id=NULL WHERE id='a3000000-0000-4000-8000-000000000001';",
        ),
        check(
            "consignment_equipment_handoffs",
            "c4_handoff_edge_proof_ck",
            "UPDATE public.consignment_equipment_handoffs SET proof_ref='' WHERE id='a3000000-0000-4000-8000-000000000001';",
        ),
        check(
            "consignment_equipment_handoffs",
            "c4_handoff_edge_time_ck",
            "UPDATE public.consignment_equipment_handoffs SET occurred_at='infinity' WHERE id='a3000000-0000-4000-8000-000000000001';",
        ),
        fk(
            "consignment_equipment_handoffs",
            "c4_handoff_edge_asset_fk",
            "UPDATE public.consignment_equipment_handoffs SET equipment_id='deadbeef-0000-4000-8000-00000000000d' WHERE id='a3000000-0000-4000-8000-000000000001';",
        ),
        fk(
            "consignment_equipment_handoffs",
            "c4_handoff_edge_predecessor_fk",
            "UPDATE public.consignment_equipment_handoffs SET predecessor_id='deadbeef-0000-4000-8000-00000000000e' WHERE id='a3000000-0000-4000-8000-000000000001';",
        ),
        fk(
            "consignment_equipment_handoffs",
            "c4_handoff_edge_from_user_fk",
            "UPDATE public.consignment_equipment_handoffs SET from_user_id='deadbeef-0000-4000-8000-00000000000f' WHERE id='a3000000-0000-4000-8000-000000000001';",
        ),
        fk(
            "consignment_equipment_handoffs",
            "c4_handoff_edge_to_user_fk",
            "UPDATE public.consignment_equipment_handoffs SET to_user_id='deadbeef-0000-4000-8000-000000000010' WHERE id='a3000000-0000-4000-8000-000000000001';",
        ),
        fk(
            "consignment_equipment_handoffs",
            "c4_handoff_edge_lot_fk",
            "UPDATE public.consignment_equipment_handoffs SET auction_lot_id='deadbeef-0000-4000-8000-000000000011', position_id=NULL WHERE id='a3000000-0000-4000-8000-000000000001';",
        ),
        fk(
            "consignment_equipment_handoffs",
            "c4_handoff_edge_position_fk",
            "UPDATE public.consignment_equipment_handoffs SET position_id='deadbeef-0000-4000-8000-000000000012', auction_lot_id=NULL WHERE id='a3000000-0000-4000-8000-000000000001';",
        ),
        check(
            "consignment_season_notices",
            "c4_notice_id_ck",
            "UPDATE public.consignment_season_notices SET id='00000000-0000-0000-0000-000000000000' WHERE id='a4000000-0000-4000-8000-000000000001';",
        ),
        check(
            "consignment_season_notices",
            "c4_notice_kind_ck",
            "UPDATE public.consignment_season_notices SET kind='bogus' WHERE id='a4000000-0000-4000-8000-000000000001';",
        ),
        check(
            "consignment_season_notices",
            "c4_notice_time_ck",
            "UPDATE public.consignment_season_notices SET created_at='infinity' WHERE id='a4000000-0000-4000-8000-000000000001';",
        ),
        fk(
            "consignment_season_notices",
            "c4_notice_position_fk",
            "UPDATE public.consignment_season_notices SET position_id='deadbeef-0000-4000-8000-000000000013' WHERE id='a4000000-0000-4000-8000-000000000001';",
        ),
        unique(
            "consignment_season_notices",
            "c4_notice_business_uq",
            "INSERT INTO public.consignment_season_notices
                (id, season_window_id, position_id, seller_user_id, kind, command_id, created_at)
             SELECT 'a4000000-0000-4000-8000-000000000099', season_window_id, position_id,
                    seller_user_id, kind, command_id, created_at
             FROM public.consignment_season_notices WHERE id='a4000000-0000-4000-8000-000000000001';",
        ),
    ];
    let eid = format!(
        "ALTER TABLE public.consignment_deposit_reservations DISABLE TRIGGER USER;
         INSERT INTO public.consignment_deposit_reservations
            (id, listing_id, seller_user_id, verified_sender, bot_uid, world_id,
             canonical_eid, season_window_id, generation, service_epoch, session_id,
             token_digest, issued_at, expires_at, state, last_command_id, retired_at, reason)
         SELECT '82000000-0000-0000-0000-000000000096', listing_id, seller_user_id,
                verified_sender, bot_uid, world_id, canonical_eid, season_window_id, 2,
                service_epoch, session_id, decode(repeat('5b', 32), 'hex'), issued_at,
                expires_at, 'active', last_command_id, NULL, NULL
         FROM public.consignment_deposit_reservations WHERE id='{RESERVATION}';
         INSERT INTO public.consignment_deposit_items
            (id, bot_uid, world_id, external_mid, decision_id, ordinal, canonical_eid,
             reservation_id, listing_id, seller_user_id, equipment_id,
             accepted_listing_version, accepted_price, credential_ref, observation_id)
         SELECT '88000000-0000-0000-0000-000000000097', bot_uid, world_id, external_mid,
                decision_id, 2, canonical_eid, '82000000-0000-0000-0000-000000000096',
                listing_id, seller_user_id, equipment_id, accepted_listing_version,
                accepted_price, credential_ref, observation_id
         FROM public.consignment_deposit_items WHERE id='{ITEM}';"
    );
    cases.push(unique(
        "consignment_deposit_items",
        "c4_item_eid_uq",
        Box::leak(eid.into_boxed_str()),
    ));
    let reservation = format!(
        "INSERT INTO public.mail_envelope_observations
            (id, bot_uid, world_id, external_mid, remote_observation_id, session_id,
             mailbox_key, quarantine_id, quarantine_version, observed_at, parser_version,
             evidence_ref, subject_digest, body_digest, manifest_digest, sender, recipient,
             returnable, takeable, credits, cod, raw_count, manifest, derived_facts,
             season_window_id)
         SELECT '84000000-0000-0000-0000-000000000096', bot_uid, world_id, '559', 'obs-559',
                session_id, mailbox_key, NULL, NULL, observed_at, parser_version, evidence_ref,
                subject_digest, body_digest, manifest_digest, sender, recipient, returnable,
                takeable, credits, cod, raw_count, manifest, derived_facts, NULL
         FROM public.mail_envelope_observations WHERE id='{OBSERVATION}';
         ALTER TABLE public.mail_initial_decisions DISABLE TRIGGER USER;
         ALTER TABLE public.mail_final_dispositions DISABLE TRIGGER USER;
         INSERT INTO public.mail_initial_decisions
            (bot_uid, world_id, external_mid, id, observation_id, command_id, kind,
             quarantine_id, decided_at)
         VALUES ('12345', 'main', '559', '86000000-0000-0000-0000-000000000097',
                 '84000000-0000-0000-0000-000000000096',
                 '41000000-0000-5000-8000-000000000002', 'accepted', NULL,
                 '2026-01-01T03:00:00Z');
         INSERT INTO public.mail_final_dispositions
            (bot_uid, world_id, external_mid, id, kind, source_observation_id, command_id,
             decided_at)
         VALUES ('12345', 'main', '559', '87000000-0000-0000-0000-000000000097',
                 'accepted', '84000000-0000-0000-0000-000000000096',
                 '41000000-0000-5000-8000-000000000002', '2026-01-01T03:00:00Z');
         INSERT INTO public.consignment_deposit_items
            (id, bot_uid, world_id, external_mid, decision_id, ordinal, canonical_eid,
             reservation_id, listing_id, seller_user_id, equipment_id,
             accepted_listing_version, accepted_price, credential_ref, observation_id)
         VALUES ('88000000-0000-0000-0000-000000000096', '12345', 'main', '559',
                 '87000000-0000-0000-0000-000000000097', 1, '42', '{RESERVATION}',
                 '{LISTING}', '10000000-0000-0000-0000-000000000001', '{ASSET}',
                 4, 101, '{ENVELOPE}', '84000000-0000-0000-0000-000000000096');"
    );
    cases.push(unique(
        "consignment_deposit_items",
        "c4_item_reservation_uq",
        Box::leak(reservation.into_boxed_str()),
    ));
    cases
}

pub const EXCEPTIONS: &[(&str, &str)] = &[
    ("c4_res_id_generation_seller_uq", "元组含主键 id；PK 先拦截"),
    ("c4_res_id_world_eid_uq", "元组含主键 id；PK 先拦截"),
    ("c4_res_id_world_eid_seller_uq", "元组含主键 id；PK 先拦截"),
    ("c4_res_world_ck", "world_id 非 main/isekai 时 c4_res_season_shape_ck 同时为假并先失败"),
    ("c4_asset_identity_composite_uq", "元组含 equipment_id 主键；PK 先拦截"),
    ("c4_asset_equipment_identity_uq", "元组含 equipment_id 主键；PK 先拦截"),
    ("c4_asset_world_ck", "world_id 非 main/isekai 时 c4_asset_season_shape_ck 同时为假并先失败"),
    (
        "c4_position_world_ck",
        "world_id 非 main/isekai 时 c4_position_season_shape_ck 同时为假并先失败",
    ),
    (
        "c4_position_listing_fk",
        "与 c4_position_item_fk 共用 listing_id/seller/world/eid，违反含卖家的复合 listing 外键必先违反 item 外键",
    ),
    (
        "c4_position_asset_fk",
        "与 c4_position_item_fk 共用 equipment_id/world/eid，违反前者必先违反后者",
    ),
    ("c4_position_id_world_eid_uq", "元组含主键 id；PK 先拦截"),
    ("c4_position_id_seller_uq", "元组含主键 id；PK 先拦截"),
    ("c4_position_season_id_uq", "元组含主键 id；PK 先拦截"),
    (
        "c4_generation_successor_uq",
        "不可达：c4_guard_generation 强制 generation = predecessor + 1；重复 predecessor 的两行必同 generation，先违反主键 (position_id,generation)",
    ),
    ("c4_item_position_identity_uq", "元组含主键 id；PK 先拦截"),
];
