//! 邮件、原信、隔离与旧 owner 交接表的命名约束违反行。

use super::support::Case;

const fn update(table: &'static str, assignment: &'static str, constraint: &'static str) -> Case {
    Case { table, constraint, state: "23514", sql: assignment }
}

pub const CASES: &[Case] = &[
    Case {
        table: "consignment_season_windows",
        constraint: "c4_season_id_nonzero",
        state: "23514",
        sql: "UPDATE public.consignment_season_windows SET id='00000000-0000-0000-0000-000000000000' WHERE id='8d000000-0000-0000-0000-000000000001';",
    },
    Case {
        table: "consignment_season_windows",
        constraint: "c4_season_world_ck",
        state: "23514",
        sql: "UPDATE public.consignment_season_windows SET world_id='main' WHERE id='8d000000-0000-0000-0000-000000000001';",
    },
    Case {
        table: "consignment_season_windows",
        constraint: "c4_season_text_ck",
        state: "23514",
        sql: "UPDATE public.consignment_season_windows SET season_id='' WHERE id='8d000000-0000-0000-0000-000000000001';",
    },
    Case {
        table: "consignment_season_windows",
        constraint: "c4_season_time_ck",
        state: "23514",
        sql: "UPDATE public.consignment_season_windows SET start_utc=end_utc WHERE id='8d000000-0000-0000-0000-000000000001';",
    },
    Case {
        table: "consignment_season_windows",
        constraint: "c4_season_digest_ck",
        state: "23514",
        sql: "UPDATE public.consignment_season_windows SET calendar_version=decode('11', 'hex') WHERE id='8d000000-0000-0000-0000-000000000001';",
    },
    Case {
        table: "consignment_season_windows",
        constraint: "c4_season_preclose_ck",
        state: "23514",
        sql: "UPDATE public.consignment_season_windows SET preclose_days=0 WHERE id='8d000000-0000-0000-0000-000000000001';",
    },
    Case {
        table: "consignment_season_windows",
        constraint: "c4_season_world_season_uq",
        state: "23505",
        sql: "INSERT INTO public.consignment_season_windows
                (id, season_id, world_id, timezone, start_utc, end_utc,
                 admission_cutoff_utc, calendar_version, preclose_days)
              SELECT '8d000000-0000-0000-0000-000000000099', season_id, world_id, timezone,
                     start_utc, end_utc, admission_cutoff_utc, calendar_version, preclose_days
              FROM public.consignment_season_windows WHERE id='8d000000-0000-0000-0000-000000000001';",
    },
    update(
        "mail_evidence_sessions",
        "UPDATE public.mail_evidence_sessions SET id='00000000-0000-0000-0000-000000000000' WHERE id='81000000-0000-0000-0000-000000000001';",
        "c4_session_id_nonzero",
    ),
    update(
        "mail_evidence_sessions",
        "UPDATE public.mail_evidence_sessions SET credential_id='00000000-0000-0000-0000-000000000000' WHERE id='81000000-0000-0000-0000-000000000001';",
        "c4_session_credential_nonzero",
    ),
    update(
        "mail_evidence_sessions",
        "UPDATE public.mail_evidence_sessions SET bot_uid='0' WHERE id='81000000-0000-0000-0000-000000000001';",
        "c4_session_bot_uid_ck",
    ),
    update(
        "mail_evidence_sessions",
        "UPDATE public.mail_evidence_sessions SET world_id='bogus' WHERE id='81000000-0000-0000-0000-000000000001';",
        "c4_session_world_ck",
    ),
    update(
        "mail_evidence_sessions",
        "UPDATE public.mail_evidence_sessions SET player_name='' WHERE id='81000000-0000-0000-0000-000000000001';",
        "c4_session_text_ck",
    ),
    update(
        "mail_evidence_sessions",
        "UPDATE public.mail_evidence_sessions SET credential_version=0 WHERE id='81000000-0000-0000-0000-000000000001';",
        "c4_session_versions_ck",
    ),
    update(
        "mail_evidence_sessions",
        "UPDATE public.mail_evidence_sessions SET observed_at='infinity' WHERE id='81000000-0000-0000-0000-000000000001';",
        "c4_session_observed_ck",
    ),
    update(
        "mail_evidence_sessions",
        "UPDATE public.mail_evidence_sessions SET binding_digest=decode('11', 'hex') WHERE id='81000000-0000-0000-0000-000000000001';",
        "c4_session_digest_ck",
    ),
    Case {
        table: "mail_evidence_sessions",
        constraint: "c4_session_binding_uq",
        state: "23505",
        sql: "ALTER TABLE public.mail_evidence_sessions DISABLE TRIGGER USER;
              INSERT INTO public.mail_evidence_sessions
                (id, bot_uid, world_id, player_name, credential_id, credential_version,
                 runtime_generation, observed_at, protocol_version, binding_digest)
              SELECT '81000000-0000-0000-0000-000000000099', bot_uid, world_id, player_name,
                     credential_id, credential_version, runtime_generation, observed_at,
                     protocol_version, binding_digest
              FROM public.mail_evidence_sessions WHERE id='81000000-0000-0000-0000-000000000001';",
    },
    update(
        "consignment_mail_readiness",
        "UPDATE public.consignment_mail_readiness SET world_id='bogus' WHERE world_id='main';",
        "c4_readiness_world_ck",
    ),
    Case {
        table: "consignment_mail_readiness",
        constraint: "c4_readiness_session_fk",
        state: "23503",
        sql: "UPDATE public.consignment_mail_readiness SET session_id='deadbeef-0000-4000-8000-000000000001' WHERE world_id='main';",
    },
    update(
        "consignment_mail_readiness",
        "UPDATE public.consignment_mail_readiness SET allowlist_mode='bogus' WHERE world_id='main';",
        "c4_readiness_mode_ck",
    ),
    update(
        "consignment_mail_readiness",
        "UPDATE public.consignment_mail_readiness SET allowed_senders=ARRAY[NULL]::text[] WHERE world_id='main';",
        "c4_readiness_senders_ck",
    ),
    update(
        "consignment_mail_readiness",
        "UPDATE public.consignment_mail_readiness SET validator_versions=ARRAY[NULL]::text[] WHERE world_id='main';",
        "c4_readiness_validators_ck",
    ),
    update(
        "consignment_mail_readiness",
        "UPDATE public.consignment_mail_readiness SET service_epoch=0 WHERE world_id='main';",
        "c4_readiness_epoch_ck",
    ),
    update(
        "consignment_mail_readiness",
        "UPDATE public.consignment_mail_readiness SET version=0 WHERE world_id='main';",
        "c4_readiness_version_ck",
    ),
    update(
        "consignment_mail_readiness",
        "UPDATE public.consignment_mail_readiness SET policy_digest=decode('11', 'hex') WHERE world_id='main';",
        "c4_readiness_digest_ck",
    ),
    update(
        "consignment_mail_readiness",
        "UPDATE public.consignment_mail_readiness SET valid_until=checked_at WHERE world_id='main';",
        "c4_readiness_time_ck",
    ),
    update(
        "mail_originals",
        "UPDATE public.mail_originals SET bot_uid='0' WHERE bot_uid='12345' AND external_mid='555';",
        "c4_original_bot_uid_ck",
    ),
    update(
        "mail_originals",
        "UPDATE public.mail_originals SET external_mid='0' WHERE bot_uid='12345' AND external_mid='555';",
        "c4_original_mid_ck",
    ),
    update(
        "mail_originals",
        "UPDATE public.mail_originals SET received_minute_raw='' WHERE bot_uid='12345' AND external_mid='555';",
        "c4_original_text_ck",
    ),
    update(
        "mail_originals",
        "UPDATE public.mail_originals SET first_seen_at='infinity' WHERE bot_uid='12345' AND external_mid='555';",
        "c4_original_time_ck",
    ),
    update(
        "mail_originals",
        "UPDATE public.mail_originals SET season_window_id='8d000000-0000-0000-0000-000000000001' WHERE bot_uid='12345' AND external_mid='555';",
        "c4_original_season_shape_ck",
    ),
    Case {
        table: "mail_originals",
        constraint: "c4_original_session_fk",
        state: "23503",
        sql: "UPDATE public.mail_originals SET first_session_id='deadbeef-0000-4000-8000-000000000002' WHERE bot_uid='12345' AND external_mid='555';",
    },
    Case {
        table: "mail_originals",
        constraint: "c4_original_season_fk",
        state: "23503",
        sql: "ALTER TABLE public.mail_evidence_sessions DISABLE TRIGGER USER;
              INSERT INTO public.mail_evidence_sessions
                (id, bot_uid, world_id, player_name, credential_id, credential_version,
                 runtime_generation, observed_at, protocol_version, binding_digest)
              SELECT '81000000-0000-0000-0000-000000000099', bot_uid, 'isekai', player_name,
                     credential_id, credential_version, runtime_generation, observed_at,
                     protocol_version, decode(repeat('e1', 32), 'hex')
              FROM public.mail_evidence_sessions WHERE id='81000000-0000-0000-0000-000000000001';
              INSERT INTO public.mail_originals
                (bot_uid, world_id, external_mid, first_session_id, received_minute_raw,
                 received_timezone, first_seen_at, season_window_id)
              VALUES ('12345', 'isekai', '557', '81000000-0000-0000-0000-000000000099',
                      '2026-01-01T03:00', 'UTC', '2026-01-01T03:00:00Z',
                      'deadbeef-0000-4000-8000-000000000003');",
    },
    update(
        "mail_evidence_envelopes",
        "UPDATE public.mail_evidence_envelopes SET id='00000000-0000-0000-0000-000000000000' WHERE id='83000000-0000-0000-0000-000000000001';",
        "c4_envelope_id_nonzero",
    ),
    update(
        "mail_evidence_envelopes",
        "UPDATE public.mail_evidence_envelopes SET purpose='' WHERE id='83000000-0000-0000-0000-000000000001';",
        "c4_envelope_text_ck",
    ),
    update(
        "mail_evidence_envelopes",
        "UPDATE public.mail_evidence_envelopes SET updated_at='2025-12-31T00:00:00Z' WHERE id='83000000-0000-0000-0000-000000000001';",
        "c4_envelope_times_ck",
    ),
    update(
        "mail_evidence_envelopes",
        "UPDATE public.mail_evidence_envelopes SET nonce=decode('11', 'hex') WHERE id='83000000-0000-0000-0000-000000000001';",
        "c4_envelope_shape_ck",
    ),
    update(
        "mail_envelope_observations",
        "UPDATE public.mail_envelope_observations SET id='00000000-0000-0000-0000-000000000000' WHERE id='84000000-0000-0000-0000-000000000001';",
        "c4_observation_id_nonzero",
    ),
    update(
        "mail_envelope_observations",
        "UPDATE public.mail_envelope_observations SET bot_uid='0' WHERE id='84000000-0000-0000-0000-000000000001';",
        "c4_observation_bot_uid_ck",
    ),
    update(
        "mail_envelope_observations",
        "UPDATE public.mail_envelope_observations SET external_mid='0' WHERE id='84000000-0000-0000-0000-000000000001';",
        "c4_observation_mid_ck",
    ),
    Case {
        table: "mail_envelope_observations",
        constraint: "c4_observation_session_fk",
        state: "23503",
        sql: "UPDATE public.mail_envelope_observations SET session_id='deadbeef-0000-4000-8000-000000000004' WHERE id='84000000-0000-0000-0000-000000000001';",
    },
    Case {
        table: "mail_envelope_observations",
        constraint: "c4_observation_season_fk",
        state: "23503",
        sql: "ALTER TABLE public.mail_evidence_sessions DISABLE TRIGGER USER;
              INSERT INTO public.mail_evidence_sessions
                (id, bot_uid, world_id, player_name, credential_id, credential_version,
                 runtime_generation, observed_at, protocol_version, binding_digest)
              SELECT '81000000-0000-0000-0000-000000000098', bot_uid, 'isekai', player_name,
                     credential_id, credential_version, runtime_generation, observed_at,
                     protocol_version, decode(repeat('e2', 32), 'hex')
              FROM public.mail_evidence_sessions WHERE id='81000000-0000-0000-0000-000000000001';
              INSERT INTO public.mail_envelope_observations
                (id, bot_uid, world_id, external_mid, remote_observation_id, session_id,
                 mailbox_key, quarantine_id, quarantine_version, observed_at, parser_version,
                 evidence_ref, subject_digest, body_digest, manifest_digest, sender, recipient,
                 returnable, takeable, credits, cod, raw_count, manifest, derived_facts,
                 season_window_id)
              SELECT '84000000-0000-0000-0000-000000000099', '12345', 'isekai', '558',
                     'obs-isekai', '81000000-0000-0000-0000-000000000098', mailbox_key,
                     NULL, NULL, observed_at, parser_version, evidence_ref, subject_digest,
                     body_digest, manifest_digest, sender, recipient, returnable, takeable,
                     credits, cod, raw_count, manifest, derived_facts,
                     'deadbeef-0000-4000-8000-000000000005'
              FROM public.mail_envelope_observations WHERE id='84000000-0000-0000-0000-000000000001';",
    },
    update(
        "mail_envelope_observations",
        "UPDATE public.mail_envelope_observations SET remote_observation_id='' WHERE id='84000000-0000-0000-0000-000000000001';",
        "c4_observation_text_ck",
    ),
    update(
        "mail_envelope_observations",
        "UPDATE public.mail_envelope_observations SET observed_at='infinity' WHERE id='84000000-0000-0000-0000-000000000001';",
        "c4_observation_time_ck",
    ),
    update(
        "mail_envelope_observations",
        "UPDATE public.mail_envelope_observations SET subject_digest=decode('11', 'hex') WHERE id='84000000-0000-0000-0000-000000000001';",
        "c4_observation_digest_ck",
    ),
    update(
        "mail_envelope_observations",
        "UPDATE public.mail_envelope_observations SET quarantine_version=0 WHERE id='84000000-0000-0000-0000-000000000001';",
        "c4_observation_quarantine_ck",
    ),
    update(
        "mail_envelope_observations",
        "UPDATE public.mail_envelope_observations SET raw_count=0 WHERE id='84000000-0000-0000-0000-000000000001';",
        "c4_observation_amount_ck",
    ),
    update(
        "mail_envelope_observations",
        "UPDATE public.mail_envelope_observations SET manifest='[{\"ordinal\":0,\"kind\":\"equipment\"}]'::jsonb WHERE id='84000000-0000-0000-0000-000000000001';",
        "c4_observation_manifest_ck",
    ),
    update(
        "mail_envelope_observations",
        "UPDATE public.mail_envelope_observations SET derived_facts='{}'::jsonb WHERE id='84000000-0000-0000-0000-000000000001';",
        "c4_observation_derived_ck",
    ),
    update(
        "mail_envelope_observations",
        "UPDATE public.mail_envelope_observations SET season_window_id='8d000000-0000-0000-0000-000000000001' WHERE id='84000000-0000-0000-0000-000000000001';",
        "c4_observation_season_shape_ck",
    ),
    Case {
        table: "mail_envelope_observations",
        constraint: "c4_observation_remote_uq",
        state: "23505",
        sql: "INSERT INTO public.mail_envelope_observations
                (id, bot_uid, world_id, external_mid, remote_observation_id, session_id,
                 mailbox_key, quarantine_id, quarantine_version, observed_at, parser_version,
                 evidence_ref, subject_digest, body_digest, manifest_digest, sender, recipient,
                 returnable, takeable, credits, cod, raw_count, manifest, derived_facts,
                 season_window_id)
              SELECT '84000000-0000-0000-0000-000000000098', bot_uid, world_id, external_mid,
                     remote_observation_id, session_id, mailbox_key, quarantine_id,
                     quarantine_version, observed_at, parser_version, evidence_ref,
                     subject_digest, body_digest, manifest_digest, sender, recipient,
                     returnable, takeable, credits, cod, raw_count, manifest, derived_facts,
                     season_window_id
              FROM public.mail_envelope_observations WHERE id='84000000-0000-0000-0000-000000000001';",
    },
    Case {
        table: "mail_envelope_observations",
        constraint: "c4_observation_quarantine_id_fk",
        state: "23503",
        sql: "INSERT INTO public.mail_envelope_observations
              SELECT (jsonb_populate_record(NULL::public.mail_envelope_observations,
                      to_jsonb(src) || '{\"id\":\"8f000000-0000-0000-0000-000000000099\",\"remote_observation_id\":\"obs-q-3\"}'::jsonb)).*
              FROM public.mail_envelope_observations src
              WHERE id='8f000000-0000-0000-0000-000000000002';
              UPDATE public.mail_envelope_observations
              SET quarantine_id='deadbeef-0000-4000-8000-000000000006'
              WHERE id='8f000000-0000-0000-0000-000000000099';",
    },
    Case {
        table: "mail_original_links",
        constraint: "c4_link_mailbox_uq",
        state: "23505",
        sql: "INSERT INTO public.inbound_mail
              SELECT (jsonb_populate_record(NULL::public.inbound_mail,
                      to_jsonb(src) || '{\"id\":\"8e000000-0000-0000-0000-000000000099\",\"mailbox_key\":\"other-mailbox\"}'::jsonb)).*
              FROM public.inbound_mail src WHERE id='8e000000-0000-0000-0000-000000000001';
              INSERT INTO public.mail_original_links
                (inbound_id, mailbox_key, external_mid, bot_uid, world_id, auth_observation_id)
              VALUES ('8e000000-0000-0000-0000-000000000099', 'fake-mailbox', '555',
                      '12345', 'main', '84000000-0000-0000-0000-000000000001');",
    },
    Case {
        table: "mail_original_links",
        constraint: "c4_link_key_fk",
        state: "23503",
        sql: "INSERT INTO public.mail_evidence_sessions
                (id, bot_uid, world_id, player_name, credential_id, credential_version,
                 runtime_generation, observed_at, protocol_version, binding_digest)
              SELECT '81000000-0000-0000-0000-000000000097', '999', world_id, player_name,
                     credential_id, credential_version, runtime_generation, observed_at,
                     protocol_version, decode(repeat('e3', 32), 'hex')
              FROM public.mail_evidence_sessions WHERE id='81000000-0000-0000-0000-000000000001';
              INSERT INTO public.mail_envelope_observations
                (id, bot_uid, world_id, external_mid, remote_observation_id, session_id,
                 mailbox_key, quarantine_id, quarantine_version, observed_at, parser_version,
                 evidence_ref, subject_digest, body_digest, manifest_digest, sender, recipient,
                 returnable, takeable, credits, cod, raw_count, manifest, derived_facts,
                 season_window_id)
              SELECT '84000000-0000-0000-0000-000000000097', '999', world_id, '555',
                     'obs-999', '81000000-0000-0000-0000-000000000097', mailbox_key,
                     NULL, NULL, observed_at, parser_version, evidence_ref, subject_digest,
                     body_digest, manifest_digest, sender, recipient, returnable, takeable,
                     credits, cod, raw_count, manifest, derived_facts, NULL
              FROM public.mail_envelope_observations WHERE id='84000000-0000-0000-0000-000000000001';
              UPDATE public.mail_original_links
              SET bot_uid='999', auth_observation_id='84000000-0000-0000-0000-000000000097'
              WHERE inbound_id='8e000000-0000-0000-0000-000000000001';",
    },
    Case {
        table: "mail_original_links",
        constraint: "c4_link_observation_fk",
        state: "23503",
        sql: "UPDATE public.mail_original_links SET auth_observation_id='deadbeef-0000-4000-8000-000000000007' WHERE inbound_id='8e000000-0000-0000-0000-000000000001';",
    },
    Case {
        table: "mail_original_links",
        constraint: "c4_link_inbound_identity_fk",
        state: "23503",
        sql: "INSERT INTO public.inbound_mail
              SELECT (jsonb_populate_record(NULL::public.inbound_mail,
                      to_jsonb(src) || '{\"id\":\"8e000000-0000-0000-0000-000000000098\",\"mailbox_key\":\"third-mailbox\",\"external_mail_id\":\"999\"}'::jsonb)).*
              FROM public.inbound_mail src WHERE id='8e000000-0000-0000-0000-000000000001';
              UPDATE public.mail_original_links
              SET inbound_id='8e000000-0000-0000-0000-000000000098'
              WHERE inbound_id='8e000000-0000-0000-0000-000000000001';",
    },
    update(
        "mail_original_links",
        "UPDATE public.mail_original_links SET mailbox_key='' WHERE inbound_id='8e000000-0000-0000-0000-000000000001';",
        "c4_link_text_ck",
    ),
    update(
        "mail_observation_conflicts",
        "UPDATE public.mail_observation_conflicts SET conflicting_digest=decode('11', 'hex') WHERE observation_id='84000000-0000-0000-0000-000000000001';",
        "c4_conflict_digest_ck",
    ),
    update(
        "mail_observation_conflicts",
        "UPDATE public.mail_observation_conflicts SET evidence_ref='' WHERE observation_id='84000000-0000-0000-0000-000000000001';",
        "c4_conflict_evidence_ck",
    ),
    update(
        "mail_observation_conflicts",
        "UPDATE public.mail_observation_conflicts SET observed_at='infinity' WHERE observation_id='84000000-0000-0000-0000-000000000001';",
        "c4_conflict_time_ck",
    ),
    update(
        "mail_initial_decisions",
        "UPDATE public.mail_initial_decisions SET kind='bogus' WHERE id='86000000-0000-0000-0000-000000000001';",
        "c4_initial_decision_kind_ck",
    ),
    update(
        "mail_initial_decisions",
        "UPDATE public.mail_initial_decisions SET quarantine_id='91000000-0000-0000-0000-000000000001' WHERE id='86000000-0000-0000-0000-000000000001';",
        "c4_initial_decision_quarantine_ck",
    ),
    update(
        "mail_initial_decisions",
        "UPDATE public.mail_initial_decisions SET id='00000000-0000-0000-0000-000000000000' WHERE id='86000000-0000-0000-0000-000000000001';",
        "c4_initial_decision_id_ck",
    ),
    update(
        "mail_initial_decisions",
        "UPDATE public.mail_initial_decisions SET decided_at='infinity' WHERE id='86000000-0000-0000-0000-000000000001';",
        "c4_initial_decision_time_ck",
    ),
    Case {
        table: "mail_initial_decisions",
        constraint: "c4_initial_decision_observation_fk",
        state: "23503",
        sql: "UPDATE public.mail_initial_decisions SET observation_id='deadbeef-0000-4000-8000-000000000008' WHERE id='86000000-0000-0000-0000-000000000001';",
    },
    Case {
        table: "mail_initial_decisions",
        constraint: "c4_initial_quarantine_fk",
        state: "23503",
        sql: "UPDATE public.mail_initial_decisions SET quarantine_id='deadbeef-0000-4000-8000-000000000009' WHERE bot_uid='12346';",
    },
    Case {
        table: "mail_initial_decisions",
        constraint: "c4_initial_decision_id_uq",
        state: "23505",
        sql: "UPDATE public.mail_initial_decisions SET id='90000000-0000-0000-0000-000000000001' WHERE id='86000000-0000-0000-0000-000000000001';",
    },
    update(
        "mail_quarantines",
        "UPDATE public.mail_quarantines SET id='00000000-0000-0000-0000-000000000000' WHERE id='91000000-0000-0000-0000-000000000001';",
        "c4_quarantine_id_ck",
    ),
    update(
        "mail_quarantines",
        "UPDATE public.mail_quarantines SET state='bogus' WHERE id='91000000-0000-0000-0000-000000000001';",
        "c4_quarantine_state_ck",
    ),
    update(
        "mail_quarantines",
        "UPDATE public.mail_quarantines SET responsibility_code='bogus' WHERE id='91000000-0000-0000-0000-000000000001';",
        "c4_quarantine_responsibility_ck",
    ),
    update(
        "mail_quarantines",
        "UPDATE public.mail_quarantines SET version=0 WHERE id='91000000-0000-0000-0000-000000000001';",
        "c4_quarantine_version_ck",
    ),
    update(
        "mail_quarantines",
        "UPDATE public.mail_quarantines SET reread_locator='' WHERE id='91000000-0000-0000-0000-000000000001';",
        "c4_quarantine_text_ck",
    ),
    update(
        "mail_quarantines",
        "UPDATE public.mail_quarantines SET updated_at='2025-12-31T00:00:00Z' WHERE id='91000000-0000-0000-0000-000000000001';",
        "c4_quarantine_time_ck",
    ),
    Case {
        table: "mail_quarantines",
        constraint: "c4_quarantine_initial_fk",
        state: "23503",
        sql: "UPDATE public.mail_quarantines SET initial_decision_id='deadbeef-0000-4000-8000-00000000000a' WHERE id='91000000-0000-0000-0000-000000000001';",
    },
    update(
        "mail_quarantine_rechecks",
        "UPDATE public.mail_quarantine_rechecks SET id='00000000-0000-0000-0000-000000000000' WHERE id='93000000-0000-0000-0000-000000000001';",
        "c4_recheck_id_ck",
    ),
    update(
        "mail_quarantine_rechecks",
        "UPDATE public.mail_quarantine_rechecks SET requested_version=0 WHERE id='93000000-0000-0000-0000-000000000001';",
        "c4_recheck_version_ck",
    ),
    update(
        "mail_quarantine_rechecks",
        "UPDATE public.mail_quarantine_rechecks SET available_at='infinity' WHERE id='93000000-0000-0000-0000-000000000001';",
        "c4_recheck_time_ck",
    ),
    update(
        "mail_quarantine_rechecks",
        "UPDATE public.mail_quarantine_rechecks SET state='leased' WHERE id='93000000-0000-0000-0000-000000000001';",
        "c4_recheck_shape_ck",
    ),
    update(
        "mail_quarantine_rechecks",
        "UPDATE public.mail_quarantine_rechecks SET fence=-1 WHERE id='93000000-0000-0000-0000-000000000001';",
        "c4_recheck_counts_ck",
    ),
    Case {
        table: "mail_quarantine_rechecks",
        constraint: "c4_recheck_command_uq",
        state: "23505",
        sql: "INSERT INTO public.mail_quarantine_rechecks
                (id, quarantine_id, requested_version, command_id, state, available_at,
                 lease_token, lease_until, fence, attempt_count, completed_at)
              SELECT '93000000-0000-0000-0000-000000000099', quarantine_id, 2, command_id,
                     'done', available_at, NULL, NULL, fence, attempt_count, '2026-01-01T03:00:00Z'
              FROM public.mail_quarantine_rechecks WHERE id='93000000-0000-0000-0000-000000000001';",
    },
    update(
        "mail_final_dispositions",
        "UPDATE public.mail_final_dispositions SET kind='bogus' WHERE id='87000000-0000-0000-0000-000000000001';",
        "c4_final_kind_ck",
    ),
    update(
        "mail_final_dispositions",
        "UPDATE public.mail_final_dispositions SET id='00000000-0000-0000-0000-000000000000' WHERE id='87000000-0000-0000-0000-000000000001';",
        "c4_final_id_ck",
    ),
    update(
        "mail_final_dispositions",
        "UPDATE public.mail_final_dispositions SET decided_at='infinity' WHERE id='87000000-0000-0000-0000-000000000001';",
        "c4_final_time_ck",
    ),
    Case {
        table: "mail_final_dispositions",
        constraint: "c4_final_observation_fk",
        state: "23503",
        sql: "UPDATE public.mail_final_dispositions SET source_observation_id='deadbeef-0000-4000-8000-00000000000b' WHERE id='87000000-0000-0000-0000-000000000001';",
    },
    Case {
        table: "mail_final_dispositions",
        constraint: "c4_final_id_uq",
        state: "23505",
        sql: "UPDATE public.mail_final_dispositions SET id='9e000000-0000-4000-8000-000000000001' WHERE id='87000000-0000-0000-0000-000000000001';",
    },
];

