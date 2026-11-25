//! 处罚阈值：达标在同一事务写齐处罚、构成项、新购阻断与唯一账号工作；重复事实不多计；失败整笔回滚。

use auction_domain::consignment::refusal::Qualification;
use auction_storage::DatabaseStore;

use super::super::contracts::fixture::{OBLIGATION_1, OBLIGATION_3};
use super::super::deposit::support::succeeds;
use super::super::ledger136::fixture::{
    Intake, PAY_DELIVERY, PAY_MID, REFUND_DELIVERY, REFUND_MID, original_dispatch_sql,
};
use super::fixture::{
    LEGS, buyer, command, count, good_evidence, seed_base, seed_intakes, seller, submission, submit,
};
use crate::support::TestDatabase;

async fn gate(store: &DatabaseStore, n: u128) -> bool {
    store.test_purchase_gate(command(900 + n), buyer(), seller(), None).await.expect("阻断读")
}

#[tokio::test]
async fn postgres_consignment_refusal_threshold_commits_whole_sanction_and_counts_once() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let owner = owner.postgres_pool();
    let store = database.store().await.expect("以运行账号连接");
    seed_base(owner).await;
    seed_intakes(owner, 4).await;

    // 未达阈值：只有合格拒收，没有处罚、没有账号工作、不阻断购买。
    for n in 1..=2 {
        let outcome = submit(&store, n).await;
        assert_eq!(outcome.qualification, Qualification::Qualified);
        assert!(outcome.sanction.is_none());
    }
    assert!(!gate(&store, 1).await);
    assert_eq!(count(owner, "consignment_sanctions").await, 0);

    // 同一事实重复提交：重放，不多计，也不提前凑够阈值。
    let replay = submit(&store, 2).await;
    assert!(replay.replayed);
    assert_eq!(count(owner, "consignment_refusals").await, 2);
    assert!(!gate(&store, 2).await);

    // 第三份合格拒收达阈值：处罚、三个构成项、唯一账号工作同事务提交，新购被阻断。
    let third = submit(&store, 3).await;
    let sanction = third.sanction.expect("第三份应触发处罚");
    assert_eq!(count(owner, "consignment_sanctions").await, 1);
    assert_eq!(count(owner, "consignment_sanction_members").await, 3);
    assert_eq!(count(owner, "consignment_account_work").await, 1);
    let state: String =
        sqlx::query_scalar("SELECT state FROM public.consignment_sanctions WHERE id = $1")
            .bind(sanction.sanction_id)
            .fetch_one(owner)
            .await
            .expect("处罚状态");
    assert_eq!(state, "pending");
    assert!(gate(&store, 3).await);

    // 3 到 4：同周期不再形成第二项处罚、第二份账号工作，构成项仍是原三份。
    let fourth = submit(&store, 4).await;
    assert_eq!(fourth.qualification, Qualification::Qualified);
    assert!(fourth.sanction.is_none());
    assert_eq!(count(owner, "consignment_refusals").await, 4);
    assert_eq!(count(owner, "consignment_sanctions").await, 1);
    assert_eq!(count(owner, "consignment_sanction_members").await, 3);
    assert_eq!(count(owner, "consignment_account_work").await, 1);
}

#[tokio::test]
async fn postgres_consignment_refusal_second_crossing_in_same_cycle_is_recorded_without_second_sanction()
 {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let owner = owner.postgres_pool();
    let store = database.store().await.expect("以运行账号连接");
    seed_base(owner).await;
    // 生产默认阈值 3：同一合同再借付款腿的首派建三个接管行（腿身份按义务区分），共六份合格拒收。
    succeeds(owner, &original_dispatch_sql()).await;
    seed_intakes(owner, 3).await;
    for n in 4..=6 {
        let intake = Intake { n, leg: LEGS[usize::try_from(n).expect("n") - 4] };
        let sql = intake
            .sql()
            .replace(OBLIGATION_3, OBLIGATION_1)
            .replace(REFUND_DELIVERY, PAY_DELIVERY)
            .replace(REFUND_MID, PAY_MID);
        succeeds(owner, &sql).await;
    }

    // 第 1 到 3 份达阈值形成处罚；第 4、5 份池里没有足够的未用构成项，不处罚。
    assert!(submit(&store, 1).await.sanction.is_none());
    assert!(submit(&store, 2).await.sanction.is_none());
    assert!(submit(&store, 3).await.sanction.is_some());
    assert!(submit(&store, 4).await.sanction.is_none());
    assert!(submit(&store, 5).await.sanction.is_none());

    // 第 6 份与池中未用的第 4、5 份又凑够阈值：同周期已有处罚，正常记录且不再建第二项处罚。
    let sixth = submit(&store, 6).await;
    assert_eq!(sixth.qualification, Qualification::Qualified);
    assert!(sixth.sanction.is_none());
    assert_eq!(count(owner, "consignment_refusals").await, 6);
    assert_eq!(count(owner, "consignment_sanctions").await, 1);
    assert_eq!(count(owner, "consignment_sanction_members").await, 3);
    assert_eq!(count(owner, "consignment_account_work").await, 1);
}

#[tokio::test]
async fn postgres_consignment_refusal_failed_sanction_step_rolls_back_everything() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let owner = owner.postgres_pool();
    let store = database.store().await.expect("以运行账号连接");
    seed_base(owner).await;
    seed_intakes(owner, 3).await;
    submit(&store, 1).await;
    submit(&store, 2).await;
    let before = (
        count(owner, "consignment_return_facts").await,
        count(owner, "consignment_return_fact_verdicts").await,
        count(owner, "consignment_refusals").await,
    );

    // 故障注入：处罚链最后一步（账号工作）失败，之前写的事实、裁决、拒收、处罚、构成项必须一起回滚。
    succeeds(
        owner,
        "CREATE FUNCTION public.test_reject_account_work() RETURNS trigger LANGUAGE plpgsql AS
         $f$ BEGIN RAISE EXCEPTION 'test_account_work_failure' USING ERRCODE = 'P0001'; END $f$;
         CREATE TRIGGER test_reject_account_work BEFORE INSERT ON public.consignment_account_work
         FOR EACH ROW EXECUTE FUNCTION public.test_reject_account_work();",
    )
    .await;
    let failed = store
        .test_refusal_submit(command(3), &[buyer()], &submission(3, good_evidence()), None)
        .await;
    assert!(failed.is_err(), "账号工作写失败应使整笔失败");
    assert_eq!(count(owner, "consignment_return_facts").await, before.0);
    assert_eq!(count(owner, "consignment_return_fact_verdicts").await, before.1);
    assert_eq!(count(owner, "consignment_refusals").await, before.2);
    assert_eq!(count(owner, "consignment_sanctions").await, 0);
    assert_eq!(count(owner, "consignment_sanction_members").await, 0);
    assert!(!gate(&store, 1).await, "回滚后不得残留新购阻断");

    // 移除故障后同一提交成功：证明回滚没有留下会挡住重试的半态。
    succeeds(owner, "DROP TRIGGER test_reject_account_work ON public.consignment_account_work;")
        .await;
    let retried = submit(&store, 3).await;
    assert!(retried.sanction.is_some());
    assert_eq!(count(owner, "consignment_account_work").await, 1);
}
