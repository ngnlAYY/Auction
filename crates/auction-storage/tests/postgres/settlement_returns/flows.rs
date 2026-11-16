//! 结算退件案件的资金、补寄、结案与对账 PostgreSQL 18 用例（迁移 000114–000115），以运行账号执行：
//! - 现金领回证明只消费一次：原分配 spent → available（金额仍为 M），返还事实唯一绑定入站、Take
//!   与 `returned_delivery` 来源；补寄换新派发编号与新身份，派发历史引用授权案件；新派发寄出证明
//!   成立后本案结束；旧派发不能被投递台重新激活；
//! - 补寄派发再被退回时，按正文派发引用与原正文摘要绑定到第 2 次派发，没有引用的同类退件不绑定；
//! - 未绑定退件经只读核验（读到的系统来源、资产与唯一派发）改绑，读不到时保持隔离；
//! - 新事实表只追加，运行账号对它们只有 INSERT。

#![allow(clippy::expect_used, clippy::unwrap_used)]

use auction_application::core::RequestHash;
use auction_application::delivery::{DeliveryAdminAction, DeliveryAdminCommand, DeliveryAdminPort};
use auction_application::recovery::RecoveryWorkStore;
use auction_application::settlement::{
    SettlementDeliveryPreparation, SettlementFulfillmentStore, SettlementReturnAdminPort,
    SettlementReturnCommand, SettlementReturnMailObservation, SettlementReturnMatch,
    SettlementReturnReason, SettlementReturnRecheckOutcome, SettlementReturnWorkStore,
    parse_returned_settlement_body, returned_mail_body_digest,
};
use auction_domain::{
    CommandId, SettlementDispatchReference, SettlementReturnAction, SettlementReturnCaseStatus,
    resend_delivery_id, resend_prepare_command_id,
};
use chrono::{Duration, Utc};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use super::{evidence, insert_inbound, insert_sent_fulfillment};
use crate::recovery_returns::{CodSale, insert_sale, settlement_minute};
use crate::support;

const PAYOUT: i64 = 90;
const FUNDING_UID: &str = "7311";

struct Returned {
    sale: CodSale,
    delivery: Uuid,
    case: Uuid,
}

async fn returned_payout(
    store: &auction_storage::DatabaseStore,
    sequence: i64,
    mid: &str,
) -> Returned {
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
        mid,
        minute + Duration::minutes(40),
        Some(("MoogleMail", format!("[RTS] Auction seller payout [{}]", sale.seller_obligation))),
        evidence(sale.seller_obligation, PAYOUT, &[]),
    )
    .await;
    store.consume_due_settlement_return(Utc::now()).await.expect("消费").expect("必须消费");
    Returned { sale, delivery, case }
}

async fn seed_spent_payout(pool: &PgPool, returned: &Returned) -> Uuid {
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
    .expect("登记历史绑定");
    allocation
}

async fn handle(
    store: &auction_storage::DatabaseStore,
    case: Uuid,
    version: i64,
    action: SettlementReturnAction,
) -> SettlementReturnCaseStatus {
    let command = SettlementReturnCommand::new(
        CommandId::from_uuid(Uuid::new_v4()),
        case,
        version,
        action,
        "admin-returns",
        Utc::now(),
    )
    .expect("构造命令");
    store.handle_settlement_return(command).await.expect("执行退件命令").status
}

async fn take_through(store: &auction_storage::DatabaseStore, case: Uuid, succeeded: bool) {
    let pool = store.postgres_pool();
    store.activate_due_return_take(Utc::now()).await.expect("激活").expect("必须激活");
    let take: Uuid = sqlx::query_scalar(
        "SELECT delivery_id FROM public.settlement_return_actions WHERE return_mail_id = $1",
    )
    .bind(case)
    .fetch_one(pool)
    .await
    .expect("读取 Take 投递");
    sqlx::query(
        "UPDATE public.deliveries
         SET status = CASE WHEN $2 THEN 'succeeded' ELSE 'manual_reconcile' END,
             fencing_version = 1, attempt_count = 1, claimed_at = now(), sending_at = now(),
             completed_at = now(), updated_at = now(),
             external_reference = CASE WHEN $2 THEN 'mooglemail:' || (payload ->> 'world')
                 || ':take:' || (payload ->> 'external_mail_id') END,
             failure_code = CASE WHEN $2 THEN NULL ELSE 'remote_unknown' END,
             result_metadata = CASE WHEN $2 THEN jsonb_build_object(
                 'operation', 'take', 'external_mail_id', payload ->> 'external_mail_id',
                 'proof', 'action_removed_or_success_page',
                 'expected_credits', payload -> 'credits', 'expected_cod', payload -> 'cod',
                 'expected_attachment_ids', payload -> 'attachment_ids')
                 ELSE '{}'::jsonb END
         WHERE id = $1",
    )
    .bind(take)
    .bind(succeeded)
    .execute(pool)
    .await
    .expect("模拟 Take 终态");
    store.settle_due_return_take(Utc::now()).await.expect("收口 Take").expect("必须收口");
}

