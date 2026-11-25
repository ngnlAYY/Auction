//! record_only 收款：同一协议付款门、无远端付款明细不得转 satisfied、载体本金进不可支用的核销子账。

use auction_application::settlement::FundingError;
use auction_domain::consignment::funding::FundingMode;
use auction_storage::DatabaseStore;

use super::cod_fixture::{GROSS, Shape, allocation_amount, certify, ledger, seed};
use crate::support::TestDatabase;

const RECORD_ONLY: Shape = Shape { record_only: true, ..Shape::STRICT };

fn record_only_command(byte: u8) -> auction_application::settlement::CertifyCod {
    let mut command = certify(byte, None);
    command.mode = FundingMode::RecordOnly;
    command.cash = None;
    command
}

async fn env(shape: Shape) -> (TestDatabase, DatabaseStore, DatabaseStore) {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let store = database.store().await.expect("以运行账号连接");
    seed(owner.postgres_pool(), shape).await;
    (database, owner, store)
}

#[tokio::test]
async fn postgres_consignment_funding_record_only_certify_requires_remote_payment_detail() {
    // 正例：有远端付款明细，本金 1 进合同的核销子账，不归个人自有，也不写补偿池。
    let (_database, owner, store) = env(RECORD_ONLY).await;
    let pool = owner.postgres_pool();
    let receipt = store.test_certify_cod(record_only_command(2)).await.expect("record_only 认证");
    assert_eq!((receipt.split.opening_included(), receipt.split.new_issue()), (0, GROSS));
    assert_eq!(allocation_amount(pool, "carrier_writeoff", true).await, 1);
    assert_eq!(allocation_amount(pool, "proprietary", false).await, 0);
    assert_eq!(allocation_amount(pool, "seller", true).await, 96);
    let after = ledger(pool).await;
    assert_eq!((after.compensation_pool, after.compensation_rows), (0, 0));
    assert_eq!(after.payment_state, "satisfied");

    // 反例：账户模式与命令声明的模式不一致，不换模式、不放行。
    let (_database, owner, store) = env(RECORD_ONLY).await;
    let before = ledger(owner.postgres_pool()).await;
    assert!(matches!(store.test_certify_cod(certify(2, None)).await, Err(FundingError::Conflict)));
    assert_eq!(ledger(owner.postgres_pool()).await, before);

    // 反例：record_only 没有远端付款明细，整笔回滚，不留孤立 satisfied 或下游 ready。
    let (_database, owner, store) =
        env(Shape { record_only: true, remote_payment: false, ..Shape::STRICT }).await;
    let before = ledger(owner.postgres_pool()).await;
    assert!(matches!(
        store.test_certify_cod(record_only_command(2)).await,
        Err(FundingError::Unproven)
    ));
    assert_eq!(ledger(owner.postgres_pool()).await, before);
}
