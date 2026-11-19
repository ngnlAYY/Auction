//! 结算退件案件（迁移 000114–000116）的 PostgreSQL 18 用例，全部以运行账号执行：
//! - 每条退件证据一个案件（含 untrusted/unbound/fulfillment_terminal），列表、详情与待办计数一致；
//! - Admin 命令按 command_id 幂等、按案件版本乐观并发，并发只成功一项；
//! - 来源、绑定与资产不成立的退件不能领取或补寄，零动作、零资金变化；
//! - 持久 Take 与入站/恢复的同邮件处置互斥；Take 结果未知不恢复资金、不新建 Send，投递台不能
//!   人工重试或声明结果；
//! - 精确领回证明只消费一次：现金原用途由 spent 恢复为 available（金额 M，不是 2M），返还事实
//!   唯一绑定入站与 Take；补寄沿用原义务、换新派发，旧派发仍受资金保护；
//! - 代管与留箱结案保留原权利人责任，偿付只计一次代管负债，余额早于领回证明时结论未知；
//! - 事实表只追加，运行账号只能 INSERT。
//!
//! Take 的外部执行（投递 Worker）以直接 SQL 模拟终态；其余推进都经存储端口。不发外部请求。

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::assert_matches;

use auction_application::core::ApplicationError;
use auction_application::recovery::RecoveryWorkStore;
use auction_application::settlement::{
    SettlementReturnAdminPort, SettlementReturnAssetLocation, SettlementReturnCommand,
    SettlementReturnCommandResult, SettlementReturnError, SettlementReturnListRequest,
    SettlementReturnMatch, SettlementReturnWorkStore,
};
use auction_domain::{CommandId, SettlementReturnAction, SettlementReturnCaseStatus};
use auction_time::{Duration, Timestamp};
use sqlx::PgPool;
use uuid::Uuid;

use super::{evidence, insert_inbound, insert_sent_fulfillment};
use crate::recovery_returns::{CodSale, insert_sale, settlement_minute};
use crate::support;

const PAYOUT: i64 = 90;
const FUNDING_UID: &str = "7301";

/// 已寄出 90 Credits 货款并被系统退回的标准案件。
struct ReturnedPayout {
    sale: CodSale,
    delivery: Uuid,
    case: Uuid,
}

/// 写入一笔已寄出的货款与它的系统退件，并经消费方建立案件；`credits` 是退件附带的金额。
async fn returned_payout(
    store: &auction_storage::DatabaseStore,
    sequence: i64,
    external_mail_id: &str,
    credits: i64,
) -> ReturnedPayout {
    let pool = store.postgres_pool();
    let (minute, settled_at) = settlement_minute();
    let sale = insert_sale(pool, sequence, settled_at, 1).await;
    let delivery = insert_sent_fulfillment(
        pool,
        sale.seller_obligation,
        "settlement_seller_payout",
        PAYOUT,
        minute + Duration::minutes(10),
        "enqueued",
    )
    .await;
    let case = insert_inbound(
        pool,
        external_mail_id,
        minute + Duration::minutes(40),
        Some(("MoogleMail", format!("[RTS] Auction seller payout [{}]", sale.seller_obligation))),
        evidence(sale.seller_obligation, credits, &[]),
    )
    .await;
    let consumed = store
        .consume_due_settlement_return(Timestamp::now())
        .await
        .expect("消费退件")
        .expect("必须领取到退件");
    assert_eq!(consumed.return_mail_id, case);
    ReturnedPayout { sale, delivery, case }
}

