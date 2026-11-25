//! 隔离解除路径的退件接管：解除表写 acquire_return 时，同事务必须已有对应 intake
//! （`c4_release_acquire_return_intake` 延迟触发器）；终局处置上的同名守卫不能代替它。

use sqlx::PgPool;

use super::super::contracts::fixture::{CONTRACT, OBLIGATION_3};
use super::super::deposit::schema::additional_rows;
use super::super::deposit::support::{
    ACCEPTED, QUARANTINE_OBSERVATION, RELEASE, RELEASE_DECISION, fails, quarantine_release_fixture,
    succeeds,
};
use super::fixture::{REFUND_DELIVERY, REFUND_MID, refund_dispatch_sql, seed_purchased};
use crate::support::TestDatabase;

const TAKE_WORK: &str = "c6a90000-0000-4000-8000-000000000001";
const TAKE_DELIVERY: &str = "c6b90000-0000-4000-8000-000000000001";
const TAKE_COMMAND: &str = "c7090000-0000-4000-8000-000000000001";
const INTAKE: &str = "c6890000-0000-4000-8000-000000000001";
const RELEASE_INSERT: &str = "INSERT INTO public.mail_quarantine_releases";

/// 隔离解除夹具改成 acquire_return：终局处置与解除行同名，回执的决定种类随之改写。
fn acquire_return_release() -> String {
    quarantine_release_fixture()
        .replace("'archived_empty'", "'acquire_return'")
        .replace("\"decision_kind\":\"archive_empty\"", "\"decision_kind\":\"acquire_return\"")
}

/// 该决定的唯一 Take 工作与 intake（K = 12346|main|556，现金退款腿）。
fn take_work_and_intake() -> String {
    format!(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
             status, fencing_version, attempt_count, created_at, updated_at)
         VALUES ('{TAKE_DELIVERY}', '{TAKE_COMMAND}', decode(repeat('69', 32), 'hex'),
                 'consignment_disposition', 'consignment:{RELEASE_DECISION}:{TAKE_WORK}',
                 'FakeBot',
                 '{{\"schema_version\":2,\"operation\":\"take\",\"world\":\"main\",
                    \"external_mail_id\":\"556\",
                    \"mail_key\":{{\"bot_uid\":\"12346\",\"world_id\":\"main\",
                    \"external_mid\":\"556\"}},
                    \"decision_id\":\"{RELEASE_DECISION}\",\"work_id\":\"{TAKE_WORK}\"}}'::jsonb,
                 'prepared', 0, 0, '{ACCEPTED}', '{ACCEPTED}');
         INSERT INTO public.mail_disposition_work
            (id, bot_uid, world_id, external_mid, decision_id, notice_id, action,
             delivery_id, expected_observation_id, manifest_digest, created_at)
         VALUES ('{TAKE_WORK}', '12346', 'main', '556', '{RELEASE_DECISION}', NULL, 'take',
                 '{TAKE_DELIVERY}', '{QUARANTINE_OBSERVATION}', decode(repeat('ab', 32), 'hex'),
                 '{ACCEPTED}');
         INSERT INTO public.consignment_return_intakes
            (id, bot_uid, world_id, external_mid, decision_id, decision_kind, contract_id,
             obligation_id, original_round, original_delivery_id, original_fence, sent_mid,
             leg, rts_observation_id, rts_returned_at, manifest_digest, session_id,
             take_work_id, take_delivery_id)
         VALUES ('{INTAKE}', '12346', 'main', '556', '{RELEASE_DECISION}', 'acquire_return',
                 '{CONTRACT}', '{OBLIGATION_3}', 1, '{REFUND_DELIVERY}', 1, '{REFUND_MID}',
                 'cash_refund', '{QUARANTINE_OBSERVATION}', '{ACCEPTED}',
                 decode(repeat('e1', 32), 'hex'), '81000000-0000-0000-0000-000000000002',
                 '{TAKE_WORK}', '{TAKE_DELIVERY}');"
    )
}

async fn seed(pool: &PgPool) {
    seed_purchased(pool).await;
    succeeds(pool, &refund_dispatch_sql()).await;
    succeeds(pool, &additional_rows()).await;
}

#[tokio::test]
async fn postgres_consignment_ledger136_release_acquire_return_requires_same_transaction_intake() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    seed(pool).await;
    let fixture = acquire_return_release();
    let release_at = fixture.find(RELEASE_INSERT).expect("夹具含解除行");
    let (decision_part, release_part) = fixture.split_at(release_at);

    // 反例：终局处置与 intake 已在先前事务提交，解除行所在事务没有本事务写入的 intake，
    // 只能由解除表上的触发器拒绝（终局处置上的同名触发器此时不会触发）。
    succeeds(pool, &format!("{decision_part}{}", take_work_and_intake())).await;
    fails(pool, release_part, "23514", Some("c4_acquire_return_intake_required")).await;
    let released: i64 = sqlx::query_scalar("SELECT count(*) FROM public.mail_quarantine_releases")
        .fetch_one(pool)
        .await
        .expect("读取解除数");
    assert_eq!(released, 0, "回滚后没有解除行");
}

#[tokio::test]
async fn postgres_consignment_ledger136_release_acquire_return_commits_with_intake() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    seed(pool).await;
    let fixture = acquire_return_release();
    let release_at = fixture.find(RELEASE_INSERT).expect("夹具含解除行");
    let (decision_part, release_part) = fixture.split_at(release_at);

    // 正例：终局处置、Take 工作、intake 与解除行同一事务提交。
    succeeds(pool, &format!("{decision_part}{}{release_part}", take_work_and_intake())).await;
    let released: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.mail_quarantine_releases
         WHERE id = $1::uuid AND kind = 'acquire_return'",
    )
    .bind(RELEASE)
    .fetch_one(pool)
    .await
    .expect("读取解除数");
    assert_eq!(released, 1);
}
