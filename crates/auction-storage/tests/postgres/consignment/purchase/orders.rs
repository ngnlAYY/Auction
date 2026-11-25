//! 订单读侧与订单阶段事件的 PG18 测试：本人读取按角色省略金额、非本人同形为空，
//! 订单阶段事件与成交事实同事务提交、注入失败时两者一并回滚。
//!
//! 保护不变量 3（事件与事实同事务）、7（非本人不可见、金额按角色省略）。

use auction_application::consignment::{
    ConsignmentError, ListingPageWindow, OrderQuery, OrderRole, OwnedOrderRead,
    PublicCatalogReadV2, PublicListingQuery, PurchaseOutcome, PurchaseStore,
};
use auction_domain::consignment::ListingState;
use auction_domain::consignment::{
    ListingPrice, OrderDeliveryStage, OrderPaymentStage, OrderState,
};
use uuid::Uuid;

use super::super::deposit::support::USER_1;
use super::fixture::{BUYER, Fixture, PRICE, USER_3, purchase, user};

/// 完成一次购买并返回合同 ID。
async fn purchased(fixture: &Fixture) -> Uuid {
    let outcome = fixture.repository.purchase(purchase(BUYER, 1, 0x11)).await.expect("购买");
    let PurchaseOutcome::Purchased(receipt) = outcome else {
        panic!("夹具购买必须成交：{outcome:?}");
    };
    receipt.contract_id.as_uuid()
}

fn all_orders() -> OrderQuery {
    OrderQuery {
        window: ListingPageWindow::new(None, None).expect("默认窗口"),
        role: None,
        world: None,
        state: None,
    }
}

/// 买家与卖家各看到自己角色的订单：买家没有手续费与到手金额，卖家的 P = F + S；
/// 非本人（第三个本地账号）与不存在的合同同样为空，列表总数为 0。
#[tokio::test]
async fn postgres_consignment_orders_owned_reads_are_role_scoped_and_hide_other_parties() {
    let fixture = Fixture::available().await;
    let contract = purchased(&fixture).await;
    let store = &fixture.runtime;

    let buyer =
        store.owned_order(user(BUYER), contract).await.expect("买家读取").expect("买家可见");
    assert_eq!(buyer.role(), OrderRole::Buyer);
    assert_eq!(buyer.core().stages.order_state, OrderState::AwaitingPayment);
    assert_eq!(buyer.core().stages.payment, OrderPaymentStage::Unpaid);
    assert_eq!(buyer.core().stages.delivery, OrderDeliveryStage::Prepared);
    assert_eq!(buyer.core().price.map(ListingPrice::value), Some(PRICE));
    assert_eq!((buyer.core().fee, buyer.core().seller_amount), (None, None), "买家不见私有金额");

    let seller =
        store.owned_order(user(USER_1), contract).await.expect("卖家读取").expect("卖家可见");
    assert_eq!(seller.role(), OrderRole::Seller);
    let (fee, seller_amount) = (
        seller.core().fee.expect("卖家有手续费"),
        seller.core().seller_amount.expect("卖家有到手金额"),
    );
    assert_eq!(fee + seller_amount, PRICE, "卖家视图金额守恒");

    assert!(store.owned_order(user(USER_3), contract).await.expect("旁人读取").is_none());
    assert!(store.owned_order(user(BUYER), Uuid::new_v4()).await.expect("不存在").is_none());
    let outsider = store.list_owned_orders(user(USER_3), all_orders()).await.expect("旁人列表");
    assert_eq!((outsider.total, outsider.items.len()), (0, 0));
    let own = store.list_owned_orders(user(BUYER), all_orders()).await.expect("买家列表");
    assert_eq!(own.total, 1);
    assert_eq!(own.items.first().map(|order| order.core().contract_id), Some(contract));
}

async fn order_events(fixture: &Fixture, contract: Option<Uuid>) -> Vec<serde_json::Value> {
    let pattern = contract
        .map_or_else(|| "consignment_order:%".to_owned(), |id| format!("consignment_order:{id}"));
    sqlx::query_scalar(
        "SELECT public_payload FROM public.realtime_events
         WHERE stream_key LIKE $1 ORDER BY stream_key, stream_sequence",
    )
    .bind(pattern)
    .fetch_all(fixture.pool())
    .await
    .expect("读取订单阶段事件")
}