/// 为原派发写入 record_only 资金账户与已支出的货款分配，并登记历史绑定。
async fn seed_spent_payout(pool: &PgPool, returned: &ReturnedPayout) -> Uuid {
    sqlx::query(
        "INSERT INTO public.settlement_funding_accounts (uid, status, accounting_mode)
         VALUES ($1, 'ready', 'record_only') ON CONFLICT (uid) DO NOTHING",
    )
    .bind(FUNDING_UID)
    .execute(pool)
    .await
    .expect("写入资金账户");
    let allocation = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.settlement_funding_allocations
            (id, uid, purpose, lot_id, obligation_id, amount, state, delivery_id)
         VALUES ($1, $2, 'seller', $3, $4, $5, 'spent', $6)",
    )
    .bind(allocation)
    .bind(FUNDING_UID)
    .bind(returned.sale.lot_id)
    .bind(returned.sale.seller_obligation)
    .bind(PAYOUT)
    .bind(returned.delivery)
    .execute(pool)
    .await
    .expect("写入已支出的货款分配");
    sqlx::query(
        "INSERT INTO public.settlement_funding_delivery_bindings (delivery_id, allocation_id, uid)
         VALUES ($1, $2, $3)",
    )
    .bind(returned.delivery)
    .bind(allocation)
    .bind(FUNDING_UID)
    .execute(pool)
    .await
    .expect("登记历史资金绑定");
    allocation
}

async fn handle(
    store: &auction_storage::DatabaseStore,
    command_id: Uuid,
    case: Uuid,
    version: i64,
    action: SettlementReturnAction,
) -> Result<SettlementReturnCommandResult, ApplicationError> {
    let command = SettlementReturnCommand::new(
        CommandId::from_uuid(command_id),
        case,
        version,
        action,
        "admin-returns",
        Timestamp::now(),
    )
    .expect("构造退件命令");
    store.handle_settlement_return(command).await
}

fn business(error: ApplicationError) -> SettlementReturnError {
    match error {
        ApplicationError::SettlementReturn(error) => error,
        other => panic!("期望结算退件业务错误：{other:?}"),
    }
}

async fn case_state(pool: &PgPool, case: Uuid) -> (String, i64, Option<String>, Option<i32>) {
    sqlx::query_as(
        "SELECT status, case_version, plan, resend_dispatch_no
         FROM public.settlement_return_cases WHERE return_mail_id = $1",
    )
    .bind(case)
    .fetch_one(pool)
    .await
    .expect("读取案件")
}

async fn scalar(pool: &PgPool, sql: &str, id: Uuid) -> i64 {
    sqlx::query_scalar(sqlx::AssertSqlSafe(sql.to_owned()))
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("读取计数")
}

async fn take_delivery(pool: &PgPool, case: Uuid) -> Uuid {
    sqlx::query_scalar(
        "SELECT delivery_id FROM public.settlement_return_actions WHERE return_mail_id = $1",
    )
    .bind(case)
    .fetch_one(pool)
    .await
    .expect("读取 Take 投递")
}

/// 以直接 SQL 模拟投递 Worker 执行 Take 后的终态：成功时写入与传输层一致的动作证明，否则写结果
/// 未知。
async fn finish_take(pool: &PgPool, delivery: Uuid, succeeded: bool) {
    sqlx::query(
        "UPDATE public.deliveries
         SET status = CASE WHEN $2 THEN 'succeeded' ELSE 'manual_reconcile' END,
             fencing_version = 1, attempt_count = 1, claimed_at = now(), sending_at = now(),
             completed_at = now(), updated_at = now(),
             external_reference = CASE WHEN $2 THEN 'mooglemail:' || (payload ->> 'world')
                 || ':take:' || (payload ->> 'external_mail_id') END,
             failure_code = CASE WHEN $2 THEN NULL ELSE 'remote_unknown' END,
             result_metadata = CASE WHEN $2 THEN jsonb_build_object(
                 'operation', 'take',
                 'external_mail_id', payload ->> 'external_mail_id',
                 'proof', 'action_removed_or_success_page',
                 'expected_credits', payload -> 'credits',
                 'expected_cod', payload -> 'cod',
                 'expected_attachment_ids', payload -> 'attachment_ids')
                 ELSE jsonb_build_object('outcome_unknown', true) END
         WHERE id = $1 AND status = 'pending'",
    )
    .bind(delivery)
    .bind(succeeded)
    .execute(pool)
    .await
    .expect("模拟 Take 终态");
}

