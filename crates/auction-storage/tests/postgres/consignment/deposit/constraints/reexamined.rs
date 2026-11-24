//! 复核补测：第二套隔离/终局、isekai 挂单图与恢复的外键最小违反行。

use super::support::Case;

const USER_1: &str = "10000000-0000-0000-0000-000000000001";
const USER_2: &str = "10000000-0000-0000-0000-000000000002";
const LISTING: &str = "20000000-0000-0000-0000-000000000001";
const SESSION: &str = "81000000-0000-0000-0000-000000000001";
const ENVELOPE: &str = "83000000-0000-0000-0000-000000000001";
const OBSERVATION: &str = "84000000-0000-0000-0000-000000000001";
const ASSET: &str = "85000000-0000-0000-0000-000000000001";
const DECISION: &str = "87000000-0000-0000-0000-000000000001";
const ISSUE: &str = "42000000-0000-4000-8000-000000000001";
const DECIDE: &str = "41000000-0000-5000-8000-000000000002";
const RELEASE: &str = "9f000000-0000-4000-8000-000000000001";
const RELEASE_COMMAND: &str = "9d000000-0000-4000-8000-000000000001";
const START: &str = "2026-01-01T00:00:00Z";
const ISSUED: &str = "2026-01-01T02:00:00Z";
const ACCEPTED: &str = "2026-01-01T03:00:00Z";
const END: &str = "2026-01-02T00:00:00Z";

fn unique(table: &'static str, constraint: &'static str, sql: String) -> Case {
    Case { table, constraint, state: "23505", sql: leak(sql) }
}

fn fk(table: &'static str, constraint: &'static str, sql: String) -> Case {
    Case { table, constraint, state: "23503", sql: leak(sql) }
}

fn leak(sql: String) -> &'static str {
    Box::leak(sql.into_boxed_str())
}

fn disable_related_triggers() -> &'static str {
    "ALTER TABLE public.consignment_listings DISABLE TRIGGER USER;
     ALTER TABLE public.consignment_deposit_reservations DISABLE TRIGGER USER;
     ALTER TABLE public.mail_initial_decisions DISABLE TRIGGER USER;
     ALTER TABLE public.mail_final_dispositions DISABLE TRIGGER USER;"
}

fn main_listing(listing_id: &str, seller: &str, eid: &str) -> String {
    format!(
        "INSERT INTO public.consignment_listings
            (id, seller_user_id, world_id, external_equipment_id, mode, state, price,
             pricing_version, fee_bps, state_version, service_epoch,
             blacklist_count_at_register, blacklist_baseline_ids, created_at, updated_at,
             expires_at, registration_xact_id)
         VALUES ('{listing_id}', '{seller}', 'main', '{eid}', 'consigned', 'resolving', 101,
                 1, 500, 1, 1, 0, ARRAY[]::uuid[], '{START}', '{START}', '{END}',
                 pg_current_xact_id());"
    )
}

fn main_reservation(
    listing_id: &str,
    reservation_id: &str,
    seller: &str,
    eid: &str,
    digest: &str,
) -> String {
    format!(
        "INSERT INTO public.consignment_deposit_reservations
            (id, listing_id, seller_user_id, verified_sender, bot_uid, world_id, canonical_eid,
             season_window_id, generation, service_epoch, session_id, token_digest, issued_at,
             expires_at, state, last_command_id, retired_at, reason)
         VALUES ('{reservation_id}', '{listing_id}', '{seller}', 'FakeSellerOne', '12345',
                 'main', '{eid}', NULL, 1, 1, '{SESSION}',
                 decode(repeat('{digest}', 32), 'hex'), '{ISSUED}',
                 '2026-01-01T12:00:00Z', 'active', '{ISSUE}', NULL, NULL);"
    )
}

fn main_asset(asset_id: &str, eid: &str) -> String {
    format!(
        "INSERT INTO public.consignment_custody_assets
            (equipment_id, world_id, canonical_eid, season_id, first_observation_id,
             auction_equipment_id)
         VALUES ('{asset_id}', 'main', '{eid}', NULL, '{OBSERVATION}', NULL);"
    )
}