pub const EXCEPTIONS: &[(&str, &str)] = &[
    ("c4_season_id_world_uq", "主键 id 已唯一，无法在无主键冲突时重复 (id, world_id)"),
    ("c4_season_id_season_uq", "主键 id 已唯一，无法在无主键冲突时重复 (id, season_id)"),
    (
        "c4_original_world_ck",
        "world_id 非 main/isekai 时 c4_original_season_shape_ck 同时为假并先失败",
    ),
    (
        "c4_observation_world_ck",
        "world_id 非 main/isekai 时 c4_observation_season_shape_ck 同时为假并先失败",
    ),
    ("c4_session_identity_uq", "主键 id 已唯一，复合唯一键只服务 FK"),
    ("c4_session_world_uq", "主键 id 已唯一，复合唯一键只服务 FK"),
    ("c4_observation_id_uq", "主键 id 已唯一，复合唯一键只服务 FK"),
    ("c4_observation_quarantine_id_uq", "元组包含主键 id，重复必先触发主键"),
    ("c4_initial_decision_key_id_uq", "元组包含 id；id 唯一约束先拦截"),
    ("c4_initial_decision_key_id_kind_uq", "元组包含 id；id 唯一约束先拦截"),
    (
        "c4_quarantine_decision_uq",
        "不可达：c4_initial_decision_id_uq 使首决定 id 全局唯一，重复 initial_decision_id 必属同一 K（c4_quarantine_initial_fk），同时违反 c4_quarantine_key_uq；实测后者先被报告",
    ),
    ("c4_quarantine_key_id_uq", "元组含 K；c4_quarantine_key_uq 已限制每 K 一行"),
    ("c4_quarantine_id_version_uq", "元组含主键 id；PK 先拦截"),
    ("c4_recheck_state_ck", "state 非闭集时 c4_recheck_shape_ck 的 CASE 也返回 NULL 而先失败"),
    ("c4_final_key_id_uq", "元组包含 id；id 唯一约束先拦截"),
    ("c4_final_key_id_kind_uq", "元组包含 id；id 唯一约束先拦截"),
    ("c4_handoff_key_id_uq", "元组包含主键 id；PK 先拦截"),
    (
        "c4_release_decision_uq",
        "不可达：同一 decision_id 只属于一个 K（终局主键 + c4_release_decision_fk），重复 decision 必同时重复 K；实测 c4_release_key_uq 先被报告",
    ),
    (
        "c4_release_case_evidence_fk",
        "settlement_return_cases.return_mail_id 的 FK 保证案件必先有证据，branch 又要求 case_evidence_id 等于 case_return_mail_id，因此无法单独违反",
    ),
];