/// 四种处置的证据都建案；未知义务、空投递不会被联表丢失；列表总数、筛选与待办计数一致，详情的
/// 允许动作随绑定强度变化。
#[tokio::test]
async fn postgres_return_cases_include_quarantined_and_terminal_evidence() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let bound = returned_payout(&store, 7_500_001, "992001", PAYOUT).await;
    let (minute, settled_at) = settlement_minute();
    let terminal = insert_sale(pool, 7_500_002, settled_at, 1).await;
    insert_sent_fulfillment(
        pool,
        terminal.seller_obligation,
        "settlement_seller_payout",
        PAYOUT,
        minute + Duration::minutes(5),
        "satisfied",
    )
    .await;
    let terminal_mail = insert_inbound(
        pool,
        "992002",
        minute + Duration::minutes(41),
        Some((
            "MoogleMail",
            format!("[RTS] Auction seller payout [{}]", terminal.seller_obligation),
        )),
        evidence(terminal.seller_obligation, PAYOUT, &[]),
    )
    .await;
    let unknown = Uuid::new_v4();
    let unbound_mail = insert_inbound(
        pool,
        "992003",
        minute + Duration::minutes(42),
        Some(("MoogleMail", format!("[RTS] Auction buyer refund [{unknown}]"))),
        evidence(unknown, 5, &[]),
    )
    .await;
    let untrusted_mail = insert_inbound(
        pool,
        "992004",
        minute + Duration::minutes(43),
        Some(("Player", format!("[RTS] Auction buyer refund [{unknown}]"))),
        evidence(unknown, 5, &[]),
    )
    .await;
    while store.consume_due_settlement_return(Timestamp::now()).await.expect("消费退件").is_some()
    {
    }

    let page = store
        .list_settlement_returns(SettlementReturnListRequest { limit: 50, ..Default::default() })
        .await
        .expect("列出退件案件");
    assert_eq!(page.total, 4);
    assert_eq!(page.items.len(), 4);
    let by_id = |id: Uuid| page.items.iter().find(|item| item.return_mail_id == id).expect("案件");
    assert_eq!(by_id(bound.case).match_state, SettlementReturnMatch::ManualReconcile);
    assert_eq!(by_id(bound.case).status, SettlementReturnCaseStatus::AwaitingDecision);
    assert_eq!(by_id(bound.case).delivery_id, Some(bound.delivery));
    assert_eq!(by_id(bound.case).dispatch_no, Some(1));
    assert_eq!(by_id(bound.case).resend_limit, 3);
    assert_eq!(by_id(bound.case).recipient.as_deref(), Some("Recovery seller"));
    assert_eq!(by_id(bound.case).asset_location, SettlementReturnAssetLocation::ReturnedInInbox);
    assert_eq!(by_id(terminal_mail).match_state, SettlementReturnMatch::FulfillmentTerminal);
    assert_eq!(by_id(unbound_mail).match_state, SettlementReturnMatch::Unbound);
    assert_eq!(by_id(unbound_mail).obligation_id, None, "未知义务不被联表丢弃");
    assert_eq!(by_id(unbound_mail).status, SettlementReturnCaseStatus::Quarantined);
    assert_eq!(by_id(untrusted_mail).match_state, SettlementReturnMatch::Untrusted);
    assert_eq!(by_id(untrusted_mail).asset_location, SettlementReturnAssetLocation::Unverified);

    let quarantined = store
        .list_settlement_returns(SettlementReturnListRequest {
            status: Some(SettlementReturnCaseStatus::Quarantined),
            limit: 1,
            ..Default::default()
        })
        .await
        .expect("按状态筛选");
    assert_eq!((quarantined.total, quarantined.items.len()), (2, 1), "总数不受分页影响");
    let referenced = store
        .list_settlement_returns(SettlementReturnListRequest {
            reference: Some(bound.sale.seller_obligation),
            limit: 10,
            ..Default::default()
        })
        .await
        .expect("按义务参考筛选");
    assert_eq!(
        referenced.items.iter().map(|item| item.return_mail_id).collect::<Vec<_>>(),
        [bound.case]
    );
    assert_eq!(store.open_settlement_return_count().await.expect("待办计数"), 4);

    let detail = store.settlement_return_detail(bound.case).await.expect("案件详情");
    // 原派发没有资金分配的历史现金退件只能代管或留箱，不能补寄。
    assert_eq!(
        detail.allowed_actions,
        [SettlementReturnAction::ReclaimAndHold, SettlementReturnAction::CloseInbox],
        "{:?}",
        detail.blockers
    );
    assert!(detail.blockers.contains(&(
        SettlementReturnAction::ReclaimAndResend,
        auction_domain::SettlementReturnBlocker::ObligationIneligible
    )));
    assert_eq!(detail.events.len(), 1);
    assert_eq!(detail.events[0].event_kind, "opened");
    assert_eq!(detail.dispatches.len(), 0, "直接写入的历史履约没有派发历史行");
    let untrusted = store.settlement_return_detail(untrusted_mail).await.expect("隔离详情");
    assert_eq!(untrusted.allowed_actions, [SettlementReturnAction::Acknowledge]);
    let unbound = store.settlement_return_detail(unbound_mail).await.expect("未绑定详情");
    assert_eq!(
        unbound.allowed_actions,
        [SettlementReturnAction::Acknowledge, SettlementReturnAction::Recheck]
    );
    assert_matches!(
        store.settlement_return_detail(Uuid::new_v4()).await,
        Err(ApplicationError::SettlementReturn(SettlementReturnError::NotFound))
    );

    pool.close().await;
}

