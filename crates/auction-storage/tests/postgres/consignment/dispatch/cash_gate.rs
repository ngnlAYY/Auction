//! 寄售现金腿外发的同会话现金门（维护者 2026-11-25 决定）：起写事务按拍卖通用现金门核对发信会话里取得的
//! 新鲜资金证据——认证 UID 不是成交冻结的资金 UID、观察不新鲜或没有证据，一律整笔回滚、零许可、资金零痕迹；
//! 余额不足与拍卖同口径提交账户冻结、零许可、不写起写账本（裁定 2026-11-25），解冻后同命令可重试；
//! 满足时才 reserve → in_flight 并发出唯一许可。端到端：派发 executor 经生产的领取、
//! 衔接与起写把载体轮走到 `succeeded`、载体本金 spent；卖款轮经同一派发写口起写并由 Sent 证明结清 S。
//!
//! 只测核心规则：金额与资金、失败关闭与原子性（新增许可数、起写命令数）。传输是计数替身，不连接外部服务。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use auction_application::consignment::{
    ConsignmentError, ConsignmentFuture, DepositBusinessRejection, DispatchService, PostPermit,
    WriteStart,
};
use auction_application::settlement::{FundingCommitOutcome, FundingReconciliationReader};
use auction_domain::consignment::dispatch::{AttemptKey, DispatchRoundKey, FrozenDispatchIdentity};
use auction_jobs::consignment::{
    ConsignmentDispatchRuntime, DispatchExecution, DispatchRound, DispatchTransport, DispatchWork,
    PreparedDispatch, SentObservation,
};
use auction_time::{Duration as TimeDelta, Timestamp};
use sqlx::PgPool;
use tokio::sync::Notify;
use uuid::Uuid;

use super::super::contracts::fixture::{CONTRACT, OBLIGATION_2, OBLIGATION_3};
use super::super::deposit::support::{DELIVERY, succeeds};
use super::super::funding::fixture::reconcile_with;
use super::carrier_funding::{
    BALANCE, Carrier, Mode, OWN, UID, carrier, cash_evidence, ledger, own, send_writes,
};
use super::cash_rounds::{
    allocation, frozen, insert_round, release_for_dispatch, sent, start_round,
};
use super::fixture::{CONTENT, at, binding, uuid, work_store};
use super::worker_chain::{Chain, delivery_facts};

/// 成交冻结的卖款 S（与载体夹具的 isekai Sale 图一致）。
const SELLER_AMOUNT: i64 = 96;

async fn account_status(pool: &PgPool) -> String {
    sqlx::query_scalar("SELECT status FROM public.settlement_funding_accounts WHERE uid = $1")
        .bind(UID)
        .fetch_one(pool)
        .await
        .expect("账户状态")
}

#[tokio::test]
async fn postgres_consignment_dispatch_cash_gate_requires_fresh_evidence_of_the_frozen_uid() {
    let seeded = Carrier::purchased(Mode::Strict { own: OWN }).await;
    let pool = seeded.pool();
    let (attempt, token) = seeded.claim_and_register().await;
    let service = seeded.service();
    let before = ledger(pool).await;
    let now = Timestamp::now();
    let credential = binding();
    let evidence = |uid: &'static str, credits: i64, observed: Timestamp| {
        cash_evidence(pool, attempt, token, &credential, uid, Some(credits), observed)
    };

    // 反例：没有同会话证据、为另一租约取得的证据、认证 UID 不是冻结 UID、观察已过期——证据本身不成立，
    // 各自整笔回滚：
    // 零 POST 许可、零起写命令、资金零痕迹、账户不冻结。
    let missing = service.mark_write_started(attempt, token, Some(binding())).await;
    assert_eq!(missing.expect_err("缺证据"), ConsignmentError::Unavailable);
    let other_lease =
        cash_evidence(pool, attempt, Uuid::new_v4(), &credential, UID, Some(BALANCE), now).await;
    for (case, denied) in [
        ("另一租约", other_lease),
        ("UID 不符", evidence("7002", BALANCE, now).await),
        ("观察不新鲜", evidence(UID, BALANCE, now - TimeDelta::minutes(10)).await),
    ] {
        let outcome =
            service.mark_cash_write_started(attempt, token, Some(binding()), &denied).await;
        assert_eq!(outcome.expect_err(case), ConsignmentError::Unavailable, "{case}");
        assert_eq!(send_writes(pool, seeded.delivery_id).await, (0, 0, 0), "{case}：零许可");
        assert_eq!(ledger(pool).await, before, "{case}：资金零痕迹");
        assert_eq!(account_status(pool).await, "ready", "{case}：账户不冻结");
    }

    // 正例：同一尝试换上新鲜、足额、同 UID 的证据，得到唯一许可，载体本金同事务在途。
    let fresh = evidence(UID, BALANCE, Timestamp::now()).await;
    let started = service.mark_cash_write_started(attempt, token, Some(binding()), &fresh).await;
    assert!(matches!(started, Ok(WriteStart::FirstCommitted { .. })), "{started:?}");
    assert_eq!(send_writes(pool, seeded.delivery_id).await, (1, 1, 1));
    assert_eq!(carrier(pool).await, Some(("in_flight".to_owned(), 1, Some(seeded.delivery_id))));
}

