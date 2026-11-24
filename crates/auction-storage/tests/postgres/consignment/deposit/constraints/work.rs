//! 投递工作、尝试、起写与证明表的命名约束违反行。

use super::support::Case;

const START_COMMAND: &str = "9c000000-0000-5000-8000-000000000001";
const DECISION: &str = "87000000-0000-0000-0000-000000000001";

fn check(table: &'static str, constraint: &'static str, sql: &'static str) -> Case {
    Case { table, constraint, state: "23514", sql }
}

fn unique(table: &'static str, constraint: &'static str, sql: &'static str) -> Case {
    Case { table, constraint, state: "23505", sql }
}

fn fk(table: &'static str, constraint: &'static str, sql: &'static str) -> Case {
    Case { table, constraint, state: "23503", sql }
}

const SECOND_CHAIN: &str = "
INSERT INTO public.deliveries
    (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
     status, fencing_version, attempt_count, created_at, updated_at)
SELECT '8a000000-0000-0000-0000-000000000099', '8b000000-0000-0000-0000-000000000099',
       request_digest, origin_kind, origin_id, recipient, payload, status, 0, 0,
       created_at, updated_at
FROM public.deliveries WHERE id='8a000000-0000-0000-0000-000000000001';
INSERT INTO public.mail_disposition_work
    (id, bot_uid, world_id, external_mid, decision_id, notice_id, action, delivery_id,
     expected_observation_id, manifest_digest, created_at)
VALUES ('8c000000-0000-0000-0000-000000000099', '12346', 'main', '556',
        '9e000000-0000-4000-8000-000000000001', NULL, 'take',
        '8a000000-0000-0000-0000-000000000099',
        '8f000000-0000-0000-0000-000000000002', decode(repeat('68', 32), 'hex'),
        '2026-01-01T03:00:00Z');
INSERT INTO public.mail_disposition_attempts
    (id, work_id, delivery_id, fence, credential_id, credential_version,
     runtime_generation, session_id, expected_digest, prepared_at)
VALUES ('9a000000-0000-4000-8000-000000000099',
        '8c000000-0000-0000-0000-000000000099',
        '8a000000-0000-0000-0000-000000000099', 2,
        '80000000-0000-0000-0000-000000000001', 1, 1,
        '81000000-0000-0000-0000-000000000002', decode(repeat('a6', 32), 'hex'),
        '2026-01-01T03:00:00Z');
INSERT INTO public.mail_disposition_write_starts
    (work_id, decision_id, delivery_id, attempt_id, fence, started_at, command_id)
VALUES ('8c000000-0000-0000-0000-000000000099',
        '9e000000-0000-4000-8000-000000000001',
        '8a000000-0000-0000-0000-000000000099',
        '9a000000-0000-4000-8000-000000000099', 2,
        '2026-01-01T03:00:00Z', '41000000-0000-5000-8000-000000000002');
";

fn leaked(sql: String) -> &'static str {
    Box::leak(sql.into_boxed_str())
}

