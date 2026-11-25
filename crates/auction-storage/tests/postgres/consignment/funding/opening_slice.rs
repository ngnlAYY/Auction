//! opening 份额登记：同一份额只能封存给一个实收业务键，同键重放读原结果。

use auction_application::settlement::FundingError;
use sqlx::PgPool;

use super::cod_fixture::{OPENING_ENTRY, Shape, ledger, register, seed};
use crate::support::TestDatabase;

async fn slice_rows(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM public.consignment_opening_slices")
        .fetch_one(pool)
        .await
        .expect("读取份额数")
}

#[tokio::test]
async fn postgres_consignment_funding_opening_slices_belong_to_one_receipt_key() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    let store = database.store().await.expect("以运行账号连接");
    seed(pool, Shape::STRICT).await;

    // 正例：封存 [0,60)；同命令与换命令重放都读原结果，不新增份额。
    let sealed = store
        .test_register_opening_attribution(register(1, &[(OPENING_ENTRY, 0, 60)]))
        .await
        .expect("封存份额");
    assert_eq!((sealed.included, sealed.replayed), (60, false));
    let same = store
        .test_register_opening_attribution(register(1, &[(OPENING_ENTRY, 0, 60)]))
        .await
        .expect("同命令重放");
    let other = store
        .test_register_opening_attribution(register(2, &[(OPENING_ENTRY, 0, 60)]))
        .await
        .expect("换命令重放");
    assert!(same.replayed && other.replayed);
    assert_eq!(same.attribution_id, sealed.attribution_id);
    assert_eq!(slice_rows(pool).await, 1);
    // 回执与审计同事务写入：首次登记一行；同命令重放不再写；换命令是另一条被接受的命令，各多一行。
    let accepted = ledger(pool).await;
    assert_eq!((accepted.receipt_commands, accepted.audits), (2, 2));
    let (status, disposition, target_kind, amount): (i16, String, String, i64) = sqlx::query_as(
        "SELECT result_status, result_body->>'disposition', target_kind,
                (result_body->>'attributed_amount')::bigint
         FROM public.consignment_commands
         WHERE operation = 'register_opening_attribution' AND command_id = $1",
    )
    .bind(super::cod_fixture::mutation(1).command_id)
    .fetch_one(pool)
    .await
    .expect("读取登记回执");
    assert_eq!(
        (status, disposition.as_str(), target_kind.as_str(), amount),
        (200, "attribution_registered", "receipt_key", 60)
    );

    // 反例：同键换份额、另一实收业务键抢同一区间、区间越过原发行额，都整笔拒绝，不留孤立份额。
    let mut other_key = register(3, &[(OPENING_ENTRY, 50, 80)]);
    other_key.receipt.payment_mail_id = "9002".to_owned();
    assert!(matches!(
        store.test_register_opening_attribution(other_key).await,
        Err(FundingError::Conflict)
    ));
    assert!(matches!(
        store.test_register_opening_attribution(register(4, &[(OPENING_ENTRY, 0, 50)])).await,
        Err(FundingError::Conflict)
    ));
    let mut beyond = register(5, &[(OPENING_ENTRY, 190, 260)]);
    beyond.receipt.payment_mail_id = "9003".to_owned();
    assert!(matches!(
        store.test_register_opening_attribution(beyond).await,
        Err(FundingError::Unproven)
    ));
    assert_eq!(slice_rows(pool).await, 1);
    // 同命令 ID 异请求：冲突，不入账。
    let mut changed = register(1, &[(OPENING_ENTRY, 0, 60)]);
    changed.mutation.request_digest = [9; 32];
    assert!(matches!(
        store.test_register_opening_attribution(changed).await,
        Err(FundingError::Conflict)
    ));
    let rejected = ledger(pool).await;
    assert_eq!(
        (rejected.receipt_commands, rejected.audits),
        (2, 2),
        "拒绝的登记不入账：无回执、无审计"
    );

    // 正例：紧邻的不相交区间可以封存给另一实收业务键。
    let mut adjacent = register(6, &[(OPENING_ENTRY, 60, 100)]);
    adjacent.receipt.payment_mail_id = "9002".to_owned();
    store.test_register_opening_attribution(adjacent).await.expect("不相交区间");
    assert_eq!(slice_rows(pool).await, 2);
}