/// 余额不足与拍卖通用现金门同口径：提交账户冻结、不写起写事实与账本回执、零许可，以确定的“不可发”回答；
/// 补足并经既有对账流程解冻后，同一命令拿到唯一许可。
#[tokio::test]
async fn postgres_consignment_dispatch_cash_shortfall_freezes_the_account_and_retries_after_reopen()
{
    let seeded = Carrier::purchased(Mode::Strict { own: OWN }).await;
    let pool = seeded.pool();
    let (attempt, token) = seeded.claim_and_register().await;
    let service = seeded.service();
    let credential = binding();

    // 观察到的余额低于该 UID 的未支出占用（自有 OWN）：账户冻结已提交，零许可、零起写命令。
    let short =
        cash_evidence(pool, attempt, token, &credential, UID, Some(OWN - 1), Timestamp::now())
            .await;
    let denied = service.mark_cash_write_started(attempt, token, Some(binding()), &short).await;
    assert_eq!(account_status(pool).await, "frozen", "缺口冻结随起写事务提交");
    assert_eq!(
        denied.expect_err("余额不足"),
        ConsignmentError::Deposit(DepositBusinessRejection::ActionNotAllowed)
    );
    assert_eq!(send_writes(pool, seeded.delivery_id).await, (0, 0, 0), "零许可、零账本回执");
    assert_eq!(carrier(pool).await, None, "零占用");

    // 冻结期间即使证据足额也不发。
    let still =
        cash_evidence(pool, attempt, token, &credential, UID, Some(BALANCE), Timestamp::now())
            .await;
    assert!(
        service.mark_cash_write_started(attempt, token, Some(binding()), &still).await.is_err()
    );
    assert_eq!(send_writes(pool, seeded.delivery_id).await, (0, 0, 0));

    // 补足后按既有流程解冻：取真实预览并显式对账，账户回到 ready。共用夹具的存入 Take 投递外壳省略了
    // 生产载荷里的 credits / cod（生产 Take 工作会写这两项），预览会判为资产无界；按生产形状补上，不放宽约束
    // （与 `returns/consume.rs` 的对账正例同一处理）。
    succeeds(
        pool,
        &format!(
            "UPDATE public.deliveries SET payload = payload || '{{\"credits\":0,\"cod\":0}}'::jsonb
             WHERE id = '{DELIVERY}';"
        ),
    )
    .await;
    let store = seeded.runtime.clone();
    let preview = store.preview_funding(UID.to_owned()).await.expect("资金预览");
    assert!(preview.blockers.is_empty(), "{:?}", preview.blockers);
    let reopened = reconcile_with(&store, pool, UID, preview.account_version, preview.digest).await;
    assert!(matches!(reopened, Ok(FundingCommitOutcome::Committed(_))), "{reopened:?}");
    assert_eq!(account_status(pool).await, "ready");
    assert_eq!(own(pool).await, OWN, "解冻不动原 UID 自有");

    // 同一命令（同尝试、同租约、同凭据）带新鲜足额证据：唯一许可。
    let fresh =
        cash_evidence(pool, attempt, token, &credential, UID, Some(BALANCE), Timestamp::now())
            .await;
    let started = service.mark_cash_write_started(attempt, token, Some(binding()), &fresh).await;
    assert!(matches!(started, Ok(WriteStart::FirstCommitted { .. })), "{started:?}");
    assert_eq!(send_writes(pool, seeded.delivery_id).await, (1, 1, 1));
}

/// 计数替身：现金轮在准备时给出同会话资金证据（新鲜、足额、冻结 UID），执行记一次 POST 并给出 Sent 证明。
struct CashRemote {
    pool: PgPool,
    posts: AtomicUsize,
}

impl DispatchTransport for CashRemote {
    fn prepare<'a>(
        &'a self,
        work: &'a DispatchWork,
        attempt: AttemptKey,
        claim_token: Uuid,
    ) -> ConsignmentFuture<'a, Option<PreparedDispatch>> {
        Box::pin(async move {
            let cash_evidence = if work.frozen.credits > 0 {
                let credits = Some(BALANCE);
                let now = Timestamp::now();
                Some(
                    cash_evidence(&self.pool, attempt, claim_token, &binding(), UID, credits, now)
                        .await,
                )
            } else {
                None
            };
            Ok(Some(PreparedDispatch {
                credential: binding(),
                cash_evidence,
                handle: Box::new(()),
            }))
        })
    }

    fn execute(
        &self,
        _prepared: PreparedDispatch,
        _permit: PostPermit,
    ) -> ConsignmentFuture<'_, DispatchExecution> {
        let post = self.posts.fetch_add(1, Ordering::SeqCst) + 1;
        Box::pin(async move {
            Ok(DispatchExecution::Proven(SentObservation {
                sent_mid: format!("910000{post}"),
                observed_at: at("2026-01-01T05:00:00Z"),
                validator_version: "fake-validator".to_owned(),
                evidence_digest: [7; 32],
            }))
        })
    }
}