fn second_decision(decision_id: &str, initial_id: &str, observation_id: &str) -> String {
    let mid = "557";
    format!(
        "INSERT INTO public.mail_envelope_observations
            (id, bot_uid, world_id, external_mid, remote_observation_id, session_id,
             mailbox_key, quarantine_id, quarantine_version, observed_at, parser_version,
             evidence_ref, subject_digest, body_digest, manifest_digest, sender, recipient,
             returnable, takeable, credits, cod, raw_count, manifest, derived_facts,
             season_window_id)
         VALUES ('{observation_id}', '12345', 'main', '{mid}', 'obs-{mid}', '{SESSION}',
                 'fake-mailbox', NULL, NULL, '{ACCEPTED}', 'parser-v1', NULL,
                 decode(repeat('b1', 32), 'hex'), decode(repeat('b2', 32), 'hex'),
                 decode(repeat('b3', 32), 'hex'), 'FakeSellerOne', 'FakeBot',
                 true, true, 0, 0, 1,
                 '[{{\"ordinal\":1,\"kind\":\"equipment\",\"name\":\"Fake Gear\",
                    \"canonical_eid\":\"42\"}}]'::jsonb,
                 '{{\"subject_is_consign\":true,\"sender_identity\":\"verified\",
                   \"list_detail_consistent\":true,\"duplicate_raw_eid\":false,
                   \"canonical_alias_ambiguous\":false,\"unknown_attachment\":false,
                   \"system_rts\":false,\"legacy_auction_owner\":null,
                   \"received_minute_raw\":\"2026-01-01T03:00\",
                   \"items\":[{{\"ordinal\":1,\"world\":\"main\",\"tradeable\":true,
                               \"history_ok\":true}}]}}'::jsonb,
                 NULL);
         INSERT INTO public.mail_initial_decisions
            (bot_uid, world_id, external_mid, id, observation_id, command_id, kind,
             quarantine_id, decided_at)
         VALUES ('12345', 'main', '{mid}', '{initial_id}', '{observation_id}', '{DECIDE}',
                 'archived_empty', NULL, '{ACCEPTED}');
         INSERT INTO public.mail_final_dispositions
            (bot_uid, world_id, external_mid, id, kind, source_observation_id, command_id,
             decided_at)
         VALUES ('12345', 'main', '{mid}', '{decision_id}', 'archived_empty',
                 '{observation_id}', '{DECIDE}', '{ACCEPTED}');"
    )
}

struct ItemFixture<'a> {
    item_id: &'a str,
    decision_id: &'a str,
    observation_id: &'a str,
    mid: &'a str,
    eid: &'a str,
    reservation_id: &'a str,
    listing_id: &'a str,
    seller: &'a str,
    equipment_id: &'a str,
}

fn item(fixture: ItemFixture<'_>) -> String {
    let ItemFixture {
        item_id,
        decision_id,
        observation_id,
        mid,
        eid,
        reservation_id,
        listing_id,
        seller,
        equipment_id,
    } = fixture;
    format!(
        "INSERT INTO public.consignment_deposit_items
            (id, bot_uid, world_id, external_mid, decision_id, ordinal, canonical_eid,
             reservation_id, listing_id, seller_user_id, equipment_id,
             accepted_listing_version, accepted_price, credential_ref, observation_id)
         VALUES ('{item_id}', '12345', 'main', '{mid}', '{decision_id}', 1, '{eid}',
                 '{reservation_id}', '{listing_id}', '{seller}', '{equipment_id}', 4, 101,
                 '{ENVELOPE}', '{observation_id}');"
    )
}