pub fn cases() -> Vec<Case> {
    let mut cases = vec![
        unique(
            "mail_disposition_work",
            "c4_work_delivery_uq",
            "INSERT INTO public.mail_disposition_work
                (id, bot_uid, world_id, external_mid, decision_id, notice_id, action,
                 delivery_id, expected_observation_id, manifest_digest, created_at)
              VALUES ('8c000000-0000-0000-0000-000000000098', '12346', 'main', '556',
                      '9e000000-0000-4000-8000-000000000001', NULL, 'take',
                      '8a000000-0000-0000-0000-000000000001',
                      '8f000000-0000-0000-0000-000000000002',
                      decode(repeat('68', 32), 'hex'), '2026-01-01T03:00:00Z');",
        ),
        fk(
            "mail_disposition_work",
            "c4_work_decision_fk",
            "UPDATE public.mail_disposition_work SET decision_id='deadbeef-0000-4000-8000-000000000001' WHERE id='8c000000-0000-0000-0000-000000000001';",
        ),
        fk(
            "mail_disposition_work",
            "c4_work_observation_fk",
            "UPDATE public.mail_disposition_work SET expected_observation_id='deadbeef-0000-4000-8000-000000000002' WHERE id='8c000000-0000-0000-0000-000000000001';",
        ),
        check(
            "mail_disposition_work",
            "c4_work_id_ck",
            "UPDATE public.mail_disposition_work SET id='00000000-0000-0000-0000-000000000000' WHERE id='8c000000-0000-0000-0000-000000000001';",
        ),
        check(
            "mail_disposition_work",
            "c4_work_action_ck",
            "UPDATE public.mail_disposition_work SET action='bogus' WHERE id='8c000000-0000-0000-0000-000000000001';",
        ),
        check(
            "mail_disposition_work",
            "c4_work_shape_ck",
            "UPDATE public.mail_disposition_work SET bot_uid=NULL WHERE id='8c000000-0000-0000-0000-000000000001';",
        ),
        check(
            "mail_disposition_work",
            "c4_work_time_ck",
            "UPDATE public.mail_disposition_work SET created_at='infinity' WHERE id='8c000000-0000-0000-0000-000000000001';",
        ),
        check(
            "mail_disposition_attempts",
            "c4_attempt_id_ck",
            "UPDATE public.mail_disposition_attempts SET id='00000000-0000-0000-0000-000000000000' WHERE id='9a000000-0000-4000-8000-000000000001';",
        ),
        check(
            "mail_disposition_attempts",
            "c4_attempt_versions_ck",
            "UPDATE public.mail_disposition_attempts SET fence=0 WHERE id='9a000000-0000-4000-8000-000000000001';",
        ),
        check(
            "mail_disposition_attempts",
            "c4_attempt_digest_ck",
            "UPDATE public.mail_disposition_attempts SET expected_digest=decode('11', 'hex') WHERE id='9a000000-0000-4000-8000-000000000001';",
        ),
        check(
            "mail_disposition_attempts",
            "c4_attempt_time_ck",
            "UPDATE public.mail_disposition_attempts SET prepared_at='infinity' WHERE id='9a000000-0000-4000-8000-000000000001';",
        ),
        unique(
            "mail_disposition_attempts",
            "c4_attempt_delivery_fence_uq",
            "INSERT INTO public.mail_disposition_attempts
                (id, work_id, delivery_id, fence, credential_id, credential_version,
                 runtime_generation, session_id, expected_digest, prepared_at)
             SELECT '9a000000-0000-4000-8000-000000000098', work_id, delivery_id, fence,
                    credential_id, credential_version, runtime_generation, session_id,
                    expected_digest, prepared_at
             FROM public.mail_disposition_attempts WHERE id='9a000000-0000-4000-8000-000000000001';",
        ),
        check(
            "mail_disposition_write_starts",
            "c4_start_version_ck",
            "UPDATE public.mail_disposition_write_starts SET fence=0 WHERE work_id='8c000000-0000-0000-0000-000000000001';",
        ),
        check(
            "mail_disposition_write_starts",
            "c4_start_time_ck",
            "UPDATE public.mail_disposition_write_starts SET started_at='infinity' WHERE work_id='8c000000-0000-0000-0000-000000000001';",
        ),
        check(
            "mail_disposition_proofs",
            "c4_proof_id_ck",
            "UPDATE public.mail_disposition_proofs SET id='00000000-0000-0000-0000-000000000000' WHERE id='9b000000-0000-4000-8000-000000000001';",
        ),
        check(
            "mail_disposition_proofs",
            "c4_proof_kind_ck",
            "UPDATE public.mail_disposition_proofs SET kind='bogus' WHERE id='9b000000-0000-4000-8000-000000000001';",
        ),
        check(
            "mail_disposition_proofs",
            "c4_proof_leg_ck",
            "UPDATE public.mail_disposition_proofs SET asset_leg='bogus' WHERE id='9b000000-0000-4000-8000-000000000001';",
        ),
        check(
            "mail_disposition_proofs",
            "c4_proof_conclusion_ck",
            "UPDATE public.mail_disposition_proofs SET conclusion='bogus' WHERE id='9b000000-0000-4000-8000-000000000001';",
        ),
        check(
            "mail_disposition_proofs",
            "c4_proof_take_decision_ck",
            "UPDATE public.mail_disposition_proofs SET decision_id=NULL WHERE id='9b000000-0000-4000-8000-000000000001';",
        ),
        check(
            "mail_disposition_proofs",
            "c4_proof_validator_ck",
            "UPDATE public.mail_disposition_proofs SET validator_version='' WHERE id='9b000000-0000-4000-8000-000000000001';",
        ),
        check(
            "mail_disposition_proofs",
            "c4_proof_digest_ck",
            "UPDATE public.mail_disposition_proofs SET digest=decode('11', 'hex') WHERE id='9b000000-0000-4000-8000-000000000001';",
        ),
        check(
            "mail_disposition_proofs",
            "c4_proof_time_ck",
            "UPDATE public.mail_disposition_proofs SET proven_at='infinity' WHERE id='9b000000-0000-4000-8000-000000000001';",
        ),
        fk(
            "mail_disposition_proofs",
            "c4_proof_start_fk",
            "UPDATE public.mail_disposition_proofs SET attempt_id='deadbeef-0000-4000-8000-000000000003' WHERE id='9b000000-0000-4000-8000-000000000001';",
        ),
    ];
    cases.push(unique(
        "mail_disposition_write_starts",
        "c4_start_decision_uq",
        leaked(format!(
            "{SECOND_CHAIN} UPDATE public.mail_disposition_write_starts
             SET decision_id='{DECISION}'
             WHERE work_id='8c000000-0000-0000-0000-000000000099';"
        )),
    ));
    cases.push(unique(
        "mail_disposition_write_starts",
        "c4_start_command_uq",
        leaked(format!(
            "{SECOND_CHAIN} UPDATE public.mail_disposition_write_starts
             SET command_id='{START_COMMAND}'
             WHERE work_id='8c000000-0000-0000-0000-000000000099';"
        )),
    ));
    cases.push(fk(
        "mail_disposition_write_starts",
        "c4_start_attempt_fk",
        leaked(format!(
            "{SECOND_CHAIN} UPDATE public.mail_disposition_write_starts
             SET attempt_id='deadbeef-0000-4000-8000-000000000004'
             WHERE work_id='8c000000-0000-0000-0000-000000000099';"
        )),
    ));
    cases
}

pub const EXCEPTIONS: &[(&str, &str)] = &[
    ("c4_attempt_identity_uq", "元组含主键 id；PK 先拦截"),
    ("c4_start_identity_uq", "元组含 work_id 主键；PK 先拦截"),
    (
        "c4_start_attempt_uq",
        "同一 attempt 固定回指原 work/delivery/fence，重复 attempt 必先重复 start 主键",
    ),
    (
        "c4_start_delivery_uq",
        "不可达：start 的 delivery 受 c4_start_attempt_fk 约束，attempt 的 delivery 又受 c4_attempt_delivery_fk 约束且必须等于 work.delivery；第二 work 复用 delivery 先违反 c4_work_delivery_uq，改 start/attempt 则破坏 attempt 或 work 绑定",
    ),
];
