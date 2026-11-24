//! 旧 owner 恢复接管与隔离解除表的命名约束违反行。

use super::support::Case;

const fn update(table: &'static str, assignment: &'static str, constraint: &'static str) -> Case {
    Case { table, constraint, state: "23514", sql: assignment }
}

pub fn cases() -> Vec<Case> {
    vec![
        update(
            "settlement_recovery_return_handoffs",
            "UPDATE public.settlement_recovery_return_handoffs SET id='00000000-0000-0000-0000-000000000000' WHERE id='b9000000-0000-4000-8000-000000000001';",
            "c4_handoff_id_ck",
        ),
        update(
            "settlement_recovery_return_handoffs",
            "UPDATE public.settlement_recovery_return_handoffs SET accepted_at='infinity' WHERE id='b9000000-0000-4000-8000-000000000001';",
            "c4_handoff_time_ck",
        ),
        Case {
            table: "settlement_recovery_return_handoffs",
            constraint: "c4_handoff_observation_fk",
            state: "23503",
            sql: "UPDATE public.settlement_recovery_return_handoffs SET observation_id='deadbeef-0000-4000-8000-00000000000c' WHERE id='b9000000-0000-4000-8000-000000000001';",
        },
        Case {
            table: "settlement_recovery_return_handoffs",
            constraint: "c4_handoff_mail_identity_fk",
            state: "23503",
            sql: "INSERT INTO public.inbound_mail
                  SELECT (jsonb_populate_record(NULL::public.inbound_mail,
                          to_jsonb(src) || '{\"id\":\"b7000000-0000-4000-8000-000000000099\",\"mailbox_key\":\"other-return\",\"external_mail_id\":\"999\"}'::jsonb)).*
                  FROM public.inbound_mail src WHERE id='b7000000-0000-4000-8000-000000000001';
                  UPDATE public.settlement_recovery_return_handoffs
                  SET return_mail_id='b7000000-0000-4000-8000-000000000099'
                  WHERE id='b9000000-0000-4000-8000-000000000001';",
        },
        Case {
            table: "settlement_recovery_return_handoffs",
            constraint: "c4_handoff_key_uq",
            state: "23505",
            sql: "INSERT INTO public.inbound_mail
                  SELECT (jsonb_populate_record(NULL::public.inbound_mail,
                          to_jsonb(src) || '{\"id\":\"b7000000-0000-4000-8000-000000000098\",\"mailbox_key\":\"second-return\"}'::jsonb)).*
                  FROM public.inbound_mail src WHERE id='b7000000-0000-4000-8000-000000000001';
                  INSERT INTO public.settlement_recovery_return_handoffs
                    (id, bot_uid, world_id, external_mid, return_mail_id, recovery_id,
                     obligation_id, observation_id, accepted_at)
                  SELECT 'b9000000-0000-4000-8000-000000000098', bot_uid, world_id, external_mid,
                         'b7000000-0000-4000-8000-000000000098', recovery_id, obligation_id,
                         observation_id, accepted_at
                  FROM public.settlement_recovery_return_handoffs
                  WHERE id='b9000000-0000-4000-8000-000000000001';",
        },
        Case {
            table: "settlement_recovery_return_handoffs",
            constraint: "c4_handoff_mail_uq",
            state: "23505",
            sql: "INSERT INTO public.mail_evidence_sessions
                    (id, bot_uid, world_id, player_name, credential_id, credential_version,
                     runtime_generation, observed_at, protocol_version, binding_digest)
                  SELECT '81000000-0000-0000-0000-000000000096', '458', world_id, player_name,
                         credential_id, credential_version, runtime_generation, observed_at,
                         protocol_version, decode(repeat('e4', 32), 'hex')
                  FROM public.mail_evidence_sessions WHERE id='81000000-0000-0000-0000-000000000001';
                  INSERT INTO public.mail_envelope_observations
                    (id, bot_uid, world_id, external_mid, remote_observation_id, session_id,
                     mailbox_key, quarantine_id, quarantine_version, observed_at, parser_version,
                     evidence_ref, subject_digest, body_digest, manifest_digest, sender, recipient,
                     returnable, takeable, credits, cod, raw_count, manifest, derived_facts,
                     season_window_id)
                  SELECT '8f000000-0000-0000-0000-000000000096', '458', 'main', '556',
                         'obs-458', '81000000-0000-0000-0000-000000000096', mailbox_key,
                         NULL, NULL, observed_at, parser_version, evidence_ref, subject_digest,
                         body_digest, manifest_digest, sender, recipient, returnable, takeable,
                         credits, cod, raw_count, manifest, derived_facts, NULL
                  FROM public.mail_envelope_observations WHERE id='8f000000-0000-0000-0000-000000000001';
                  INSERT INTO public.settlement_recovery_return_handoffs
                    (id, bot_uid, world_id, external_mid, return_mail_id, recovery_id,
                     obligation_id, observation_id, accepted_at)
                  SELECT 'b9000000-0000-4000-8000-000000000097', '458', world_id, external_mid,
                         return_mail_id, recovery_id, obligation_id,
                         '8f000000-0000-0000-0000-000000000096', accepted_at
                  FROM public.settlement_recovery_return_handoffs
                  WHERE id='b9000000-0000-4000-8000-000000000001';",
        },
        update(
            "mail_quarantine_releases",
            "UPDATE public.mail_quarantine_releases SET id='00000000-0000-0000-0000-000000000000' WHERE id='9f000000-0000-4000-8000-000000000001';",
            "c4_release_id_ck",
        ),
        update(
            "mail_quarantine_releases",
            "UPDATE public.mail_quarantine_releases SET expected_version=0 WHERE id='9f000000-0000-4000-8000-000000000001';",
            "c4_release_version_ck",
        ),
        update(
            "mail_quarantine_releases",
            "UPDATE public.mail_quarantine_releases SET kind='bogus' WHERE id='9f000000-0000-4000-8000-000000000001';",
            "c4_release_kind_ck",
        ),
        update(
            "mail_quarantine_releases",
            "UPDATE public.mail_quarantine_releases SET kind='auction_return_case' WHERE id='9f000000-0000-4000-8000-000000000001';",
            "c4_release_branch_ck",
        ),
        update(
            "mail_quarantine_releases",
            "UPDATE public.mail_quarantine_releases SET released_at='infinity' WHERE id='9f000000-0000-4000-8000-000000000001';",
            "c4_release_time_ck",
        ),
        Case {
            table: "mail_quarantine_releases",
            constraint: "c4_release_quarantine_fk",
            state: "23503",
            sql: "UPDATE public.mail_quarantine_releases SET observation_id='84000000-0000-0000-0000-000000000001' WHERE id='9f000000-0000-4000-8000-000000000001';",
        },
        Case {
            table: "mail_quarantine_releases",
            constraint: "c4_release_decision_fk",
            state: "23503",
            sql: "UPDATE public.mail_quarantine_releases SET decision_id='87000000-0000-0000-0000-000000000001' WHERE id='9f000000-0000-4000-8000-000000000001';",
        },
    ]
}