fn second_quarantine_archive(
    quarantine_id: &str,
    initial_id: &str,
    decision_id: &str,
    initial_observation: &str,
    release_observation: &str,
    final_kind: &str,
) -> String {
    let mid = "560";
    format!(
        "ALTER TABLE public.mail_initial_decisions DISABLE TRIGGER USER;
         ALTER TABLE public.mail_final_dispositions DISABLE TRIGGER USER;
         INSERT INTO public.mail_envelope_observations
            (id, bot_uid, world_id, external_mid, remote_observation_id, session_id,
             mailbox_key, quarantine_id, quarantine_version, observed_at, parser_version,
             evidence_ref, subject_digest, body_digest, manifest_digest, sender, recipient,
             returnable, takeable, credits, cod, raw_count, manifest, derived_facts,
             season_window_id)
         VALUES ('{initial_observation}', '12345', 'main', '{mid}', 'obs-{mid}-a',
                 '{SESSION}', 'fake-mailbox', NULL, NULL, '{ACCEPTED}', 'parser-v1', NULL,
                 decode(repeat('c1', 32), 'hex'), decode(repeat('c2', 32), 'hex'),
                 decode(repeat('c3', 32), 'hex'), 'FakeSellerOne', 'FakeBot',
                 true, true, 0, 0, NULL, '[]'::jsonb,
                 '{{\"subject_is_consign\":true,\"sender_identity\":\"verified\",
                   \"list_detail_consistent\":true,\"duplicate_raw_eid\":false,
                   \"canonical_alias_ambiguous\":false,\"unknown_attachment\":false,
                   \"system_rts\":false,\"legacy_auction_owner\":null,
                   \"received_minute_raw\":\"2026-01-01T03:00\",\"items\":[]}}'::jsonb,
                 NULL);
         INSERT INTO public.mail_initial_decisions
            (bot_uid, world_id, external_mid, id, observation_id, command_id, kind,
             quarantine_id, decided_at)
         VALUES ('12345', 'main', '{mid}', '{initial_id}', '{initial_observation}',
                 '{DECIDE}', 'quarantined', '{quarantine_id}', '{ACCEPTED}');
         INSERT INTO public.mail_quarantines
            (id, bot_uid, world_id, external_mid, initial_decision_id, version, state,
             reread_locator, responsibility_code, created_at, updated_at)
         VALUES ('{quarantine_id}', '12345', 'main', '{mid}', '{initial_id}', 1, 'active',
                 'controlled-reread', 'identity_unproven', '{ACCEPTED}', '{ACCEPTED}');
         INSERT INTO public.mail_envelope_observations
            (id, bot_uid, world_id, external_mid, remote_observation_id, session_id,
             mailbox_key, quarantine_id, quarantine_version, observed_at, parser_version,
             evidence_ref, subject_digest, body_digest, manifest_digest, sender, recipient,
             returnable, takeable, credits, cod, raw_count, manifest, derived_facts,
             season_window_id)
         VALUES ('{release_observation}', '12345', 'main', '{mid}', 'obs-{mid}-b',
                 '{SESSION}', 'fake-mailbox', '{quarantine_id}', 1, '{ACCEPTED}',
                 'parser-v1', NULL, decode(repeat('c4', 32), 'hex'),
                 decode(repeat('c5', 32), 'hex'), decode(repeat('c6', 32), 'hex'),
                 'FakeSellerOne', 'FakeBot', true, true, 0, 0, NULL, '[]'::jsonb,
                 '{{\"subject_is_consign\":true,\"sender_identity\":\"verified\",
                   \"list_detail_consistent\":true,\"duplicate_raw_eid\":false,
                   \"canonical_alias_ambiguous\":false,\"unknown_attachment\":false,
                   \"system_rts\":false,\"legacy_auction_owner\":null,
                   \"received_minute_raw\":\"2026-01-01T03:00\",\"items\":[]}}'::jsonb,
                 NULL);
         INSERT INTO public.mail_final_dispositions
            (bot_uid, world_id, external_mid, id, kind, source_observation_id, command_id,
             decided_at)
         VALUES ('12345', 'main', '{mid}', '{decision_id}', '{final_kind}',
                 '{release_observation}', '{DECIDE}', '{ACCEPTED}');"
    )
}

fn second_release(
    kind: &str,
    command_id: &str,
    case_mail: Option<&str>,
    handoff: Option<&str>,
) -> String {
    let mid = "560";
    let case_mail = case_mail.map_or_else(|| "NULL".to_owned(), |id| format!("'{id}'"));
    let case_evidence = if case_mail == "NULL" { "NULL".to_owned() } else { case_mail.clone() };
    let handoff = handoff.map_or_else(|| "NULL".to_owned(), |id| format!("'{id}'"));
    format!(
        "INSERT INTO public.mail_quarantine_releases
            (id, bot_uid, world_id, external_mid, quarantine_id, expected_version,
             observation_id, decision_id, kind, command_id, case_return_mail_id,
             case_evidence_id, recovery_handoff_id, released_at)
         VALUES ('9f000000-0000-4000-8000-000000000002', '12345', 'main', '{mid}',
                 '91000000-0000-0000-0000-000000000002', 1,
                 '8f000000-0000-0000-0000-000000000004',
                 '9e000000-0000-4000-8000-000000000002', '{kind}', '{command_id}',
                 {case_mail}, {case_evidence}, {handoff}, '{ACCEPTED}');"
    )
}

