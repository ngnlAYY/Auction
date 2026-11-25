//! 退件 Take 工作的领取来源（PIWS3b）：期 4 处置领取不碰接管建的 Take 工作；退件 Take 领取只领尚未起写的
//! 接管工作，经退件起写只发一次许可，起写之后不再领取。
//!
//! 只测核心规则：资产外写（期 4 零领取、许可只一次、已起写零重领）与安全边界（旧领取重读失败关闭）。

use std::sync::Arc;

use auction_application::consignment::{StartTakeInput, WriteStart};
use auction_application::mail::{DispositionWorkStore, ReturnTakeLease, ReturnTakeWorkStore};
use auction_storage::DatabaseStore;
use auction_time::{Duration, Timestamp};
use sqlx::PgPool;
use uuid::Uuid;

use super::super::deposit::envelopes::repository;
use super::fixture::{
    MANIFEST, Rts, original_facts, seed_main_readiness, seed_original_rts, service, session_binding,
};
use crate::support::TestDatabase;

/// (投递状态, fence, 该工作的 attempt 数, 起写事实数)
async fn take_state(pool: &PgPool, work_id: Uuid) -> (String, i64, i64, i64) {
    sqlx::query_as(
        "SELECT d.status, d.fencing_version,
                (SELECT count(*) FROM public.mail_disposition_attempts WHERE work_id = w.id),
                (SELECT count(*) FROM public.mail_disposition_write_starts WHERE work_id = w.id)
         FROM public.mail_disposition_work AS w
         JOIN public.deliveries AS d ON d.id = w.delivery_id
         WHERE w.id = $1",
    )
    .bind(work_id)
    .fetch_one(pool)
    .await
    .expect("读取 Take 工作")
}

async fn claim_return_take(
    store: &DatabaseStore,
) -> Option<auction_application::mail::ReturnTakeClaim> {
    let now = Timestamp::now();
    store
        .claim_due_return_take("take-worker".to_owned(), now, now + Duration::minutes(5))
        .await
        .expect("退件 Take 领取")
}

/// 反例：接管建的 Take 工作不被期 4 处置领取（世界已就绪、投递到期），投递与 attempt 原样不动；
/// 正例：退件 Take 来源领到它，绑定该接管、推进到 fence 1。
#[tokio::test]
async fn postgres_consignment_returns_phase4_claim_leaves_intake_take_work_to_the_return_source() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("维护账号");
    let store = Arc::new(database.store().await.expect("运行账号"));
    let pool = owner.postgres_pool();
    let rts = Rts { n: 1 };
    seed_original_rts(pool, rts).await;
    let receipt = service(&store).acquire(original_facts(rts)).await.expect("接管");
    seed_main_readiness(pool).await;
    // 夹具里的寄售收件取得工作（期 4 自己的工作）先收口，期 4 领取此时只剩接管的 Take 可选。
    sqlx::query(
        "UPDATE public.deliveries
         SET status = 'failed', completed_at = clock_timestamp(), failure_code = 'fake_closed_work',
             lease_token = NULL, lease_expires_at = NULL
         WHERE id IN (SELECT delivery_id FROM public.mail_disposition_work WHERE id <> $1)
           AND status NOT IN ('succeeded', 'failed', 'manual_reconcile')",
    )
    .bind(receipt.work_id)
    .execute(pool)
    .await
    .expect("收口期 4 自己的工作");
    assert_eq!(take_state(pool, receipt.work_id).await, ("prepared".to_owned(), 0, 0, 0));

    let now = Timestamp::now();
    let phase4 = repository(&database)
        .await
        .claim_due("phase4-worker".to_owned(), now, now + Duration::minutes(5))
        .await
        .expect("期 4 领取");
    assert_eq!(phase4, None, "期 4 处置不得领走接管的 Take 工作");
    assert_eq!(take_state(pool, receipt.work_id).await, ("prepared".to_owned(), 0, 0, 0));

    let claim = claim_return_take(&store).await.expect("退件 Take 可领取");
    assert!(claim.validate());
    assert_eq!((claim.intake_id, claim.work.work_id), (receipt.intake_id, receipt.work_id));
    assert_eq!(claim.work.fence, 1);
    assert_eq!(take_state(pool, receipt.work_id).await, ("claimed".to_owned(), 1, 1, 0));
}

/// 正例：领取 → 当前领取重读 → 起写首次提交拿到许可；反例：旧 token 重读为空，起写之后租约过期也不再
/// 领取（零新 fence、零新 attempt、起写事实仍只有一条）。
#[tokio::test]
async fn postgres_consignment_returns_take_source_never_reclaims_a_started_take() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("维护账号");
    let store = Arc::new(database.store().await.expect("运行账号"));
    let pool = owner.postgres_pool();
    let rts = Rts { n: 2 };
    seed_original_rts(pool, rts).await;
    let receipt = service(&store).acquire(original_facts(rts)).await.expect("接管");
    seed_main_readiness(pool).await;

    let claim = claim_return_take(&store).await.expect("退件 Take 可领取");
    let lease = ReturnTakeLease {
        intake_id: claim.intake_id,
        attempt_id: claim.work.attempt_id,
        fence: claim.work.fence,
        claim_token: claim.work.claim_token,
    };
    let current = store.load_current_return_take(lease).await.expect("重读当前领取");
    assert_eq!(current.as_ref(), Some(&claim));
    let stale = ReturnTakeLease { claim_token: Uuid::new_v4(), ..lease };
    assert_eq!(store.load_current_return_take(stale).await.expect("重读旧领取"), None);

    let started = service(&store)
        .start_take(StartTakeInput {
            intake_id: claim.intake_id,
            work_id: claim.work.work_id,
            delivery_id: claim.work.delivery_id,
            attempt_id: claim.work.attempt_id,
            fence: claim.work.fence,
            claim_token: claim.work.claim_token,
            session_binding_digest: session_binding(pool).await,
            expected_manifest_digest: MANIFEST,
        })
        .await
        .expect("起写");
    assert!(matches!(started, WriteStart::FirstCommitted { .. }), "首次起写得到许可");

    sqlx::query(
        "UPDATE public.deliveries SET lease_expires_at = clock_timestamp() - interval '1 hour'
         WHERE id = $1",
    )
    .bind(claim.work.delivery_id)
    .execute(pool)
    .await
    .expect("让租约过期");
    assert_eq!(claim_return_take(&store).await, None, "已起写的 Take 不再领取");
    assert_eq!(take_state(pool, receipt.work_id).await, ("claimed".to_owned(), 1, 1, 1));
}
