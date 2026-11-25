//! 两世界端到端验收（PIAC E19、D9 §11.1 第 7 期“全期及发布”行、PIWP、PIWQ）：main 与 isekai 各两张订单，从代管
//! 上架起经真实写入口走到受益人确领——卖款链：购买 → 停用（软关闭）后续办 → 派发 executor 首派（假传输，恰一次
//! POST）→ 收款与确领推进循环（回环观察替身、测试验证器）记付款证据并认证实收（main 同页确领原件、放行卖款）→
//! （isekai：下游首轮生产者建原件交割轮并派发，推进循环确领原件、放行卖款）→ 生产者建卖款首轮 → executor 派发
//! 卖款 → 推进循环记卖家现金确领；退款链：实收认证后退款转换 → 生产者建退款首轮 → executor 派发退款 → 推进循环
//! 记买家现金确领。每步断言状态、资金分录与订单阶段事件，关键写入另验重放（推进循环重跑）无新增。全程不连接
//! 任何外部账号，推进循环零 POST、零 Take。
//!
//! 保护不变量 1（重放返回原结果）、2（同事务锁定与判定）、3（事实与事件同事务）、4（每轮恰一次外写）、
//! 6（冻结事实逐项复验、失败关闭）。

use auction_application::settlement::CashLeg;
use auction_domain::consignment::dispatch::AttemptKey;
use auction_jobs::consignment::DispatchRound;

use super::super::dispatch::worker_chain::delivery_facts;
use super::chain::{OWN, Order, World};

/// 购买、软关闭与首派：两条链共用的前半段，返回付款腿首轮的尝试。
async fn purchased_and_dispatched(order: &Order) -> AttemptKey {
    let world = order.world;
    let pool = order.pool();

    // 购买：挂单 sale_pending、position committed、合同与义务图、首轮同事务落库，追加订单阶段事件。
    let events = order.order_events().await;
    order.purchase().await.expect("购买");
    let listing: (String, i64) =
        sqlx::query_as("SELECT state, state_version FROM public.consignment_listings")
            .fetch_one(pool)
            .await
            .expect("挂单");
    assert_eq!(listing, ("sale_pending".to_owned(), 6));
    assert_eq!(order.position_state().await, "committed");
    let obligations = if world == World::Main { 3 } else { 4 };
    assert_eq!(order.count("settlement_obligations").await, obligations);
    assert_eq!(order.count("consignment_dispatch_rounds").await, 1, "首轮随购买同事务建立");
    assert!(order.order_events().await > events, "购买追加订单阶段事件");

    // 停用寄售（软关闭）：只截止新业务，已成交订单继续履约。
    order.soft_close().await;

    // 首派：生产 executor 恰一次 POST，Sent 证据落库，投递 succeeded；再跑一轮无第二次 POST。
    assert_eq!(order.run_executor().await, DispatchRound::Sent);
    assert_eq!(order.posts(), 1);
    let (payment_kind, _) = world.payment();
    let attempt = order.first_round(order.obligation(payment_kind).await).await;
    assert_eq!(delivery_facts(pool, attempt.delivery_id).await, ("succeeded".to_owned(), 1, 1));
    if world == World::Isekai {
        // 载体本金：严格现金由原 UID 自有借出 1，Sent 证明后 spent。
        assert_eq!(order.allocation("carrier").await, Some(("spent".to_owned(), 1)));
        assert_eq!(order.allocation("proprietary").await, Some(("available".to_owned(), OWN - 1)));
    }
    assert_eq!(order.run_executor().await, DispatchRound::Idle);
    assert_eq!(order.produce_rounds().await, 0, "实收认证前下游义务都未放行");
    assert_eq!(order.posts(), 1, "下游义务未就绪，零第二次 POST");

    attempt
}

/// 付款认证之后的共同断言：付款义务满足、Sale 资金化、S 进卖款用途、费用恰一条；返回成交冻结的卖款 S。
async fn assert_certified(order: &Order) -> i64 {
    let seller_amount: i64 =
        order.scalar("SELECT seller_amount FROM public.consignment_sales".to_owned()).await;
    assert_eq!(order.count("consignment_funded_sales").await, 1);
    assert_eq!(order.count("consignment_fee_facts").await, 1);
    assert_eq!(order.count("consignment_cod_receipts").await, 1);
    assert_eq!(order.allocation("seller").await, Some(("available".to_owned(), seller_amount)));
    seller_amount
}

