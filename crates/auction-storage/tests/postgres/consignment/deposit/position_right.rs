//! 整封接受登记卖家取回权（7D-6）：取得证明事务内登记 `withdraw_held`，同 position 同卖家至多一行。

use auction_application::consignment::{
    ApplyMailProofCommand, DispositionAction, DispositionStore, MailProofKind,
};
use auction_application::core::RequestHash;
use auction_domain::CommandId;
use sqlx::PgPool;
use uuid::Uuid;

use super::envelopes::repository;
use super::support::{
    ACCEPTED, ATTEMPT, CREDENTIAL, DECISION, DELIVERY, OBSERVATION, SESSION, WORK,
    accepted_envelope, succeeds,
};
use crate::support::TestDatabase;

async fn control(pool: &PgPool) {
    succeeds(
        pool,
        "UPDATE public.consignment_lookup_control
         SET admitting = true, enabled_epoch = 1,
             worker_ready_until = clock_timestamp() + interval '1 hour'
         WHERE id = 1;",
    )
    .await;
}

async fn seed_attempt_and_start(owner: &PgPool) {
    succeeds(
        owner,
        &format!(
            "INSERT INTO public.mail_disposition_attempts
                (id, work_id, delivery_id, fence, credential_id, credential_version,
                 runtime_generation, session_id, expected_digest, prepared_at)
             VALUES ('{ATTEMPT}', '{WORK}', '{DELIVERY}', 1, '{CREDENTIAL}', 1, 1,
                     '{SESSION}', decode(repeat('68', 32), 'hex'), '{ACCEPTED}');
             UPDATE public.deliveries
             SET status = 'claimed', lease_token = '00000000-0000-4000-8000-0000000000aa',
                 lease_expires_at = timestamptz '2099-01-01T00:00:00Z',
                 fencing_version = 1, attempt_count = 1
             WHERE id = '{DELIVERY}';
             INSERT INTO public.mail_disposition_write_starts
                (work_id, decision_id, delivery_id, attempt_id, fence, started_at, command_id)
             VALUES ('{WORK}', '{DECISION}', '{DELIVERY}', '{ATTEMPT}', 1, '{ACCEPTED}',
                     (SELECT command_id FROM public.consignment_commands ORDER BY created_at LIMIT 1));"
        ),
    )
    .await;
}

#[tokio::test]
async fn postgres_consignment_take_proof_registers_one_withdraw_held_right_per_position() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    succeeds(pool, &accepted_envelope()).await;
    control(pool).await;
    let owner = database.maintenance_store().await.expect("维护账号");
    // 取得证明之前（position 仍 awaiting_take）没有权利行。
    let count = || async {
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM public.consignment_rights")
            .fetch_one(pool)
            .await
            .expect("权利行数")
    };
    assert_eq!(count().await, 0);
    seed_attempt_and_start(owner.postgres_pool()).await;
    let repository = repository(&database).await;
    let apply = || ApplyMailProofCommand {
        command_id: CommandId::from_uuid(Uuid::new_v5(
            &Uuid::NAMESPACE_URL,
            b"position-right-take",
        )),
        request_hash: RequestHash::new([33; 32]),
        original_attempt_id: Uuid::parse_str(ATTEMPT).expect("attempt"),
        proof_kind: MailProofKind::ExactTake,
        final_decision_id: Uuid::parse_str(DECISION).expect("决定"),
        action: DispositionAction::Take,
        delivery_id: Uuid::parse_str(DELIVERY).expect("投递"),
        fence: 1,
        observation_id: Uuid::parse_str(OBSERVATION).expect("观察"),
        validator_version: "validator-test".to_owned(),
        proof_digest: [0xa7; 32],
    };
    repository.apply_mail_proof(apply()).await.expect("应用取得证明");
    // 正例：卖家得到唯一一行 withdraw_held，目标是 position，状态版本 1 带 withdraw_held 动作。
    let row: (String, Option<Uuid>, Vec<String>) = sqlx::query_as(
        "SELECT r.kind, r.contract_id, s.allowed_actions
         FROM public.consignment_rights AS r
         JOIN public.consignment_right_states AS s USING (right_id)",
    )
    .fetch_one(pool)
    .await
    .expect("读取权利");
    assert_eq!(row, ("withdraw_held".to_owned(), None, vec!["withdraw_held".to_owned()]));
    // 反例：同命令重放不增权利行。
    repository.apply_mail_proof(apply()).await.expect("重放");
    assert_eq!(count().await, 1);
}