async fn count(pool: &PgPool, sql: &str, id: Uuid) -> i64 {
    sqlx::query_scalar(sqlx::AssertSqlSafe(sql.to_owned()))
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("读取计数")
}

/// 现金退件领回补寄：返还只发生一次且金额不翻倍，补寄换新派发、引用授权案件，寄出证明后本案
/// 结束；旧派发不能被投递台重新激活。
#[tokio::test]
async fn postgres_return_cash_take_returns_funding_once_and_resends() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let returned = returned_payout(&store, 7_600_001, "993001").await;
    let allocation = seed_spent_payout(pool, &returned).await;
    // 货款的前置付款已满足：补寄的履约才可被领取。
    sqlx::query(
        "INSERT INTO public.settlement_obligation_fulfillments
            (obligation_id, execution_state, dispatch_world, delivery_operation, completed_at)
         VALUES ($1, 'satisfied', 'main', 'send', now())",
    )
    .bind(returned.sale.payment_obligation)
    .execute(pool)
    .await
    .expect("前置付款已满足");
    let detail = store.settlement_return_detail(returned.case).await.expect("详情");
    assert!(detail.allowed_actions.contains(&SettlementReturnAction::ReclaimAndResend));
    assert_eq!(
        handle(&store, returned.case, 1, SettlementReturnAction::ReclaimAndResend).await,
        SettlementReturnCaseStatus::TakePending
    );
    take_through(&store, returned.case, true).await;
    let (status, plan, dispatch): (String, Option<String>, Option<i32>) = sqlx::query_as(
        "SELECT status, plan, resend_dispatch_no FROM public.settlement_return_cases
         WHERE return_mail_id = $1",
    )
    .bind(returned.case)
    .fetch_one(pool)
    .await
    .expect("读取案件");
    assert_eq!(
        (status.as_str(), plan.as_deref(), dispatch),
        ("resend_pending", Some("resend"), Some(2))
    );

    // 领回事实与返还事实各一条，唯一绑定入站、Take 与来源；分配恢复为 available、金额仍为 90。
    assert_eq!(
        count(
            pool,
            "SELECT count(*) FROM public.settlement_return_takes WHERE return_mail_id = $1",
            returned.case
        )
        .await,
        1
    );
    let (return_case, take_delivery, source): (Option<Uuid>, Option<Uuid>, Option<Uuid>) = sqlx::query_as(
        "SELECT return_mail_id, take_delivery_id, source_id FROM public.settlement_funding_returns
         WHERE delivery_id = $1",
    )
    .bind(returned.delivery)
    .fetch_one(pool)
    .await
    .expect("读取资金返还");
    assert_eq!(return_case, Some(returned.case));
    assert!(take_delivery.is_some() && source.is_some());
    let (kind, key, amount): (String, Uuid, i64) = sqlx::query_as(
        "SELECT kind, source_key, amount FROM public.settlement_funding_sources WHERE id = $1",
    )
    .bind(source)
    .fetch_one(pool)
    .await
    .expect("读取返还来源");
    assert_eq!((kind.as_str(), key, amount), ("returned_delivery", returned.delivery, PAYOUT));
    let (state, allocated, pointer): (String, i64, Option<Uuid>) = sqlx::query_as(
        "SELECT state, amount, delivery_id FROM public.settlement_funding_allocations WHERE id = $1",
    )
    .bind(allocation)
    .fetch_one(pool)
    .await
    .expect("读取分配");
    assert_eq!(
        (state.as_str(), allocated, pointer),
        ("available", PAYOUT, None),
        "金额是 M 不是 2M"
    );
    assert_eq!(
        count(pool, "SELECT count(*) FROM public.settlement_funding_entries WHERE kind = 'return' AND credit_id = $1", allocation).await,
        1
    );
    // 同一证明不再消费：再次收口没有到期项，返还与来源计数不变。
    assert!(store.settle_due_return_take(Utc::now()).await.expect("再次收口").is_none());
    assert_eq!(
        count(
            pool,
            "SELECT count(*) FROM public.settlement_funding_sources WHERE source_key = $1",
            returned.delivery
        )
        .await,
        1
    );

    // 原义务换成第 2 次派发：履约重置为 pending，身份由补寄命名空间派生。
    let claims = SettlementFulfillmentStore::claim_due(
        &store,
        "returns-fulfillment".to_owned(),
        Utc::now(),
        Utc::now() + Duration::minutes(5),
        10,
    )
    .await
    .expect("领取补寄履约");
    let claim = claims
        .into_iter()
        .find(|claim| claim.obligation_id.as_uuid() == returned.sale.seller_obligation)
        .expect("补寄履约可被领取");
    assert_eq!((claim.dispatch_no, claim.delivery_id), (2, None));
    let obligation = returned.sale.seller_obligation;
    let new_delivery = resend_delivery_id(obligation, 2).expect("补寄投递 ID");
    let preparation = |recipient: String| SettlementDeliveryPreparation {
        delivery_id: new_delivery,
        prepare_command_id: CommandId::from_uuid(
            resend_prepare_command_id(obligation, 2).expect("补寄 prepare ID"),
        ),
        request_hash: RequestHash::new([0x77; 32]),
        recipient,
        payload: json!({
            "schema_version": 1, "operation": "send", "world": "main",
            "template": "settlement_seller_payout",
            "variables": {"amount": PAYOUT.to_string(), "item_name": "Recovery equipment",
                          "reference": obligation.to_string(),
                          "dispatch": SettlementDispatchReference::for_resend(obligation, 2)
                              .expect("派发引用").encode()},
            "credits": PAYOUT, "cod": 0, "equipment_secret_refs": []
        }),
    };
    // 补寄只寄原冻结收件人：领取到的收件人与被退回派发不同（例如改名）时失败关闭、不写投递。
    let mut renamed = claim.clone();
    renamed.recipient = "Renamed seller".to_owned();
    assert!(
        SettlementFulfillmentStore::prepare_delivery(
            &store,
            renamed,
            preparation("Renamed seller".to_owned()),
            Utc::now(),
            None,
        )
        .await
        .is_err(),
        "改名后的收件人不能接收补寄"
    );
    assert_eq!(
        count(pool, "SELECT count(*) FROM public.deliveries WHERE id = $1", new_delivery).await,
        0
    );
    SettlementFulfillmentStore::prepare_delivery(
        &store,
        claim.clone(),
        preparation(claim.recipient.clone()),
        Utc::now(),
        None,
    )
    .await
    .expect("准备补寄投递");
    let authorized: (i32, Option<Uuid>) = sqlx::query_as(
        "SELECT dispatch_no, authorized_by FROM public.settlement_fulfillment_dispatches
         WHERE delivery_id = $1",
    )
    .bind(new_delivery)
    .fetch_one(pool)
    .await
    .expect("读取派发历史");
    assert_eq!(authorized, (2, Some(returned.case)), "补寄派发引用授权它的退件案件");

    // 旧派发已不是当前派发，投递台不能重新激活它。
    let command = DeliveryAdminCommand {
        command_id: Uuid::new_v4(),
        delivery_id: returned.delivery,
        expected_fencing_version: 1,
        action: DeliveryAdminAction::ReconcileRetry,
        request_hash: DeliveryAdminAction::ReconcileRetry.request_hash(
            returned.delivery,
            1,
            None,
            None,
            "admin-returns",
        ),
        external_reference: None,
        failure_code: None,
        actor_id: "admin-returns".to_owned(),
        request_id: None,
    };
    assert!(DeliveryAdminPort::execute(&store, command).await.is_err(), "旧派发受保护");

    // 新派发寄出并有 Sent 证明后结束本案；原义务仍等待收件人领取。
    sqlx::query(
        "UPDATE public.deliveries
         SET status = 'succeeded', activation_command_id = $2,
             activation_request_digest = decode(repeat('78', 32), 'hex'), activated_at = now(),
             fencing_version = 1, attempt_count = 1, claimed_at = now(), sending_at = now(),
             completed_at = now(), updated_at = now(),
             external_reference = 'mooglemail:main:send:771001',
             result_metadata = jsonb_build_object('operation', 'send',
                 'external_reference_proven', true, 'sent_verified', true,
                 'proof', 'sent_exact_payload_v1')
         WHERE id = $1",
    )
    .bind(new_delivery)
    .bind(Uuid::new_v4())
    .execute(pool)
    .await
    .expect("模拟补寄寄出");
    let done =
        store.complete_due_return_resend(Utc::now()).await.expect("收口补寄").expect("必须收口");
    assert_eq!(
        (done.return_mail_id, done.status),
        (returned.case, SettlementReturnCaseStatus::Resent)
    );
    assert!(store.complete_due_return_resend(Utc::now()).await.expect("排空").is_none());

    pool.close().await;
}