/// 推进循环认证实收（main 同页确领原件）；重跑零新增。返回成交冻结的卖款 S。
async fn certified_by_loop(order: &Order) -> i64 {
    let before = order.order_events().await;
    let round = order.run_collections().await;
    // 付款证据 + 认证；main 的同一次领取另确领原件。
    let expected = if order.world == World::Main { 3 } else { 2 };
    assert_eq!((round.advanced, round.failed), (expected, false));
    let seller_amount = assert_certified(order).await;
    assert!(order.order_events().await > before, "认证追加订单阶段事件");
    let entries = order.count("settlement_funding_entries").await;
    assert_eq!(order.run_collections().await.advanced, 0, "重跑零新增");
    assert_eq!(order.count("settlement_funding_entries").await, entries);
    assert_eq!(order.count("consignment_cod_receipts").await, 1);
    seller_amount
}

/// 直接经写口认证（不确领）：main 的退款链需要“已认证、尚未确领”的订单，推进循环在同一页里对 main 原件 COD 先
/// 认证后确领，构造不出这一状态（确领临时缺证时它在生产上仍可达），故这一条链保留直接调用。
async fn certified_directly(order: &Order, attempt: AttemptKey) {
    order.record_payment(attempt).await;
    let receipt = order.certify(attempt).await.expect("COD 实收认证");
    assert!(!receipt.replayed);
    assert_certified(order).await;
    assert!(order.certify(attempt).await.expect("认证重放").replayed);
    // 卖款仍等原件确领：认证不放行卖款。
    assert_eq!(order.business_state("seller_payout").await, "blocked");
}

/// 卖家或买家现金确领经推进循环：恰一条 claimed、追加订单阶段事件；重跑零新增。
async fn cash_claimed_by_loop(order: &Order, amount: i64) {
    let before = order.order_events().await;
    let round = order.run_collections().await;
    assert_eq!((round.advanced, round.failed), (1, false), "现金确领证据恰记一次");
    assert_eq!(order.entries("claimed").await, (1, amount));
    assert!(order.order_events().await > before);
    let entries = order.count("settlement_funding_entries").await;
    assert_eq!(order.run_collections().await.advanced, 0, "重跑零新增");
    assert_eq!(order.count("settlement_funding_entries").await, entries);
}

/// 一张订单从上架走到卖家确领。
async fn order_reaches_seller_claim(world: World) {
    let order = Order::available(world).await;
    let pool = order.pool();
    purchased_and_dispatched(&order).await;
    let takes = order.take_write_starts().await;
    let before_handoff = order.order_events().await;
    let seller_amount = certified_by_loop(&order).await;

    // 买家确领原件：main 是付款腿同一封 COD 的领取，推进循环已在认证的同一页确领；isekai 先由生产者建原件交割
    // 首轮并在 isekai 派发，交割轮被领取后推进循环确领。
    if world == World::Isekai {
        assert_eq!(order.business_state("seller_payout").await, "blocked");
        assert_eq!(order.produce_rounds().await, 1, "认证放行原件交割，生产者恰建一轮");
        order.send_from("isekai").await;
        assert_eq!(order.run_executor().await, DispatchRound::Sent);
        order.send_from("main").await;
        assert_eq!(order.posts(), 2);
        let handed = order.run_collections().await;
        assert_eq!((handed.advanced, handed.failed), (2, false), "交割证据与确领各一次");
    }

    // position 已交付、挂单成交关闭、funded 前进，同事务放行卖款；重跑零新增。
    let listing: (String, Option<String>) =
        sqlx::query_as("SELECT state, reason_code FROM public.consignment_listings")
            .fetch_one(pool)
            .await
            .expect("挂单");
    assert_eq!(listing, ("closed".to_owned(), Some("sold".to_owned())));
    assert_eq!(order.position_state().await, "handed_over");
    assert_eq!(order.business_state("seller_payout").await, "ready");
    assert!(order.order_events().await > before_handoff, "确领追加订单阶段事件");
    assert_eq!(order.run_collections().await.advanced, 0, "确领重跑零新增");
    assert_eq!(order.count("consignment_handoff_proofs").await, 1);

    // 卖款首轮：生产者恰建一轮（再跑零新增），executor 起写同事务占用 S、恰一次 POST、Sent 后 spent。
    assert_eq!(order.produce_rounds().await, 1);
    assert_eq!(order.produce_rounds().await, 0);
    let posts = order.posts();
    assert_eq!(order.run_executor().await, DispatchRound::Sent);
    assert_eq!(order.posts(), posts + 1);
    let payout = order.first_round(order.obligation("seller_payout").await).await;
    assert_eq!(order.allocation("seller").await, Some(("spent".to_owned(), seller_amount)));
    assert_eq!(delivery_facts(pool, payout.delivery_id).await, ("succeeded".to_owned(), 1, 1));
    assert_eq!(order.run_executor().await, DispatchRound::Idle);

    // 卖家确领：推进循环记现金确领证据，同事务把已支出的 S 记一条 claimed；之后直接推进只登记命令。
    cash_claimed_by_loop(&order, seller_amount).await;
    let settled = order.count("settlement_funding_entries").await;
    let direct = order.settle_claimed("seller_payout", CashLeg::Seller, payout, seller_amount);
    assert!(direct.await.expect("直接推进"), "已确领只登记命令");
    assert_eq!(order.count("settlement_funding_entries").await, settled);
    assert_eq!(order.entries("claimed").await, (1, seller_amount));
    assert_eq!(order.take_write_starts().await, takes, "推进循环零新增 Take");
}