/// 同 ID 同请求重放原结果；同 ID 不同请求冲突；陈旧版本拒绝；两个并发命令只成功一项，且只建立
/// 一项 Take。
#[tokio::test]
async fn postgres_return_handle_is_idempotent_and_versioned() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let returned = returned_payout(&store, 7_500_011, "992011", PAYOUT).await;

    let first = Uuid::new_v4();
    let result = handle(&store, first, returned.case, 1, SettlementReturnAction::ReclaimAndHold)
        .await
        .expect("领回代管");
    assert_eq!((result.case_version, result.status), (2, SettlementReturnCaseStatus::TakePending));
    let replay = handle(&store, first, returned.case, 1, SettlementReturnAction::ReclaimAndHold)
        .await
        .expect("同 ID 同请求重放");
    assert_eq!(replay, result);
    assert_eq!(
        business(
            handle(&store, first, returned.case, 1, SettlementReturnAction::CloseInbox)
                .await
                .expect_err("同 ID 不同请求")
        ),
        SettlementReturnError::CommandConflict
    );
    assert_eq!(
        business(
            handle(&store, Uuid::new_v4(), returned.case, 1, SettlementReturnAction::CloseInbox)
                .await
                .expect_err("陈旧版本")
        ),
        SettlementReturnError::Stale
    );
    assert_eq!(
        business(
            handle(&store, Uuid::new_v4(), returned.case, 2, SettlementReturnAction::CloseInbox)
                .await
                .expect_err("处理中不接受新动作")
        ),
        SettlementReturnError::ActionConflict
    );
    assert_eq!(
        scalar(
            pool,
            "SELECT count(*) FROM public.settlement_return_actions WHERE return_mail_id = $1",
            returned.case
        )
        .await,
        1
    );
    assert_eq!(
        scalar(
            pool,
            "SELECT count(*) FROM public.settlement_return_commands WHERE return_mail_id = $1",
            returned.case
        )
        .await,
        1
    );

    // 两个 Admin 同时以同一版本提交不同命令：只有一项提交，另一项按版本或冲突失败。
    let other = returned_payout(&store, 7_500_012, "992012", PAYOUT).await;
    let (left, right) = tokio::join!(
        handle(&store, Uuid::new_v4(), other.case, 1, SettlementReturnAction::ReclaimAndHold),
        handle(&store, Uuid::new_v4(), other.case, 1, SettlementReturnAction::ReclaimAndResend),
    );
    assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1, "{left:?} {right:?}");
    assert_eq!(
        scalar(
            pool,
            "SELECT count(*) FROM public.settlement_return_actions WHERE return_mail_id = $1",
            other.case
        )
        .await,
        1
    );
    assert_eq!(case_state(pool, other.case).await.1, 2);

    pool.close().await;
}