/// 第 2 次派发被退回时：正文带派发引用且原正文摘要与原收件人都一致才绑定第 2 次派发并建立新
/// 案件；没有派发引用的同义务退件只认首寄，而首寄已被本案绑定，按已退回隔离。
#[tokio::test]
async fn postgres_resend_return_binds_by_dispatch_reference() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let returned = returned_payout(&store, 7_600_011, "993011").await;
    seed_spent_payout(pool, &returned).await;
    sqlx::query(
        "INSERT INTO public.settlement_fulfillment_dispatches
            (obligation_id, dispatch_no, delivery_id, authorized_by, recorded_at)
         VALUES ($1, 1, $2, NULL, now())",
    )
    .bind(returned.sale.seller_obligation)
    .bind(returned.delivery)
    .execute(pool)
    .await
    .expect("登记首寄派发历史");
    handle(&store, returned.case, 1, SettlementReturnAction::ReclaimAndResend).await;
    take_through(&store, returned.case, true).await;
    let obligation = returned.sale.seller_obligation;
    let reference = SettlementDispatchReference::for_resend(obligation, 2).expect("派发引用");
    let original_body = format!(
        "Your auction payout is attached.\n\nReference: {obligation}\n\nDispatch reference: {}",
        reference.encode()
    );
    let body_digest = returned_mail_body_digest(&original_body);
    // 第 2 次派发以补寄身份写入并寄出，发送围栏冻结了原正文摘要。
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload, status,
             fencing_version, attempt_count, claimed_at, sending_at, completed_at,
             external_reference, result_metadata, created_at, updated_at)
         VALUES ($1, $2, decode(repeat('79', 32), 'hex'), 'settlement', $3, 'Recovery seller',
                 jsonb_build_object('schema_version', 1, 'operation', 'send', 'world', 'main',
                     'template', 'settlement_seller_payout',
                     'variables', jsonb_build_object('reference', $3, 'dispatch', $4::text),
                     'credits', 90, 'cod', 0, 'equipment_secret_refs', '[]'::jsonb),
                 'succeeded', 1, 1, now(), now(), now(), 'mooglemail:main:send:772001',
                 '{\"operation\":\"send\"}'::jsonb, now(), now())",
    )
    .bind(reference.delivery_id)
    .bind(resend_prepare_command_id(obligation, 2).expect("prepare ID"))
    .bind(obligation.to_string())
    .bind(reference.encode())
    .execute(pool)
    .await
    .expect("写入第 2 次派发");
    sqlx::query(
        "INSERT INTO public.delivery_send_attempts
            (delivery_id, fencing_version, command_id, request_digest, world_id, sending_at,
             expectation)
         VALUES ($1, 1, $2, decode(repeat('79', 32), 'hex'), 'main', now(),
                 jsonb_build_object('world_id', 'main', 'recipient', 'Recovery seller',
                     'subject', 'Auction seller payout', 'body_digest', $3::jsonb,
                     'credits', 90, 'cod', 0, 'equipment_ids', '[]'::jsonb,
                     'credential', null))",
    )
    .bind(reference.delivery_id)
    .bind(resend_prepare_command_id(obligation, 2).expect("prepare ID"))
    .bind(serde_json::to_value(body_digest).expect("摘要"))
    .execute(pool)
    .await
    .expect("写入发送冻结");
    sqlx::query(
        "INSERT INTO public.settlement_fulfillment_dispatches
            (obligation_id, dispatch_no, delivery_id, authorized_by, recorded_at)
         VALUES ($1, 2, $2, $3, now())",
    )
    .bind(obligation)
    .bind(reference.delivery_id)
    .bind(returned.case)
    .execute(pool)
    .await
    .expect("登记第 2 次派发");
    sqlx::query(
        "UPDATE public.settlement_obligation_fulfillments
         SET execution_state = 'enqueued', delivery_id = $2,
             prepare_command_id = $3, prepare_request_digest = decode(repeat('79', 32), 'hex'),
             activate_command_id = $4, activate_request_digest = decode(repeat('7a', 32), 'hex'),
             state_version = state_version + 1, updated_at = now()
         WHERE obligation_id = $1",
    )
    .bind(obligation)
    .bind(reference.delivery_id)
    .bind(resend_prepare_command_id(obligation, 2).expect("prepare ID"))
    .bind(Uuid::new_v4())
    .execute(pool)
    .await
    .expect("履约指向第 2 次派发");

    let parsed = parse_returned_settlement_body(&format!(
        "{original_body}\n\nThis message was returned from recovery seller, kupo!"
    ))
    .expect("系统后缀完整");
    let hex: String = parsed.body_digest.iter().map(|byte| format!("{byte:02x}")).collect();
    let subject = format!("[RTS] Auction seller payout [{obligation}]");
    let bound = insert_inbound(
        pool,
        "993012",
        Utc::now(),
        Some(("MoogleMail", subject.clone())),
        json!({"obligation_id": obligation.to_string(), "credits": PAYOUT, "cod": 0,
               "attachment_ids": [], "returned_from": parsed.returned_from,
               "body_digest": hex, "dispatch_reference": parsed.dispatch_reference}),
    )
    .await;
    let consumed =
        store.consume_due_settlement_return(Utc::now()).await.expect("消费").expect("必须消费");
    assert_eq!(consumed.return_mail_id, bound);
    assert_eq!(consumed.delivery_id, Some(reference.delivery_id));
    assert_eq!(consumed.decision.matched, SettlementReturnMatch::ManualReconcile);
    let dispatch_no: Option<i32> = sqlx::query_scalar(
        "SELECT dispatch_no FROM public.settlement_return_cases WHERE return_mail_id = $1",
    )
    .bind(bound)
    .fetch_one(pool)
    .await
    .expect("读取新案件");
    assert_eq!(dispatch_no, Some(2));

    let unreferenced = insert_inbound(
        pool,
        "993013",
        Utc::now(),
        Some(("MoogleMail", subject)),
        evidence(obligation, PAYOUT, &[]),
    )
    .await;
    let consumed =
        store.consume_due_settlement_return(Utc::now()).await.expect("消费").expect("必须消费");
    assert_eq!(consumed.return_mail_id, unreferenced);
    assert_eq!(
        (consumed.decision.matched, consumed.decision.reason),
        (SettlementReturnMatch::Unbound, SettlementReturnReason::DispatchAlreadyReturned),
        "没有引用只认首寄，首寄已由第一案绑定"
    );

    pool.close().await;
}