fn isekai_listing_and_session(eid: &str) -> String {
    format!(
        "UPDATE public.credential_keys
         SET status='disabled', updated_at='{START}'
         WHERE id='80000000-0000-0000-0000-000000000001';
         INSERT INTO public.credential_keys
            (id, credential_kind, scope, label, ciphertext, nonce, encryption_key_id,
             fingerprint, status, state_version, created_at, updated_at)
         VALUES ('80000000-0000-0000-0000-000000000009', 'mail_cookie', 'isekai',
                 'fake-isekai', decode('11', 'hex'), decode(repeat('22', 12), 'hex'),
                 'fake-data-key', repeat('d', 64), 'active', 1, '{START}', '{START}');
         INSERT INTO public.mail_evidence_sessions
            (id, bot_uid, world_id, player_name, credential_id, credential_version,
             runtime_generation, observed_at, protocol_version, binding_digest)
         VALUES ('81000000-0000-0000-0000-000000000009', '12345', 'isekai', 'FakeBot',
                 '80000000-0000-0000-0000-000000000009', 1, 1, '{ACCEPTED}', 'v1',
                 decode(repeat('d1', 32), 'hex'));
         INSERT INTO public.consignment_listings
            (id, seller_user_id, world_id, external_equipment_id, mode, state, price,
             pricing_version, fee_bps, state_version, service_epoch,
             blacklist_count_at_register, blacklist_baseline_ids, created_at, updated_at,
             expires_at, registration_xact_id)
         VALUES ('20000000-0000-0000-0000-000000000009', '{USER_1}', 'isekai', '{eid}',
                 'consigned', 'resolving', 101, 1, 500, 1, 1, 0, ARRAY[]::uuid[],
                 '{START}', '{START}', '{END}', pg_current_xact_id());"
    )
}

fn isekai_reservation(season_window_id: &str) -> String {
    format!(
        "INSERT INTO public.consignment_deposit_reservations
            (id, listing_id, seller_user_id, verified_sender, bot_uid, world_id, canonical_eid,
             season_window_id, generation, service_epoch, session_id, token_digest, issued_at,
             expires_at, state, last_command_id, retired_at, reason)
         VALUES ('82000000-0000-0000-0000-000000000009',
                 '20000000-0000-0000-0000-000000000009', '{USER_1}', 'FakeSellerOne',
                 '12345', 'isekai', '43', '{season_window_id}', 1, 1,
                 '81000000-0000-0000-0000-000000000009', decode(repeat('d2', 32), 'hex'),
                 '{ISSUED}', '2026-01-01T12:00:00Z', 'active', '{ISSUE}', NULL, NULL);"
    )
}