/// 来源、绑定或资产不成立的退件不能领取：untrusted、unbound、资产不符分别给出类型化错误，
/// 不建立动作、不写资金事实、不新建投递。
#[tokio::test]
async fn postgres_return_action_requires_exact_authenticated_binding() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let mismatch = returned_payout(&store, 7_500_021, "992021", PAYOUT - 1).await;
    let (minute, _) = settlement_minute();
    let unknown = Uuid::new_v4();
    let unbound = insert_inbound(
        pool,
        "992022",
        minute + Duration::minutes(50),
        Some(("MoogleMail", format!("[RTS] Auction buyer refund [{unknown}]"))),
        evidence(unknown, 5, &[]),
    )
    .await;
    let untrusted = insert_inbound(
        pool,
        "992023",
        minute + Duration::minutes(51),
        None,
        evidence(unknown, 5, &[]),
    )
    .await;
    while store.consume_due_settlement_return(Timestamp::now()).await.expect("消费").is_some() {}
    let deliveries_before =
        scalar(pool, "SELECT count(*) FROM public.deliveries WHERE $1 IS NOT NULL", unknown).await;

    for (case, expected) in [
        (mismatch.case, SettlementReturnError::AssetsMismatch),
        (unbound, SettlementReturnError::Unbound),
        (untrusted, SettlementReturnError::Untrusted),
    ] {
        for action in
            [SettlementReturnAction::ReclaimAndHold, SettlementReturnAction::ReclaimAndResend]
        {
            let error =
                handle(&store, Uuid::new_v4(), case, 1, action).await.expect_err("必须拒绝");
            assert_eq!(business(error), expected, "{action:?}");
        }
    }
    assert_eq!(
        business(
            handle(&store, Uuid::new_v4(), untrusted, 1, SettlementReturnAction::Recheck)
                .await
                .expect_err("untrusted 不能核验")
        ),
        SettlementReturnError::Untrusted
    );
    assert_eq!(
        scalar(
            pool,
            "SELECT count(*) FROM public.settlement_return_actions WHERE $1 IS NOT NULL",
            unknown
        )
        .await,
        0
    );
    assert_eq!(
        scalar(
            pool,
            "SELECT count(*) FROM public.settlement_funding_returns WHERE $1 IS NOT NULL",
            unknown
        )
        .await,
        0
    );
    assert_eq!(
        scalar(pool, "SELECT count(*) FROM public.deliveries WHERE $1 IS NOT NULL", unknown).await,
        deliveries_before
    );
    // 隔离记录只能核对结案。
    let acknowledged =
        handle(&store, Uuid::new_v4(), untrusted, 1, SettlementReturnAction::Acknowledge)
            .await
            .expect("核对隔离记录");
    assert_eq!(acknowledged.status, SettlementReturnCaseStatus::Reviewed);

    pool.close().await;
}