/// 成交提交的同时恰有一条订单阶段事件，负载只有合同与阶段码，不含金额与任何对方信息；
/// 注入事件写入失败：整笔购买回滚——零合同、零事件、挂单仍 available、命令 ID 未占用；
/// 撤掉注入后同命令 ID 原样重试成交，事件仍恰一条。
#[tokio::test]
async fn postgres_consignment_orders_purchase_event_commits_and_rolls_back_with_the_sale() {
    let fixture = Fixture::available().await;
    sqlx::raw_sql(sqlx::AssertSqlSafe(
        "CREATE FUNCTION public.fake_fail_order_event() RETURNS trigger
         LANGUAGE plpgsql AS $$
         BEGIN RAISE EXCEPTION 'fake injected failure'; END $$;
         CREATE TRIGGER fake_fail_order_event
         BEFORE INSERT ON public.realtime_events
         FOR EACH ROW WHEN (NEW.kind = 'consignment.order_stage_changed')
         EXECUTE FUNCTION public.fake_fail_order_event();"
            .to_owned(),
    ))
    .execute(fixture.pool())
    .await
    .expect("注入事件写入失败");

    let failed = fixture.repository.purchase(purchase(BUYER, 1, 0x11)).await;
    assert!(failed.is_err(), "事件写入失败时购买必须失败：{failed:?}");
    assert_eq!(fixture.count("consignment_contracts").await, 0);
    assert_eq!(fixture.listing().await, ("available".to_owned(), 5, PRICE));
    assert!(order_events(&fixture, None).await.is_empty(), "回滚后不留事件");

    sqlx::raw_sql(sqlx::AssertSqlSafe(
        "DROP TRIGGER fake_fail_order_event ON public.realtime_events;".to_owned(),
    ))
    .execute(fixture.pool())
    .await
    .expect("撤掉注入");
    let contract = purchased(&fixture).await;
    let events = order_events(&fixture, Some(contract)).await;
    assert_eq!(events.len(), 1, "成交恰有一条订单阶段事件");
    let payload = events.first().and_then(|payload| payload.as_object()).expect("负载是对象");
    let mut keys: Vec<&str> = payload.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "contract_id",
            "delivery_stage",
            "order_state",
            "payment_stage",
            "payout_stage",
            "refund_stage"
        ],
        "负载恰为合同 ID 与阶段码"
    );
    assert_eq!(payload["order_state"], "awaiting_payment");
}

/// 代管上架的 available 件出现在公开 v2 目录与详情，带处分代；验证器未签收的本期 `purchasable` 为假。
/// 成交后挂单转 sale_pending，目录不再列出它，详情同形为空。
#[tokio::test]
async fn postgres_consignment_orders_catalog_v2_lists_available_but_is_not_purchasable() {
    let fixture = Fixture::available().await;
    let query = PublicListingQuery {
        window: ListingPageWindow::new(None, None).expect("默认窗口"),
        world: None,
        mode: None,
        category: None,
        name: None,
    };
    let page = fixture.runtime.list_public_v2(query.clone()).await.expect("目录");
    assert_eq!(page.total, 1);
    let entry = page.items.first().expect("available 件在目录");
    assert_eq!((entry.listing_id, entry.state), (fixture.listing, ListingState::Available));
    assert!(entry.generation.is_some(), "available 必带处分代");
    assert!(!entry.purchasable, "验证器未签收时恒不可购买");
    let detail = fixture
        .runtime
        .public_detail_v2(fixture.listing)
        .await
        .expect("详情")
        .expect("available 件有详情");
    assert!(!detail.entry.purchasable);

    purchased(&fixture).await;
    assert_eq!(fixture.runtime.list_public_v2(query).await.expect("成交后目录").total, 0);
    assert!(fixture.runtime.public_detail_v2(fixture.listing).await.expect("成交后详情").is_none());
}

/// 寄售关闭后只读重放：同命令 ID 同请求回原成交回执（`replayed`），同 ID 异摘要为命令冲突，
/// 账本里没有的新命令为空；重放不写任何行（命令数、合同数不变）。
#[tokio::test]
async fn postgres_consignment_purchase_replay_after_close_returns_the_original_receipt() {
    let fixture = Fixture::available().await;
    let first = fixture.repository.purchase(purchase(BUYER, 1, 0x11)).await.expect("购买");
    let PurchaseOutcome::Purchased(first) = first else { panic!("夹具购买必须成交") };
    sqlx::query("UPDATE public.consignment_lookup_control SET admitting = false WHERE id = 1")
        .execute(fixture.pool())
        .await
        .expect("关闭寄售");
    let commands = fixture.count("consignment_commands").await;

    let replayed = fixture
        .repository
        .replay_purchase(purchase(BUYER, 1, 0x11))
        .await
        .expect("重放")
        .expect("账本命中");
    let PurchaseOutcome::Purchased(replayed) = replayed else { panic!("应回成交回执") };
    assert!(replayed.replayed);
    assert_eq!(replayed.contract_id, first.contract_id);

    let conflict = fixture.repository.replay_purchase(purchase(BUYER, 1, 0x22)).await;
    assert_eq!(conflict, Err(ConsignmentError::CommandIdConflict));
    let fresh = fixture.repository.replay_purchase(purchase(BUYER, 2, 0x33)).await.expect("新命令");
    assert!(fresh.is_none(), "账本没有的命令不得凭空成交");
    assert_eq!(fixture.count("consignment_commands").await, commands, "重放不写账本");
    assert_eq!(fixture.count("consignment_contracts").await, 1);
}