#[tokio::test]
async fn postgres_consignment_dispatch_cash_rounds_reach_succeeded_and_spend_the_funds() {
    let seeded = Carrier::purchased(Mode::Strict { own: OWN }).await;
    let pool = seeded.pool();
    let remote = Arc::new(CashRemote { pool: pool.clone(), posts: AtomicUsize::new(0) });
    let service = Arc::new(DispatchService::new(Arc::new(seeded.runtime.clone())));
    let executor = ConsignmentDispatchRuntime {
        work: Arc::new(Chain {
            store: seeded.runtime.clone(),
            work: work_store(&seeded.runtime, CONTENT),
        }),
        service: service.clone(),
        transport: remote.clone(),
        worker_id: "fake-worker".to_owned(),
        lease_duration: Duration::from_secs(300),
        wakeup: Arc::new(Notify::new()),
        paused: Arc::new(|| false),
    };

    // 载体轮：生产领取 → 衔接 → 起写（同会话证据过现金门）→ 唯一一次 POST → Sent 证据，投递 succeeded、
    // 载体本金 spent；再跑一轮零第二次 POST。
    assert_eq!(executor.run_once().await.expect("载体轮"), DispatchRound::Sent);
    assert_eq!(remote.posts.load(Ordering::SeqCst), 1);
    assert_eq!(delivery_facts(pool, seeded.delivery_id).await, ("succeeded".to_owned(), 1, 1));
    assert_eq!(carrier(pool).await, Some(("spent".to_owned(), 1, Some(seeded.delivery_id))));
    assert_eq!(executor.run_once().await.expect("再跑"), DispatchRound::Idle);
    assert_eq!(remote.posts.load(Ordering::SeqCst), 1);

    // 卖款轮：用途里有 S，起写同事务占用 S，Sent 证明后 spent、投递 succeeded。
    succeeds(
        pool,
        &format!(
            "INSERT INTO public.settlement_funding_allocations
                (id, uid, purpose, obligation_id, consignment_contract_id, amount)
             VALUES (gen_random_uuid(), '{UID}', 'seller', '{OBLIGATION_3}', '{CONTRACT}',
                     {SELLER_AMOUNT});"
        ),
    )
    .await;
    let key = DispatchRoundKey {
        contract_id: uuid(CONTRACT),
        obligation_id: uuid(OBLIGATION_3),
        round: 1,
    };
    let seller = FrozenDispatchIdentity {
        rights_holder_identity: "FakeSellerOne".to_owned(),
        ..frozen(&key, "consignment_seller_payout", SELLER_AMOUNT, 0)
    };
    let delivery = insert_round(pool, &key, &seller, "FakeSellerOne", None).await;
    release_for_dispatch(pool, OBLIGATION_3, OBLIGATION_2).await;
    let (attempt, started) = start_round(&seeded, key, seller, delivery, binding()).await;
    assert!(matches!(started, Ok(WriteStart::FirstCommitted { .. })), "{started:?}");
    assert_eq!(
        allocation(pool, "seller").await,
        Some(("in_flight".to_owned(), SELLER_AMOUNT, Some(delivery)))
    );
    assert!(!service.record_evidence(sent(attempt, "9105")).await.expect("Sent").replayed);
    assert_eq!(
        allocation(pool, "seller").await,
        Some(("spent".to_owned(), SELLER_AMOUNT, Some(delivery)))
    );
    assert_eq!(delivery_facts(pool, delivery).await, ("succeeded".to_owned(), 1, 1));
}

/// 纵深防御：轮冻结了 Credits 而义务不是现金腿（这里是原件交割义务）时，没有资金门可走，起写失败关闭：
/// 零许可、零起写账本、资金零痕迹。
#[tokio::test]
async fn postgres_consignment_dispatch_credits_on_a_non_cash_obligation_fail_closed() {
    let seeded = Carrier::purchased(Mode::Strict { own: OWN }).await;
    let pool = seeded.pool();
    let key = DispatchRoundKey {
        contract_id: uuid(CONTRACT),
        obligation_id: uuid(OBLIGATION_2),
        round: 1,
    };
    let handoff = frozen(&key, "consignment_equipment_handoff", 5, 0);
    let delivery = insert_round(pool, &key, &handoff, "FakeSellerTwo", None).await;
    let before = ledger(pool).await;

    let (_attempt, started) = start_round(&seeded, key, handoff, delivery, binding()).await;

    assert!(started.is_err(), "{started:?}");
    assert_eq!(send_writes(pool, delivery).await, (0, 0, 0), "零许可、零起写账本");
    assert_eq!(ledger(pool).await, before, "资金零痕迹");
}