/// 一张订单实收认证后转退款，走到买家确领退款（main 见函数内说明）。
async fn order_refund_reaches_buyer_claim(world: World) {
    let order = Order::available(world).await;
    let pool = order.pool();
    let attempt = purchased_and_dispatched(&order).await;
    let takes = order.take_write_starts().await;
    if world == World::Main {
        certified_directly(&order, attempt).await;
    } else {
        certified_by_loop(&order).await;
    }

    // 退款转换：S + F 转为 P，同事务激活退款义务；funded 前进为 refund_converted。
    assert!(!order.convert_refund().await.expect("退款转换").replayed);
    assert_eq!(order.business_state("buyer_refund").await, "ready");
    assert_eq!(order.allocation("refund").await, Some(("available".to_owned(), 101)));
    assert_eq!(order.allocation("seller").await, Some(("available".to_owned(), 0)));

    // 生产者只建退款首轮：isekai 的原件交割已随退款不再相符，卖款仍 blocked。
    assert_eq!(order.produce_rounds().await, 1);
    assert_eq!(order.produce_rounds().await, 0);
    assert_eq!(order.count("consignment_dispatch_rounds").await, 2, "付款首轮 + 退款首轮");
    let posts = order.posts();
    if world == World::Main {
        // main 的 COD 实收即买家已领原件：实收认证时买方续办权利已按订单事实转为 fulfilled（终态），之后转出的
        // 退款在起写前的收件人授权处被保留——零 POST，P 原样留在退款用途待人工处置（失败关闭；见交付报告）。
        assert_eq!(order.run_executor().await, DispatchRound::Closed);
        assert_eq!(order.posts(), posts);
        assert_eq!(order.allocation("refund").await, Some(("available".to_owned(), 101)));
        assert_eq!(order.entries("claimed").await, (0, 0));
        return;
    }
    assert_eq!(order.run_executor().await, DispatchRound::Sent);
    assert_eq!(order.posts(), posts + 1);
    let refund = order.first_round(order.obligation("buyer_refund").await).await;
    assert_eq!(order.allocation("refund").await, Some(("spent".to_owned(), 101)));
    assert_eq!(delivery_facts(pool, refund.delivery_id).await, ("succeeded".to_owned(), 1, 1));
    assert_eq!(order.run_executor().await, DispatchRound::Idle);

    // 买家确领退款：推进循环恰记一条 claimed；重跑无新增。
    cash_claimed_by_loop(&order, 101).await;
    assert_eq!(order.take_write_starts().await, takes, "推进循环零新增 Take");
}

#[tokio::test]
async fn postgres_consignment_acceptance_main_order_reaches_seller_claim() {
    order_reaches_seller_claim(World::Main).await;
}

#[tokio::test]
async fn postgres_consignment_acceptance_isekai_order_reaches_seller_claim() {
    order_reaches_seller_claim(World::Isekai).await;
}

#[tokio::test]
async fn postgres_consignment_acceptance_main_refund_is_retained_after_cod_fulfilled() {
    order_refund_reaches_buyer_claim(World::Main).await;
}

#[tokio::test]
async fn postgres_consignment_acceptance_isekai_refund_reaches_buyer_claim() {
    order_refund_reaches_buyer_claim(World::Isekai).await;
}
