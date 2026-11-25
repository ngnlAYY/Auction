//! 资金预览的寄售段：本 UID 的现金退件阻断只归本 UID，异 UID 仍走原对账门；全局存在 record_only 时预览标拒绝，
//! 现金 opening 写端同样拒绝；账户模式两个方向都不可互换。

use auction_application::settlement::{
    FundingCommitOutcome, FundingError, FundingReconciliationReader,
};
use sqlx::PgPool;

use super::cod_fixture::{Shape, seed};
use super::fixture::{UID, ledger_counts, reconcile_with, seed_unconsumed_cash_return_block};
use crate::support::TestDatabase;

const OTHER_UID: &str = "7002";
const RECORD_ONLY: Shape = Shape { record_only: true, ..Shape::STRICT };

async fn blockers(store: &auction_storage::DatabaseStore, uid: &str) -> Vec<String> {
    store.preview_funding(uid.to_owned()).await.expect("资金预览").blockers
}

#[tokio::test]
async fn postgres_consignment_funding_preview_lists_block_for_own_uid_only() {
    // A 分别为 ready 与 frozen：两种受保护状态下 B 都沿原对账门处理（D9 §11.1 ②）。
    for status in ["ready", "frozen"] {
        preview_own_uid_only(status).await;
    }
}

async fn preview_own_uid_only(status: &str) {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool: &PgPool = owner.postgres_pool();
    let store = database.store().await.expect("以运行账号连接");
    let (intake, _) = seed_unconsumed_cash_return_block(pool, &store).await;
    sqlx::query("UPDATE public.settlement_funding_accounts SET status = $2 WHERE uid = $1")
        .bind(UID)
        .bind(status)
        .execute(pool)
        .await
        .expect("A 为受保护账户");

    // 正例：本 UID 的预览列出这笔寄售退件阻断。
    let own = blockers(&store, UID).await;
    let expected = format!("consignment_cash_return:{}:cash_refund:unconsumed", intake.intake());
    assert!(own.contains(&expected), "本 UID 预览必须列出阻断：{own:?}");

    // 反例：B 的预览不列 A 的寄售退件阻断，但仍列原有的异 UID 受保护账户门。
    let other = blockers(&store, OTHER_UID).await;
    assert!(
        !other.iter().any(|blocker| blocker.starts_with("consignment_cash_return:")),
        "异 UID 预览不得列出 A 的阻断：{other:?}"
    );
    assert!(
        other.contains(&"other_uid_protected_account_requires_reconciliation".to_owned()),
        "A 为 {status} 时异 UID 仍须通过原对账门：{other:?}"
    );
}

#[tokio::test]
async fn postgres_consignment_funding_preview_rejects_when_record_only_account_exists() {
    // 反例：全局存在 record_only 账户（A），B 即使没有本 UID 寄售阻断，现金 opening 预览也标拒绝。
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let store = database.store().await.expect("以运行账号连接");
    seed(owner.postgres_pool(), RECORD_ONLY).await;
    let other = blockers(&store, OTHER_UID).await;
    assert!(other.contains(&"record_only_account_blocks_cash_opening".to_owned()), "{other:?}");

    // 正例对照：没有 record_only 账户时不出现该拒绝码。
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let store = database.store().await.expect("以运行账号连接");
    seed(owner.postgres_pool(), Shape::STRICT).await;
    let other = blockers(&store, OTHER_UID).await;
    assert!(!other.contains(&"record_only_account_blocks_cash_opening".to_owned()), "{other:?}");
}

/// 该 UID 的账户行数。
async fn accounts_of(pool: &PgPool, uid: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM public.settlement_funding_accounts WHERE uid = $1")
        .bind(uid)
        .fetch_one(pool)
        .await
        .expect("读取账户数")
}