/// 同一邮件已有其他来源的领取或退回意图时，领回命令按动作冲突拒绝；Take 已提交后入站流程也不能
/// 再领取同一邮件（同 world 行锁下互斥，失败与未知不解除）。
#[tokio::test]
async fn postgres_return_take_conflicts_with_intake_and_recovery() {
    use auction_application::mail::{
        MailDeliveryCommand, MailDeliveryEnqueuer, MailDeliveryOperation, MailDeliveryOrigin,
    };

    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let returned = returned_payout(&store, 7_500_031, "992031", PAYOUT).await;
    // 恢复流程对同一 MID 的旧 Take（结果未知也不能解除互斥）。
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload, status,
             created_at, updated_at, completed_at, failure_code)
         VALUES ($1, $2, decode(repeat('a7', 32), 'hex'), 'settlement_recovery_action',
                 'recovery:' || $3::text, 'MoogleMail',
                 jsonb_build_object('schema_version', 1, 'operation', 'take', 'world', 'main',
                     'external_mail_id', '992031', 'template', null, 'result_code', null,
                     'user_key_user_id', null, 'credits', 90, 'cod', 0,
                     'attachment_ids', '[]'::jsonb, 'body', null),
                 'manual_reconcile', now(), now(), now(), 'remote_unknown')",
    )
    .bind(Uuid::new_v4())
    .bind(Uuid::new_v4())
    .bind(Uuid::new_v4())
    .execute(pool)
    .await
    .expect("写入恢复 Take");
    assert_eq!(
        business(
            handle(
                &store,
                Uuid::new_v4(),
                returned.case,
                1,
                SettlementReturnAction::ReclaimAndHold
            )
            .await
            .expect_err("同邮件已有处置意图")
        ),
        SettlementReturnError::ActionConflict
    );
    assert_eq!(case_state(pool, returned.case).await.0, "awaiting_decision");

    // 另一封退件：Take 提交并激活后，入站流程的同 MID 领取被互斥拒绝。
    let other = returned_payout(&store, 7_500_032, "992032", PAYOUT).await;
    handle(&store, Uuid::new_v4(), other.case, 1, SettlementReturnAction::ReclaimAndHold)
        .await
        .expect("领回代管");
    let progress = store
        .activate_due_return_take(Timestamp::now(), super::take_worlds())
        .await
        .expect("激活 Take")
        .expect("必须激活");
    assert_eq!(progress.return_mail_id, other.case);
    let take = take_delivery(pool, other.case).await;
    let (origin, status, recipient): (String, String, String) = sqlx::query_as(
        "SELECT origin_kind, status, recipient FROM public.deliveries WHERE id = $1",
    )
    .bind(take)
    .fetch_one(pool)
    .await
    .expect("读取 Take 投递");
    assert_eq!(
        (origin.as_str(), status.as_str(), recipient.as_str()),
        ("settlement_return_action", "pending", "MoogleMail"),
        "Take 先 prepared 再以激活身份进入 pending，收件人字段匹配系统发件人"
    );
    let intake = MailDeliveryCommand {
        command_id: CommandId::from_uuid(Uuid::new_v4()),
        request_hash: auction_application::core::RequestHash::new([3; 32]),
        origin: MailDeliveryOrigin::MailIntake,
        origin_id: "main:992032:take".to_owned(),
        operation: MailDeliveryOperation::Take,
        world_id: "main".to_owned(),
        external_mail_id: "992032".to_owned(),
        recipient: "MoogleMail".to_owned(),
        template: None,
        result_code: None,
        user_key_user_id: None,
        credits: 0,
        cod: 0,
        attachment_ids: Vec::new(),
        notification_body: None,
    };
    assert!(store.enqueue(intake).await.is_err(), "入站领取必须与结算退件 Take 互斥");

    pool.close().await;
}