pub fn cases() -> Vec<Case> {
    let mut cases = Vec::new();
    // 该行按设计同时违反 c4_quarantine_decision_uq：mail_initial_decisions 以 K 为
    // 主键，同一 K 只有一个首决定；复用已存在的 K 时只能引用既有的
    // initial_decision_id，而它已被已有隔离行占用。因此不存在只违反 K 唯一约束的行。
    cases.push(unique(
        "mail_quarantines",
        "c4_quarantine_key_uq",
        format!(
            "INSERT INTO public.mail_quarantines
                (id, bot_uid, world_id, external_mid, initial_decision_id, version, state,
                 reread_locator, responsibility_code, created_at, updated_at)
             VALUES ('91000000-0000-0000-0000-000000000002', '12346', 'main', '556',
                     '90000000-0000-0000-0000-000000000001', 1, 'active',
                     'second-read', 'identity_unproven', '{ACCEPTED}', '{ACCEPTED}');"
        ),
    ));
    // 该行按设计同时违反 c4_release_decision_uq：mail_final_dispositions 以 K 为
    // 主键，同一 K 只有一条终局；复用已存在的 K 时只能复制既有 decision_id，
    // 因此不存在只违反 K 唯一约束的释放行。
    cases.push(unique(
        "mail_quarantine_releases",
        "c4_release_key_uq",
        format!(
            "ALTER TABLE public.mail_initial_decisions DISABLE TRIGGER USER;
             INSERT INTO public.mail_envelope_observations
                (id, bot_uid, world_id, external_mid, remote_observation_id, session_id,
                 mailbox_key, quarantine_id, quarantine_version, observed_at, parser_version,
                 evidence_ref, subject_digest, body_digest, manifest_digest, sender, recipient,
                 returnable, takeable, credits, cod, raw_count, manifest, derived_facts,
                 season_window_id)
             VALUES ('8f000000-0000-0000-0000-000000000006', '12345', 'main', '562',
                     'obs-562', '{SESSION}', 'fake-mailbox', NULL, NULL, '{ACCEPTED}',
                     'parser-v1', NULL, decode(repeat('e4', 32), 'hex'),
                     decode(repeat('e5', 32), 'hex'), decode(repeat('e6', 32), 'hex'),
                     'FakeSellerOne', 'FakeBot', true, true, 0, 0, NULL, '[]'::jsonb,
                     '{{\"subject_is_consign\":true,\"sender_identity\":\"verified\",
                       \"list_detail_consistent\":true,\"duplicate_raw_eid\":false,
                       \"canonical_alias_ambiguous\":false,\"unknown_attachment\":false,
                       \"system_rts\":false,\"legacy_auction_owner\":null,
                       \"received_minute_raw\":\"2026-01-01T03:00\",\"items\":[]}}'::jsonb,
                     NULL);
             INSERT INTO public.mail_initial_decisions
                (bot_uid, world_id, external_mid, id, observation_id, command_id, kind,
                 quarantine_id, decided_at)
             VALUES ('12345', 'main', '562', '90000000-0000-0000-0000-000000000006',
                     '8f000000-0000-0000-0000-000000000006', '{DECIDE}', 'quarantined',
                     '91000000-0000-0000-0000-000000000006', '{ACCEPTED}');
             INSERT INTO public.mail_quarantines
                (id, bot_uid, world_id, external_mid, initial_decision_id, version, state,
                 reread_locator, responsibility_code, created_at, updated_at)
             VALUES ('91000000-0000-0000-0000-000000000006', '12345', 'main', '562',
                     '90000000-0000-0000-0000-000000000006', 1, 'active',
                     'controlled-reread', 'identity_unproven', '{ACCEPTED}', '{ACCEPTED}');
             INSERT INTO public.mail_envelope_observations
                (id, bot_uid, world_id, external_mid, remote_observation_id, session_id,
                 mailbox_key, quarantine_id, quarantine_version, observed_at, parser_version,
                 evidence_ref, subject_digest, body_digest, manifest_digest, sender, recipient,
                 returnable, takeable, credits, cod, raw_count, manifest, derived_facts,
                 season_window_id)
             VALUES ('8f000000-0000-0000-0000-000000000007', '12346', 'main', '556',
                     'obs-556-cross', '81000000-0000-0000-0000-000000000002',
                     'fake-mailbox', '91000000-0000-0000-0000-000000000006', 1,
                     '{ACCEPTED}', 'parser-v1', NULL, decode(repeat('e7', 32), 'hex'),
                     decode(repeat('e8', 32), 'hex'), decode(repeat('e9', 32), 'hex'),
                     'FakeSellerOne', 'FakeBot', true, true, 0, 0, NULL, '[]'::jsonb,
                     '{{\"subject_is_consign\":true,\"sender_identity\":\"verified\",
                       \"list_detail_consistent\":true,\"duplicate_raw_eid\":false,
                       \"canonical_alias_ambiguous\":false,\"unknown_attachment\":false,
                       \"system_rts\":false,\"legacy_auction_owner\":null,
                       \"received_minute_raw\":\"2026-01-01T03:00\",\"items\":[]}}'::jsonb,
                     NULL);
             INSERT INTO public.mail_quarantine_releases
                (id, bot_uid, world_id, external_mid, quarantine_id, expected_version,
                 observation_id, decision_id, kind, command_id, case_return_mail_id,
                 case_evidence_id, recovery_handoff_id, released_at)
             SELECT '9f000000-0000-4000-8000-000000000006', '12346', 'main', '556',
                    '91000000-0000-0000-0000-000000000006', 1,
                    '8f000000-0000-0000-0000-000000000007', decision_id, kind, '{DECIDE}',
                    case_return_mail_id, case_evidence_id, NULL, '{ACCEPTED}'
             FROM public.mail_quarantine_releases WHERE id='{RELEASE}';"
        ),
    ));
    cases.push(unique(
        "mail_quarantine_releases",
        "c4_release_quarantine_uq",
        format!(
            "ALTER TABLE public.mail_initial_decisions DISABLE TRIGGER USER;
             ALTER TABLE public.mail_final_dispositions DISABLE TRIGGER USER;
             INSERT INTO public.mail_envelope_observations
                (id, bot_uid, world_id, external_mid, remote_observation_id, session_id,
                 mailbox_key, quarantine_id, quarantine_version, observed_at, parser_version,
                 evidence_ref, subject_digest, body_digest, manifest_digest, sender, recipient,
                 returnable, takeable, credits, cod, raw_count, manifest, derived_facts,
                 season_window_id)
             VALUES ('8f000000-0000-0000-0000-000000000005', '12345', 'main', '561',
                     'obs-561', '{SESSION}', 'fake-mailbox',
                     '91000000-0000-0000-0000-000000000001', 2, '{ACCEPTED}', 'parser-v1',
                     NULL, decode(repeat('e1', 32), 'hex'), decode(repeat('e2', 32), 'hex'),
                     decode(repeat('e3', 32), 'hex'), 'FakeSellerOne', 'FakeBot',
                     true, true, 0, 0, NULL, '[]'::jsonb,
                     '{{\"subject_is_consign\":true,\"sender_identity\":\"verified\",
                       \"list_detail_consistent\":true,\"duplicate_raw_eid\":false,
                       \"canonical_alias_ambiguous\":false,\"unknown_attachment\":false,
                       \"system_rts\":false,\"legacy_auction_owner\":null,
                       \"received_minute_raw\":\"2026-01-01T03:00\",\"items\":[]}}'::jsonb,
                     NULL);
             INSERT INTO public.mail_initial_decisions
                (bot_uid, world_id, external_mid, id, observation_id, command_id, kind,
                 quarantine_id, decided_at)
             VALUES ('12345', 'main', '561', '90000000-0000-0000-0000-000000000005',
                     '8f000000-0000-0000-0000-000000000005', '{DECIDE}', 'archived_empty',
                     NULL, '{ACCEPTED}');
             INSERT INTO public.mail_final_dispositions
                (bot_uid, world_id, external_mid, id, kind, source_observation_id, command_id,
                 decided_at)
             VALUES ('12345', 'main', '561', '9e000000-0000-4000-8000-000000000005',
                     'archived_empty', '8f000000-0000-0000-0000-000000000005', '{DECIDE}',
                     '{ACCEPTED}');
             INSERT INTO public.mail_quarantine_releases
                (id, bot_uid, world_id, external_mid, quarantine_id, expected_version,
                 observation_id, decision_id, kind, command_id, case_return_mail_id,
                 case_evidence_id, recovery_handoff_id, released_at)
             VALUES ('9f000000-0000-4000-8000-000000000005', '12345', 'main', '561',
                     '91000000-0000-0000-0000-000000000001', 2,
                     '8f000000-0000-0000-0000-000000000005',
                     '9e000000-0000-4000-8000-000000000005', 'archived_empty', '{DECIDE}',
                     NULL, NULL, NULL, '{ACCEPTED}');"
        ),
    ));

    let quarantine = "91000000-0000-0000-0000-000000000002";
    let initial = "90000000-0000-0000-0000-000000000002";
    let decision = "9e000000-0000-4000-8000-000000000002";
    let observation = "8f000000-0000-0000-0000-000000000003";
    let release_observation = "8f000000-0000-0000-0000-000000000004";

    cases.push(unique(
        "mail_quarantine_releases",
        "c4_release_command_uq",
        format!(
            "{}{}",
            second_quarantine_archive(
                quarantine,
                initial,
                decision,
                observation,
                release_observation,
                "archived_empty",
            ),
            second_release("archived_empty", RELEASE_COMMAND, None, None)
        ),
    ));
    cases.push(fk(
        "mail_quarantine_releases",
        "c4_release_handoff_fk",
        format!(
            "{}{}",
            second_quarantine_archive(
                quarantine,
                initial,
                decision,
                observation,
                release_observation,
                "auction_recovery_return",
            ),
            second_release(
                "auction_recovery_return",
                DECIDE,
                None,
                Some("deadbeef-0000-4000-8000-000000000001"),
            )
        ),
    ));
    cases.push(fk(
        "mail_quarantine_releases",
        "c4_release_case_mail_fk",
        format!(
            "{}INSERT INTO public.inbound_mail
                (id, mailbox_key, external_mail_id, world_id, mail_kind, content_digest,
                 received_at, status, decision_code, decision_metadata, processed_at,
                 sender, subject)
             VALUES ('c1000000-0000-4000-8000-000000000001', 'case-missing-mailbox',
                     '777', 'main', 'delivery', decode(repeat('f1', 32), 'hex'),
                     '{ACCEPTED}', 'accepted', 'settlement_recovery_returned',
                     '{{}}'::jsonb, '{ACCEPTED}', 'MoogleMail', '[RTS] Auction equipment');
             INSERT INTO public.settlement_return_evidence
                (return_mail_id, match_state, reason_code, obligation_id, delivery_id,
                 previous_execution_state, recorded_at)
             VALUES ('c1000000-0000-4000-8000-000000000001', 'unbound',
                     'obligation_unknown', NULL, NULL, NULL, '{ACCEPTED}');
             {}",
            second_quarantine_archive(
                quarantine,
                initial,
                decision,
                observation,
                release_observation,
                "auction_return_case",
            ),
            second_release(
                "auction_return_case",
                DECIDE,
                Some("c1000000-0000-4000-8000-000000000001"),
                None,
            )
        ),
    ));

    let item_ordinal = format!(
        "{}INSERT INTO public.consignment_deposit_items
            (id, bot_uid, world_id, external_mid, decision_id, ordinal, canonical_eid,
             reservation_id, listing_id, seller_user_id, equipment_id,
             accepted_listing_version, accepted_price, credential_ref, observation_id)
         VALUES ('88000000-0000-0000-0000-000000000009', '12345', 'main', '555',
                 '{DECISION}', 1, '44', '82000000-0000-0000-0000-000000000009',
                 '20000000-0000-0000-0000-000000000009', '{USER_1}',
                 '85000000-0000-0000-0000-000000000009', 4, 101, '{ENVELOPE}',
                 '{OBSERVATION}');",
        main_listing("20000000-0000-0000-0000-000000000009", USER_1, "44")
            + &main_reservation(
                "20000000-0000-0000-0000-000000000009",
                "82000000-0000-0000-0000-000000000009",
                USER_1,
                "44",
                "62",
            )
            + &main_asset("85000000-0000-0000-0000-000000000009", "44")
    );
    cases.push(unique(
        "consignment_deposit_items",
        "c4_item_ordinal_uq",
        format!("{}{}", disable_related_triggers(), item_ordinal),
    ));

    let second_decision_sql =
        second_decision(decision, "86000000-0000-0000-0000-000000000002", observation);
    let base = disable_related_triggers().to_owned()
        + &main_listing("20000000-0000-0000-0000-000000000002", USER_2, "42")
        + &main_reservation(
            "20000000-0000-0000-0000-000000000002",
            "82000000-0000-0000-0000-000000000002",
            USER_2,
            "42",
            "63",
        )
        + &second_decision_sql;
    cases.push(fk(
        "consignment_deposit_items",
        "c4_item_listing_fk",
        base.clone()
            + &item(ItemFixture {
                item_id: "88000000-0000-0000-0000-000000000002",
                decision_id: decision,
                observation_id: observation,
                mid: "557",
                eid: "42",
                reservation_id: "82000000-0000-0000-0000-000000000002",
                listing_id: LISTING,
                seller: USER_2,
                equipment_id: ASSET,
            }),
    ));
    cases.push(fk(
        "consignment_deposit_items",
        "c4_item_asset_fk",
        base.clone()
            + &main_asset("85000000-0000-0000-0000-000000000010", "44")
            + &item(ItemFixture {
                item_id: "88000000-0000-0000-0000-000000000003",
                decision_id: decision,
                observation_id: observation,
                mid: "557",
                eid: "42",
                reservation_id: "82000000-0000-0000-0000-000000000002",
                listing_id: "20000000-0000-0000-0000-000000000002",
                seller: USER_2,
                equipment_id: "85000000-0000-0000-0000-000000000010",
            }),
    ));
    // 该行同时违反 c4_item_asset_fk：item 的复合资产外键
    // (equipment_id, world_id, canonical_eid) 一旦成立，被引用的 assets 行必然满足
    // equipment_id 主键；单列 equipment FK 是复合 FK 的推论，无法单独违反。
    cases.push(fk(
        "consignment_deposit_items",
        "c4_item_asset_equipment_fk",
        base + &item(ItemFixture {
            item_id: "88000000-0000-0000-0000-000000000004",
            decision_id: decision,
            observation_id: observation,
            mid: "557",
            eid: "42",
            reservation_id: "82000000-0000-0000-0000-000000000002",
            listing_id: "20000000-0000-0000-0000-000000000002",
            seller: USER_2,
            equipment_id: "deadbeef-0000-4000-8000-000000000002",
        }),
    ));

    // 仅目标 FK 违反：只隔开夹具未写 registered 版本事件导致的
    // consignment_listing_version_event，保留挂单表的其余触发器。
    cases.push(fk(
        "consignment_deposit_reservations",
        "c4_res_season_fk",
        "ALTER TABLE public.consignment_listings
             DISABLE TRIGGER consignment_listing_version_event;"
            .to_owned()
            + &isekai_listing_and_session("43")
            + &isekai_reservation("deadbeef-0000-4000-8000-000000000003"),
    ));

    let isekai_position = format!(
        "{}{}{}INSERT INTO public.mail_envelope_observations
            (id, bot_uid, world_id, external_mid, remote_observation_id, session_id,
             mailbox_key, quarantine_id, quarantine_version, observed_at, parser_version,
             evidence_ref, subject_digest, body_digest, manifest_digest, sender, recipient,
             returnable, takeable, credits, cod, raw_count, manifest, derived_facts,
             season_window_id)
         VALUES ('84000000-0000-0000-0000-000000000009', '12345', 'isekai', '570',
                 'obs-isekai-570', '81000000-0000-0000-0000-000000000009', 'fake-mailbox',
                 NULL, NULL, '{ACCEPTED}', 'parser-v1', NULL,
                 decode(repeat('d3', 32), 'hex'), decode(repeat('d4', 32), 'hex'),
                 decode(repeat('d5', 32), 'hex'), 'FakeSellerOne', 'FakeBot',
                 true, true, 0, 0, 1,
                 '[{{\"ordinal\":1,\"kind\":\"equipment\",\"name\":\"Fake Gear\",
                    \"canonical_eid\":\"43\"}}]'::jsonb,
                 '{{\"subject_is_consign\":true,\"sender_identity\":\"verified\",
                   \"list_detail_consistent\":true,\"duplicate_raw_eid\":false,
                   \"canonical_alias_ambiguous\":false,\"unknown_attachment\":false,
                   \"system_rts\":false,\"legacy_auction_owner\":null,
                   \"received_minute_raw\":\"2026-01-01T03:00\",
                   \"items\":[{{\"ordinal\":1,\"world\":\"isekai\",\"tradeable\":true,
                               \"history_ok\":true}}]}}'::jsonb,
                 '8d000000-0000-0000-0000-000000000001');
         INSERT INTO public.consignment_custody_assets
            (equipment_id, world_id, canonical_eid, season_id, first_observation_id,
             auction_equipment_id)
         VALUES ('85000000-0000-0000-0000-000000000009', 'isekai', '43', '2026-1',
                 '84000000-0000-0000-0000-000000000009', NULL);
         INSERT INTO public.mail_initial_decisions
            (bot_uid, world_id, external_mid, id, observation_id, command_id, kind,
             quarantine_id, decided_at)
         VALUES ('12345', 'isekai', '570', '86000000-0000-0000-0000-000000000009',
                 '84000000-0000-0000-0000-000000000009', '{DECIDE}', 'accepted', NULL,
                 '{ACCEPTED}');
         INSERT INTO public.mail_final_dispositions
            (bot_uid, world_id, external_mid, id, kind, source_observation_id, command_id,
             decided_at)
         VALUES ('12345', 'isekai', '570', '87000000-0000-0000-0000-000000000009',
                 'accepted', '84000000-0000-0000-0000-000000000009', '{DECIDE}',
                 '{ACCEPTED}');
         INSERT INTO public.consignment_deposit_items
            (id, bot_uid, world_id, external_mid, decision_id, ordinal, canonical_eid,
             reservation_id, listing_id, seller_user_id, equipment_id,
             accepted_listing_version, accepted_price, credential_ref, observation_id)
         VALUES ('88000000-0000-0000-0000-000000000009', '12345', 'isekai', '570',
                 '87000000-0000-0000-0000-000000000009', 1, '43',
                 '82000000-0000-0000-0000-000000000009',
                 '20000000-0000-0000-0000-000000000009', '{USER_1}',
                 '85000000-0000-0000-0000-000000000009', 1, 101, '{ENVELOPE}',
                 '84000000-0000-0000-0000-000000000009');
         INSERT INTO public.consignment_custody_positions
            (id, item_id, listing_id, seller_user_id, equipment_id, world_id, canonical_eid,
             season_window_id, state, version, current_generation, updated_at)
         VALUES ('89000000-0000-0000-0000-000000000009',
                 '88000000-0000-0000-0000-000000000009',
                 '20000000-0000-0000-0000-000000000009', '{USER_1}',
                 '85000000-0000-0000-0000-000000000009', 'isekai', '43',
                 'deadbeef-0000-4000-8000-000000000004', 'awaiting_take', 1, NULL,
                 '{ACCEPTED}');",
        disable_related_triggers(),
        isekai_listing_and_session("43"),
        isekai_reservation("8d000000-0000-0000-0000-000000000001")
    );
    cases.push(fk("consignment_custody_positions", "c4_position_season_fk", isekai_position));
    cases
}