/// 写端（D9 §11.1 ③）：全局存在 record_only 账户时，B 即使没有本 UID 寄售阻断、持当前预览的摘要与版本，
/// 现金 opening 最终写也被拒——零 opening source、零分录、不建账户（边界不动）。
/// 对照：没有 record_only 账户时同一提交越过这道门，停在其后的原对账门（`Unproven`）。
///
/// 不变量 6：record_only 与严格现金不能在同一部署内混用，旧 strict 入口不能借新开户形成混用。
#[tokio::test]
async fn postgres_consignment_funding_cash_opening_is_rejected_while_a_record_only_account_exists()
{
    for (shape, record_only) in [(RECORD_ONLY, true), (Shape::STRICT, false)] {
        let database = TestDatabase::create().await.expect("克隆模板库");
        let owner = database.maintenance_store().await.expect("以维护账号连接");
        let pool = owner.postgres_pool();
        let store = database.store().await.expect("以运行账号连接");
        seed(pool, shape).await;
        let preview = store.preview_funding(OTHER_UID.to_owned()).await.expect("资金预览");
        let outcome =
            reconcile_with(&store, pool, OTHER_UID, preview.account_version, preview.digest).await;
        if record_only {
            assert!(
                matches!(outcome, Ok(FundingCommitOutcome::RolledBack(FundingError::Conflict))),
                "{outcome:?}"
            );
        } else {
            assert!(
                matches!(outcome, Ok(FundingCommitOutcome::RolledBack(FundingError::Unproven))),
                "{outcome:?}"
            );
        }
        assert_eq!(ledger_counts(pool, OTHER_UID).await, (0, 0), "零 opening source、零分录");
        assert_eq!(accounts_of(pool, OTHER_UID).await, 0, "不建账户，边界不动");
    }
}

/// 每个账户的 (UID, 模式, 策略版本, 状态, 版本, 分配合计)。
async fn account_snapshot(pool: &PgPool) -> Vec<(String, String, i64, String, i64, i64)> {
    sqlx::query_as(
        "SELECT a.uid, a.accounting_mode, a.policy_version, a.status, a.state_version,
                COALESCE((SELECT sum(amount) FROM public.settlement_funding_allocations AS x
                          WHERE x.uid = a.uid), 0)::bigint
         FROM public.settlement_funding_accounts AS a ORDER BY a.uid",
    )
    .fetch_all(pool)
    .await
    .expect("读取账户快照")
}

/// 断言语句被 `settlement_funding_policy_immutable` 触发器拒绝（不是 CHECK 或唯一索引）。
async fn rejected_by_policy_trigger(pool: &PgPool, sql: &'static str) {
    let error = sqlx::query(sql).execute(pool).await.expect_err("模式互换必须被拒");
    let db_error = error.as_database_error().expect("期望服务端错误");
    assert_eq!(db_error.code().as_deref(), Some("23514"), "{sql}");
    assert_eq!(db_error.constraint(), None, "应是触发器而不是 CHECK：{sql}");
    assert!(db_error.message().contains("funding policy requires a dedicated migration"), "{sql}");
}

/// D9 §11.1 ④：已建 record_only 改 strict_cash、已建 strict_cash 改 record_only 都被拒，原模式与余额不变。
/// 以维护账号直接改列（运行账号连写入口都没有），触达的就是最后一道触发器。
///
/// 不变量 6、10：受限本金不能借换模式成为自有现金；账户模式一经建立不可切换。
#[tokio::test]
async fn postgres_consignment_funding_policy_mode_cannot_be_switched_either_way() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    seed(pool, RECORD_ONLY).await;
    sqlx::query(
        "INSERT INTO public.settlement_funding_accounts (uid, status, accounting_mode)
         VALUES ($1, 'ready', 'strict_cash')",
    )
    .bind(OTHER_UID)
    .execute(pool)
    .await
    .expect("另建严格现金账户");
    let before = account_snapshot(pool).await;
    assert_eq!(
        before.iter().map(|row| (row.0.as_str(), row.1.as_str())).collect::<Vec<_>>(),
        vec![(UID, "record_only"), (OTHER_UID, "strict_cash")]
    );

    rejected_by_policy_trigger(
        pool,
        "UPDATE public.settlement_funding_accounts SET accounting_mode = 'strict_cash'
         WHERE uid = '7001'",
    )
    .await;
    rejected_by_policy_trigger(
        pool,
        "UPDATE public.settlement_funding_accounts SET accounting_mode = 'record_only'
         WHERE uid = '7002'",
    )
    .await;
    assert_eq!(account_snapshot(pool).await, before, "原模式、版本与余额不变");
}