/// Take 结果未知：动作转人工、案件停在 take_unknown；不写领回事实与资金返还，不新建 Send；投递台
/// 不能对该 Take 人工重试或声明结果，Admin 也不能补寄或代管。
#[tokio::test]
async fn postgres_return_take_unknown_never_restores_funds() {
    use auction_application::delivery::{
        DeliveryAdminAction, DeliveryAdminCommand, DeliveryAdminPort,
    };

    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let returned = returned_payout(&store, 7_500_041, "992041", PAYOUT).await;
    let allocation = seed_spent_payout(pool, &returned).await;
    handle(&store, Uuid::new_v4(), returned.case, 1, SettlementReturnAction::ReclaimAndResend)
        .await
        .expect("领回补寄");
    store
        .activate_due_return_take(Timestamp::now(), super::take_worlds())
        .await
        .expect("激活")
        .expect("必须激活");
    // 投递未到终态前不收口。
    assert!(store.settle_due_return_take(Timestamp::now()).await.expect("收口").is_none());
    let take = take_delivery(pool, returned.case).await;
    finish_take(pool, take, false).await;
    let deliveries_before =
        scalar(pool, "SELECT count(*) FROM public.deliveries WHERE $1 IS NOT NULL", take).await;
    let progress =
        store.settle_due_return_take(Timestamp::now()).await.expect("收口").expect("必须收口");
    assert_eq!(progress.status, SettlementReturnCaseStatus::TakeUnknown);
    assert_eq!(case_state(pool, returned.case).await.0, "take_unknown");
    assert_eq!(
        scalar(
            pool,
            "SELECT count(*) FROM public.settlement_return_takes WHERE return_mail_id = $1",
            returned.case
        )
        .await,
        0
    );
    assert_eq!(
        scalar(
            pool,
            "SELECT count(*) FROM public.settlement_funding_returns WHERE delivery_id = $1",
            returned.delivery
        )
        .await,
        0
    );
    let (state, delivery): (String, Option<Uuid>) = sqlx::query_as(
        "SELECT state, delivery_id FROM public.settlement_funding_allocations WHERE id = $1",
    )
    .bind(allocation)
    .fetch_one(pool)
    .await
    .expect("读取分配");
    assert_eq!((state.as_str(), delivery), ("spent", Some(returned.delivery)), "未知不恢复资金");
    let (fulfillment, dispatch): (String, i32) = sqlx::query_as(
        "SELECT execution_state, dispatch_no FROM public.settlement_obligation_fulfillments
         WHERE obligation_id = $1",
    )
    .bind(returned.sale.seller_obligation)
    .fetch_one(pool)
    .await
    .expect("读取履约");
    assert_eq!((fulfillment.as_str(), dispatch), ("manual_reconcile", 1), "不授权补寄");
    assert_eq!(
        scalar(pool, "SELECT count(*) FROM public.deliveries WHERE $1 IS NOT NULL", take).await,
        deliveries_before
    );
    assert!(store.settle_due_return_take(Timestamp::now()).await.expect("再次收口").is_none());
    for action in
        [SettlementReturnAction::ResendFromCustody, SettlementReturnAction::ReclaimAndHold]
    {
        assert_eq!(
            business(
                handle(&store, Uuid::new_v4(), returned.case, 4, action)
                    .await
                    .expect_err("未知不可处理")
            ),
            SettlementReturnError::ActionConflict
        );
    }
    for action in [
        DeliveryAdminAction::ReconcileRetry,
        DeliveryAdminAction::ReconcileSucceeded,
        DeliveryAdminAction::ReconcileFailed,
    ] {
        let external_reference = Some("mooglemail:main:take:992041".to_owned());
        let failure_code = Some("operator_declared".to_owned());
        let command = DeliveryAdminCommand {
            command_id: Uuid::new_v4(),
            delivery_id: take,
            expected_fencing_version: 1,
            action,
            request_hash: action.request_hash(
                take,
                1,
                external_reference.as_deref(),
                failure_code.as_deref(),
                "admin-returns",
            ),
            external_reference,
            failure_code,
            actor_id: "admin-returns".to_owned(),
            request_id: None,
        };
        assert!(DeliveryAdminPort::execute(&store, command).await.is_err(), "{action:?} 必须拒绝");
    }

    pool.close().await;
}
