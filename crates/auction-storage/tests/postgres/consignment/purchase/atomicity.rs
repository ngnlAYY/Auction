//! 原子性与外部前置：图写入末项失败整笔回滚且可原样重试；资金预检的确定拒绝入账，未知读取不入账。
//!
//! 保护不变量 3（同事务）、4（外部前置失败不留半个成交）、6（失败关闭）。

use auction_application::consignment::{ConsignmentError, PurchaseOutcome, PurchaseStore};
use auction_domain::consignment::purchase::PurchaseRejection;

use super::fixture::{BUYER, Fixture, Funding, PRICE, command_id, purchase};

/// 注入图写入的最后一项（休眠的退款履约行）失败：整笔回滚，挂单仍 available，零合同零义务零首轮，
/// 命令 ID 未被占用；撤掉注入后同命令 ID 原样重试成功。
#[tokio::test]
async fn postgres_consignment_purchase_failing_last_graph_row_rolls_everything_back() {
    let fixture = Fixture::available().await;
    sqlx::raw_sql(sqlx::AssertSqlSafe(
        "CREATE FUNCTION public.fake_fail_last_fulfillment() RETURNS trigger
         LANGUAGE plpgsql AS $$
         BEGIN
             IF NEW.dormant THEN RAISE EXCEPTION 'fake injected failure'; END IF;
             RETURN NEW;
         END $$;
         CREATE TRIGGER fake_fail_last_fulfillment
         BEFORE INSERT ON public.settlement_obligation_fulfillments
         FOR EACH ROW EXECUTE FUNCTION public.fake_fail_last_fulfillment();"
            .to_owned(),
    ))
    .execute(fixture.pool())
    .await
    .expect("注入失败触发器");

    let failed = fixture.repository.purchase(purchase(BUYER, 1, 0x11)).await;
    assert!(failed.is_err(), "末项失败时购买必须失败：{failed:?}");
    assert_eq!(fixture.listing().await, ("available".to_owned(), 5, PRICE));
    assert_eq!(fixture.position_state().await, "in_custody");
    for table in [
        "consignment_contracts",
        "consignment_sales",
        "settlement_obligations",
        "settlement_obligation_fulfillments",
        "consignment_dispatch_rounds",
    ] {
        assert_eq!(fixture.count(table).await, 0, "{table} 不应留行");
    }
    let leaked: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.consignment_commands WHERE command_id = $1",
    )
    .bind(command_id(1).as_uuid())
    .fetch_one(fixture.pool())
    .await
    .expect("命令行");
    assert_eq!(leaked, 0, "回滚后命令 ID 不被占用");

    sqlx::raw_sql(sqlx::AssertSqlSafe(
        "DROP TRIGGER fake_fail_last_fulfillment ON public.settlement_obligation_fulfillments;"
            .to_owned(),
    ))
    .execute(fixture.pool())
    .await
    .expect("撤掉注入");
    let retried = fixture.repository.purchase(purchase(BUYER, 1, 0x11)).await.expect("重试");
    assert!(matches!(retried, PurchaseOutcome::Purchased(_)), "{retried:?}");
    assert_eq!(fixture.count("consignment_contracts").await, 1);
}

/// 资金预检确定不可用：确定拒绝入账（503 funding_unavailable），无成交事实，重放仍是同一拒绝；
/// 预检读取失败不是确定拒绝：不写回执、不占命令 ID，之后同命令 ID 可正常成交。
#[tokio::test]
async fn postgres_consignment_purchase_funding_preflight_rejects_only_when_determined() {
    let fixture = Fixture::available().await;
    fixture.preflight.set(Funding::Unavailable);
    let rejected = fixture.repository.purchase(purchase(BUYER, 1, 0x11)).await.expect("拒绝");
    assert!(
        matches!(rejected, PurchaseOutcome::Rejected(PurchaseRejection::FundingUnavailable)),
        "{rejected:?}"
    );
    let (status, code): (i16, String) = sqlx::query_as(
        "SELECT result_status, result_code FROM public.consignment_commands WHERE command_id = $1",
    )
    .bind(command_id(1).as_uuid())
    .fetch_one(fixture.pool())
    .await
    .expect("拒绝回执");
    assert_eq!((status, code.as_str()), (503, "funding_unavailable"));
    assert_eq!(fixture.count("consignment_contracts").await, 0);
    assert_eq!(fixture.listing().await, ("available".to_owned(), 5, PRICE));

    // 资金恢复后，同命令 ID 重放仍是原拒绝，不会悄悄成交。
    fixture.preflight.set(Funding::Ready);
    let replay = fixture.repository.purchase(purchase(BUYER, 1, 0x11)).await.expect("重放");
    assert!(
        matches!(replay, PurchaseOutcome::Rejected(PurchaseRejection::FundingUnavailable)),
        "{replay:?}"
    );
    assert_eq!(fixture.count("consignment_contracts").await, 0);

    // 读取失败：未知，不入账。
    fixture.preflight.set(Funding::ReadFails);
    let unknown = fixture.repository.purchase(purchase(BUYER, 2, 0x12)).await;
    assert!(matches!(unknown, Err(ConsignmentError::Unavailable)), "{unknown:?}");
    let recorded: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.consignment_commands WHERE command_id = $1",
    )
    .bind(command_id(2).as_uuid())
    .fetch_one(fixture.pool())
    .await
    .expect("命令行");
    assert_eq!(recorded, 0);
    fixture.preflight.set(Funding::Ready);
    let bought = fixture.repository.purchase(purchase(BUYER, 2, 0x12)).await.expect("成交");
    assert!(matches!(bought, PurchaseOutcome::Purchased(_)), "{bought:?}");
}