/// 未绑定退件经只读核验：读到系统来源、资产与唯一首寄时改绑为待决定并把履约转人工；读不到时
/// 保持隔离；untrusted 不能核验。
#[tokio::test]
async fn postgres_return_recheck_rebinds_only_with_proof() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let (minute, settled_at) = settlement_minute();
    let sale = insert_sale(pool, 7_600_031, settled_at, 1).await;
    insert_sent_fulfillment(
        pool,
        sale.seller_obligation,
        "settlement_seller_payout",
        PAYOUT,
        minute + Duration::minutes(10),
        "enqueued",
    )
    .await;
    let subject = format!("[RTS] Auction seller payout [{}]", sale.seller_obligation);
    // 证据被历史程序写坏（缺附件字段）：消费时只能隔离为 unbound。
    let case = insert_inbound(
        pool,
        "993031",
        minute + Duration::minutes(40),
        Some(("MoogleMail", subject.clone())),
        json!({"obligation_id": sale.seller_obligation.to_string(), "credits": PAYOUT, "cod": 0}),
    )
    .await;
    let consumed =
        store.consume_due_settlement_return(Utc::now()).await.expect("消费").expect("必须消费");
    assert_eq!(consumed.decision.matched, SettlementReturnMatch::Unbound);
    assert_eq!(
        handle(&store, case, 1, SettlementReturnAction::Recheck).await,
        SettlementReturnCaseStatus::RecheckPending
    );

    let claim = store
        .claim_due_return_recheck(Utc::now(), Utc::now() + Duration::minutes(5))
        .await
        .expect("领取核验")
        .expect("必须领取");
    assert_eq!((claim.return_mail_id, claim.external_mail_id.as_str()), (case, "993031"));
    assert!(
        store
            .claim_due_return_recheck(Utc::now(), Utc::now() + Duration::minutes(5))
            .await
            .expect("再次领取")
            .is_none(),
        "租约内不重复领取"
    );
    let progress = store
        .record_return_recheck(
            claim,
            SettlementReturnRecheckOutcome::Observed(SettlementReturnMailObservation {
                sender: "MoogleMail".to_owned(),
                subject,
                body: None,
                credits: PAYOUT,
                cod: 0,
                attachment_ids: Vec::new(),
                observed_at: Utc::now(),
            }),
        )
        .await
        .expect("写回核验");
    assert_eq!(
        (progress.status, progress.event_kind),
        (SettlementReturnCaseStatus::AwaitingDecision, "recheck_rebound")
    );
    let (state, error): (String, Option<String>) = sqlx::query_as(
        "SELECT execution_state, last_error_code FROM public.settlement_obligation_fulfillments
         WHERE obligation_id = $1",
    )
    .bind(sale.seller_obligation)
    .fetch_one(pool)
    .await
    .expect("读取履约");
    assert_eq!(
        (state.as_str(), error.as_deref()),
        ("manual_reconcile", Some("settlement_mail_returned"))
    );
    let detail = store.settlement_return_detail(case).await.expect("改绑后详情");
    assert!(detail.allowed_actions.contains(&SettlementReturnAction::ReclaimAndHold));

    // 第二封未绑定退件：只读核验读不到邮件，保持隔离。
    let missing = insert_inbound(
        pool,
        "993032",
        minute + Duration::minutes(45),
        Some(("MoogleMail", format!("[RTS] Auction buyer refund [{}]", Uuid::new_v4()))),
        json!({"obligation_id": "broken"}),
    )
    .await;
    store.consume_due_settlement_return(Utc::now()).await.expect("消费").expect("必须消费");
    handle(&store, missing, 1, SettlementReturnAction::Recheck).await;
    let claim = store
        .claim_due_return_recheck(Utc::now(), Utc::now() + Duration::minutes(5))
        .await
        .expect("领取")
        .expect("必须领取");
    let progress = store
        .record_return_recheck(
            claim,
            SettlementReturnRecheckOutcome::NotFound { observed_at: Utc::now() },
        )
        .await
        .expect("写回核验");
    assert_eq!(
        (progress.status, progress.event_kind),
        (SettlementReturnCaseStatus::Quarantined, "recheck_unchanged")
    );

    pool.close().await;
}

/// 新事实表只追加：运行账号能插入，但改写或删除命令、事件、领回、派发历史、资金绑定与资金返还
/// 都被拒绝。
#[tokio::test]
async fn postgres_return_facts_are_append_only_and_runtime_accessible() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let returned = returned_payout(&store, 7_600_041, "993041").await;
    seed_spent_payout(pool, &returned).await;
    handle(&store, returned.case, 1, SettlementReturnAction::ReclaimAndResend).await;
    take_through(&store, returned.case, true).await;
    for (table, column) in [
        ("settlement_return_commands", "return_mail_id"),
        ("settlement_return_events", "return_mail_id"),
        ("settlement_return_takes", "return_mail_id"),
        ("settlement_funding_returns", "return_mail_id"),
        ("settlement_funding_delivery_bindings", "delivery_id"),
        ("settlement_fulfillment_dispatches", "obligation_id"),
    ] {
        let id = match column {
            "delivery_id" => returned.delivery,
            "obligation_id" => returned.sale.seller_obligation,
            _ => returned.case,
        };
        let update = format!("UPDATE public.{table} SET {column} = {column} WHERE {column} = $1");
        assert!(
            sqlx::query(sqlx::AssertSqlSafe(update)).bind(id).execute(pool).await.is_err(),
            "{table} UPDATE"
        );
        let delete = format!("DELETE FROM public.{table} WHERE {column} = $1");
        assert!(
            sqlx::query(sqlx::AssertSqlSafe(delete)).bind(id).execute(pool).await.is_err(),
            "{table} DELETE"
        );
    }
    // 开案、领回请求、Take 激活、领回证明各一条事件，版本连续。
    assert_eq!(
        count(
            pool,
            "SELECT count(*) FROM public.settlement_return_events WHERE return_mail_id = $1",
            returned.case
        )
        .await,
        4
    );

    pool.close().await;
}
