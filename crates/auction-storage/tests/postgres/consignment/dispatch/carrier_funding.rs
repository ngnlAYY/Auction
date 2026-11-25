//! isekai 载体派发的本金占用：起写同事务 reserve → in_flight，Sent 绑定同事务 spent（D9 §4.4）。
//!
//! 只测核心规则：起写同事务预留且只预留一次、自有不足失败关闭且零外写、起写回滚资金零痕迹、Sent 证明后
//! 结清、record_only 首封由 `record_carrier` 发行。夹具走真实购买首轮原语与派发端口。

use auction_application::consignment::{
    ConsignmentError, DispatchStore, MarkWriteStarted, RecordEvidenceInput, WriteStart,
    WriteStartOutcome,
};
use auction_application::core::RequestHash;
use auction_application::delivery::DeliverySendCredentialBinding;
use auction_application::settlement::{
    INTERNAL_EVIDENCE_PROTOCOL, InternalEvidencePurpose, MailAccountEvidenceBinding,
    MailRuntimeBinding, SettlementInternalEvidenceSnapshot, VerifiedMainCashObservation,
};
use auction_domain::consignment::dispatch::{
    AttemptKey, EvidenceCategory, FrozenDispatchIdentity, dispatch_attempt_id, dispatch_ref,
};
use auction_domain::consignment::{
    ConsignmentContract, ConsignmentContractCore, ConsignmentObligationGraph,
    ConsignmentSalePolicy, FeeBps, FundingAccountingMode, ListingPrice, ListingVersion,
    SaleFundingFreeze, SaleTerms, SaleTermsInput, build_consignment_obligations,
};
use auction_domain::{
    CommandId, ConsignmentContractId, SettlementId, SettlementObligationKind, SettlementWorld,
    UserId,
};
use auction_storage::DatabaseStore;
use auction_time::{Duration, Timestamp};
use sqlx::PgPool;
use uuid::Uuid;

use super::super::contracts::fixture::{
    CONTRACT, Graph, OBLIGATION_1, OBLIGATION_2, OBLIGATION_3, OBLIGATION_4, WINDOW,
    seed_isekai_custody,
};
use super::super::deposit::support::{ASSET, LISTING, POSITION, USER_1, USER_2, succeeds};
use super::super::ledger136::fixture::{
    PRICE, PURCHASE_COMMAND, PURCHASED_AT, available_sql, purchase_command, purchase_listing,
    purchase_position,
};
use super::fixture::{BOT_UID, at, binding, prepare_send_credential, round_key, uuid};
use crate::support::TestDatabase;

/// 成交冻结的资金 UID（与合同图夹具一致）。
pub(in crate::consignment) const UID: &str = "7001";
/// 严格现金账户的初始自有余额。
pub(super) const OWN: i64 = 10;
/// 正例证据观察到的 main 余额：足以覆盖夹具里该 UID 的全部未支出占用。
pub(in crate::consignment) const BALANCE: i64 = 1_000_000;

/// 发信会话里取得的资金证据替身：绑定本次尝试、投递租约、投递冻结身份摘要（投递的请求摘要）与凭据；
/// `credits` 为 `None` 是记账模式（不读余额）。`observed` 早于有效期即为不新鲜的观察。
pub(in crate::consignment) async fn cash_evidence(
    pool: &PgPool,
    attempt: AttemptKey,
    token: Uuid,
    credential: &DeliverySendCredentialBinding,
    uid: &str,
    credits: Option<i64>,
    observed: Timestamp,
) -> SettlementInternalEvidenceSnapshot {
    let digest: Vec<u8> =
        sqlx::query_scalar("SELECT request_digest FROM public.deliveries WHERE id = $1")
            .bind(attempt.delivery_id)
            .fetch_one(pool)
            .await
            .expect("投递请求摘要");
    let observed = observed.duration_trunc(Duration::microseconds(1)).expect("截断到微秒");
    let mut identity = MailAccountEvidenceBinding {
        credential_id: credential.credential_id,
        credential_version: credential.credential_version,
        runtime: MailRuntimeBinding { generation: credential.runtime_generation },
        authenticated_uid: uid.to_owned(),
        player_name: "Fake Bot".to_owned(),
        world: SettlementWorld::Main,
        observed_at: observed,
        protocol_version: INTERNAL_EVIDENCE_PROTOCOL,
        digest: [0; 32],
    };
    identity.digest = identity.compute_digest();
    let main_cash = credits.map(|credits| {
        let mut cash = VerifiedMainCashObservation {
            binding: identity.clone(),
            credits,
            observed_at: observed,
            digest: [0; 32],
        };
        cash.digest = cash.compute_digest();
        cash
    });
    let mut evidence = SettlementInternalEvidenceSnapshot {
        operation_id: dispatch_attempt_id(&attempt),
        purpose: InternalEvidencePurpose::DeliveryClaim {
            delivery_id: attempt.delivery_id,
            claim_token: token,
            claim_fencing: i64::try_from(attempt.fence).expect("fence"),
            request_hash: RequestHash::new(<[u8; 32]>::try_from(digest.as_slice()).expect("摘要")),
        },
        identity,
        equipment: None,
        main_cash,
        sent_collection: None,
        observed_from: observed,
        expires_at: observed + Duration::seconds(30),
        digest: [0; 32],
    };
    evidence.digest = evidence.compute_digest();
    evidence
}

/// 夹具凭据上、成交冻结 UID 的新鲜证据；严格现金观察到 [`BALANCE`]，记账模式不读余额。
pub(super) async fn fresh_evidence(
    pool: &PgPool,
    mode: Mode,
    attempt: AttemptKey,
    token: Uuid,
) -> SettlementInternalEvidenceSnapshot {
    let credits = (mode != Mode::RecordOnly).then_some(BALANCE);
    cash_evidence(pool, attempt, token, &binding(), UID, credits, Timestamp::now()).await
}

/// 资金账户的会计模式与严格现金的初始自有余额。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Mode {
    Strict { own: i64 },
    RecordOnly,
}

/// 载体付款首轮的冻结身份：main 派发、1 Credit、COD P+1、无附件。
pub(super) fn carrier_frozen() -> FrozenDispatchIdentity {
    FrozenDispatchIdentity {
        rights_holder_identity: "FakeSellerTwo".to_owned(),
        bot_uid: BOT_UID.to_owned(),
        world: SettlementWorld::Main,
        subject: "fake subject".to_owned(),
        template: "consignment_payment_carrier".to_owned(),
        body_digest: [0xd1; 32],
        round_ref: dispatch_ref(&round_key()),
        d0: at(PURCHASED_AT),
        credits: 1,
        cod: PRICE + 1,
        equipment_ids: Vec::new(),
    }
}

/// 与 SQL 夹具同 ID 的 isekai Sale 图；首项是载体付款义务。
fn isekai_graph(mode: Mode) -> ConsignmentObligationGraph {
    let core = ConsignmentContractCore {
        contract_id: ConsignmentContractId::from_uuid(uuid(CONTRACT)),
        origin_command_id: CommandId::from_uuid(uuid(PURCHASE_COMMAND)),
        listing_id: uuid(LISTING),
        position_id: uuid(POSITION),
        generation: 1,
        custody_equipment_id: uuid(ASSET),
        world: SettlementWorld::Isekai,
        canonical_eid: "42".to_owned(),
        season_window_id: Some(uuid(WINDOW)),
        seller_user_id: UserId::from_uuid(uuid(USER_1)),
        frozen_seller_identity: "FakeSellerOne".to_owned(),
        created_at: at("2026-01-02T00:00:00Z"),
    };
    let terms = SaleTerms::new(SaleTermsInput {
        buyer_user_id: UserId::from_uuid(uuid(USER_2)),
        frozen_buyer_identity: "FakeSellerTwo".to_owned(),
        listing_version: ListingVersion::new(4).expect("版本"),
        price: ListingPrice::new(PRICE).expect("标价"),
        fee_bps: FeeBps::new(500).expect("费率"),
        payment_deadline_at: at("2026-01-09T00:00:00Z"),
        policy: ConsignmentSalePolicy::new(7, 3, 5).expect("策略"),
        funding: SaleFundingFreeze {
            uid: UID.to_owned(),
            mode: match mode {
                Mode::Strict { .. } => FundingAccountingMode::StrictCash,
                Mode::RecordOnly => FundingAccountingMode::RecordOnly,
            },
            boundary_at: at("2026-01-02T00:00:00Z"),
            boundary_source_id: None,
            account_version: 0,
        },
    })
    .expect("成交条款");
    let mut graph =
        build_consignment_obligations(&ConsignmentContract::Sale { core, terms }).expect("义务图");
    assert_eq!(graph.obligations[0].kind, SettlementObligationKind::WinnerPaymentCarrierCod);
    for (obligation, id) in
        graph.obligations.iter_mut().zip([OBLIGATION_1, OBLIGATION_2, OBLIGATION_3, OBLIGATION_4])
    {
        obligation.id = SettlementId::from_uuid(uuid(id));
    }
    graph
}

/// 已购 isekai 合同、载体付款首轮已建（prepared）的测试库。
pub(super) struct Carrier {
    _database: TestDatabase,
    pub(super) mode: Mode,
    pub(super) owner: DatabaseStore,
    pub(super) runtime: DatabaseStore,
    pub(super) delivery_id: Uuid,
}

impl Carrier {
    pub(super) async fn purchased(mode: Mode) -> Self {
        let database = TestDatabase::create().await.expect("克隆模板库");
        let owner = database.maintenance_store().await.expect("以维护账号连接");
        let runtime = database.store().await.expect("以运行账号连接");
        let pool = owner.postgres_pool();
        seed_isekai_custody(pool, "transaction_timestamp() + interval '10 days'").await;
        succeeds(pool, &available_sql()).await;
        prepare_send_credential(pool).await;
        let account_mode = if mode == Mode::RecordOnly { "record_only" } else { "strict_cash" };
        succeeds(
            pool,
            &format!(
                "INSERT INTO public.settlement_funding_accounts (uid, status, accounting_mode)
                 VALUES ('{UID}', 'ready', '{account_mode}');"
            ),
        )
        .await;
        if let Mode::Strict { own } = mode
            && own > 0
        {
            fund_own(pool, own).await;
        }
        let graph = Graph {
            price: PRICE,
            fee: 5,
            seller_amount: 96,
            payout_amount: 96,
            ..Graph::sale_isekai()
        }
        .sql(PURCHASE_COMMAND)
        .replace("'strict_cash'", &format!("'{account_mode}'"));
        let head =
            purchase_command().replace("\"world\":\"main\"", "\"world\":\"isekai\"") + &graph;
        let tail = purchase_position() + &purchase_listing();
        let (_round, delivery_id) = owner
            .test_purchase_with_first_round(&head, &isekai_graph(mode), &carrier_frozen(), &tail)
            .await
            .expect("购买事务含载体首轮");
        Self { _database: database, mode, owner, runtime, delivery_id }
    }

    pub(super) fn pool(&self) -> &PgPool {
        self.owner.postgres_pool()
    }

    pub(super) fn service(&self) -> auction_application::consignment::DispatchService {
        auction_application::consignment::DispatchService::new(std::sync::Arc::new(
            self.runtime.clone(),
        ))
    }

    /// 模拟通用调度器的激活与领取，写入租约与 fence。
    async fn claim(&self, token: Uuid, fence: i64) {
        succeeds(
            self.pool(),
            &format!(
                "UPDATE public.deliveries
                 SET status = 'pending', activation_command_id = gen_random_uuid(),
                     activation_request_digest = decode(repeat('ab', 32), 'hex'),
                     activated_at = statement_timestamp()
                 WHERE id = '{delivery}' AND status = 'prepared';",
                delivery = self.delivery_id
            ),
        )
        .await;
        self.reclaim(token, fence).await;
    }

    /// 换租约与 fence；已 sending 的投递保持 sending。
    async fn reclaim(&self, token: Uuid, fence: i64) {
        succeeds(
            self.pool(),
            &format!(
                "UPDATE public.deliveries
                 SET status = CASE WHEN status = 'sending' THEN 'sending' ELSE 'claimed' END,
                     lease_token = '{token}', lease_expires_at = clock_timestamp() + interval '5 minutes',
                     fencing_version = {fence}, attempt_count = attempt_count + 1,
                     claimed_at = COALESCE(claimed_at, statement_timestamp())
                 WHERE id = '{delivery}';",
                delivery = self.delivery_id
            ),
        )
        .await;
    }

    /// 领取 fence 1 并经派发服务起写（带同会话的新鲜资金证据）；返回尝试键与起写结果。
    pub(super) async fn start(&self) -> (AttemptKey, Result<WriteStart, ConsignmentError>) {
        let (attempt, token) = self.claim_and_register().await;
        let evidence = fresh_evidence(self.pool(), self.mode, attempt, token).await;
        let started = self
            .service()
            .mark_cash_write_started(attempt, token, Some(binding()), &evidence)
            .await;
        (attempt, started)
    }

    /// 领取 fence 1 并登记尝试；返回尝试键与投递租约 token。
    pub(super) async fn claim_and_register(&self) -> (AttemptKey, Uuid) {
        let token = Uuid::new_v4();
        self.claim(token, 1).await;
        let attempt = AttemptKey { delivery_id: self.delivery_id, fence: 1 };
        self.service()
            .start_attempt(round_key(), attempt, token, carrier_frozen())
            .await
            .expect("登记尝试");
        (attempt, token)
    }
}

/// 给严格现金账户写入原 UID 自有余额。
pub(super) async fn fund_own(pool: &PgPool, own: i64) {
    succeeds(
        pool,
        &format!(
            "INSERT INTO public.settlement_funding_allocations (id, uid, purpose, amount)
             VALUES (gen_random_uuid(), '{UID}', 'proprietary', {own});"
        ),
    )
    .await;
}

/// 载体分配的 (状态, 金额, 绑定投递)；不存在为 `None`。
pub(super) async fn carrier(pool: &PgPool) -> Option<(String, i64, Option<Uuid>)> {
    sqlx::query_as(
        "SELECT state, amount, delivery_id FROM public.settlement_funding_allocations
         WHERE uid = $1 AND purpose = 'carrier'",
    )
    .bind(UID)
    .fetch_optional(pool)
    .await
    .expect("载体分配")
}

pub(super) async fn own(pool: &PgPool) -> i64 {
    sqlx::query_scalar(
        "SELECT COALESCE(sum(amount), 0)::bigint FROM public.settlement_funding_allocations
         WHERE uid = $1 AND purpose = 'proprietary'",
    )
    .bind(UID)
    .fetch_one(pool)
    .await
    .expect("自有余额")
}

/// 资金账本快照：(分录数, 来源数, 资金命令数, 分配数, 账户版本)。
pub(super) async fn ledger(pool: &PgPool) -> (i64, i64, i64, i64, i64) {
    sqlx::query_as(
        "SELECT (SELECT count(*) FROM public.settlement_funding_entries WHERE uid = $1),
                (SELECT count(*) FROM public.settlement_funding_sources WHERE uid = $1),
                (SELECT count(*) FROM public.settlement_funding_commands WHERE uid = $1),
                (SELECT count(*) FROM public.settlement_funding_allocations WHERE uid = $1),
                (SELECT state_version FROM public.settlement_funding_accounts WHERE uid = $1)",
    )
    .bind(UID)
    .fetch_one(pool)
    .await
    .expect("资金账本")
}

/// 某类分录的 (条数, 金额合计)。
pub(super) async fn entries(pool: &PgPool, kind: &str) -> (i64, i64) {
    sqlx::query_as(
        "SELECT count(*), COALESCE(sum(amount), 0)::bigint FROM public.settlement_funding_entries
         WHERE uid = $1 AND kind = $2",
    )
    .bind(UID)
    .bind(kind)
    .fetch_one(pool)
    .await
    .expect("分录")
}

/// (冻结 Send 尝试数, 寄售起写尝试数, 起写账本命令数)：前两项即远端 POST 许可数。
pub(super) async fn send_writes(pool: &PgPool, delivery: Uuid) -> (i64, i64, i64) {
    sqlx::query_as(
        "SELECT (SELECT count(*) FROM public.delivery_send_attempts WHERE delivery_id = $1),
                (SELECT count(*) FROM public.consignment_dispatch_attempts WHERE delivery_id = $1),
                (SELECT count(*) FROM public.consignment_commands
                 WHERE operation = 'mark_write_started')",
    )
    .bind(delivery)
    .fetch_one(pool)
    .await
    .expect("起写计数")
}

/// 故障注入：订单阶段事件写入失败（起写与证据事务的最后一笔写入之一）。
pub(super) async fn inject_order_event_failure(pool: &PgPool) {
    succeeds(
        pool,
        "CREATE FUNCTION public.fake_fail_carrier_order_event() RETURNS trigger
         LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'fake injected failure'; END $$;
         CREATE TRIGGER fake_fail_carrier_order_event BEFORE INSERT ON public.realtime_events
         FOR EACH ROW WHEN (NEW.kind = 'consignment.order_stage_changed')
         EXECUTE FUNCTION public.fake_fail_carrier_order_event();",
    )
    .await;
}

pub(super) async fn clear_order_event_failure(pool: &PgPool) {
    succeeds(pool, "DROP TRIGGER fake_fail_carrier_order_event ON public.realtime_events;").await;
}

#[tokio::test]
async fn postgres_consignment_dispatch_carrier_write_start_reserves_the_principal_once() {
    let seeded = Carrier::purchased(Mode::Strict { own: OWN }).await;
    let pool = seeded.pool();
    let token = Uuid::new_v4();
    seeded.claim(token, 1).await;
    let attempt = AttemptKey { delivery_id: seeded.delivery_id, fence: 1 };
    let command = |byte: u8| MarkWriteStarted {
        command_id: CommandId::from_uuid(Uuid::from_u128(
            0x5200_0000_0000_5000_8000_0000_0000_0000 | u128::from(byte),
        )),
        request_hash: RequestHash::new([byte; 32]),
        attempt,
        claim_token: token,
        credential: Some(binding()),
    };
    // 两个执行者以不同命令 ID 同时起写：恰一个首次提交，本金只预留一次。
    let evidence = fresh_evidence(pool, seeded.mode, attempt, token).await;
    let (left, right) = tokio::join!(
        seeded.runtime.mark_cash_write_started(command(1), &evidence),
        seeded.runtime.mark_cash_write_started(command(2), &evidence),
    );
    let firsts = [&left, &right]
        .iter()
        .filter(|outcome| matches!(outcome, Ok(WriteStartOutcome::FirstCommitted(_))))
        .count();
    assert_eq!(firsts, 1, "{left:?} {right:?}");
    // 正例：严格现金由原 UID 自有借出 1，占用在起写同事务转在途并绑定该投递。
    assert_eq!(carrier(pool).await, Some(("in_flight".to_owned(), 1, Some(seeded.delivery_id))));
    assert_eq!(own(pool).await, OWN - 1);
    assert_eq!(entries(pool, "transfer").await, (1, 1));
    assert_eq!(entries(pool, "reserve").await, (1, 1));
    assert_eq!(entries(pool, "in_flight").await, (1, 1));
    assert_eq!(send_writes(pool, seeded.delivery_id).await.0, 1);
    let after = ledger(pool).await;

    // 反例：胜出命令重放、过租后新 fence 起写都只得“已提交”，不再预留。
    let winner = if matches!(left, Ok(WriteStartOutcome::FirstCommitted(_))) { 1 } else { 2 };
    let replay = seeded.runtime.mark_write_started(command(winner)).await.expect("重放");
    assert!(matches!(replay, WriteStartOutcome::AlreadyCommitted(_)));
    let new_token = Uuid::new_v4();
    seeded.reclaim(new_token, 2).await;
    let retry = AttemptKey { delivery_id: seeded.delivery_id, fence: 2 };
    let again = seeded.service().mark_write_started(retry, new_token, Some(binding())).await;
    assert!(matches!(again, Ok(WriteStart::AlreadyCommitted(_))), "{again:?}");
    assert_eq!(ledger(pool).await, after, "重放与新 fence 均零资金写入");
    assert_eq!(own(pool).await, OWN - 1);
    assert_eq!(send_writes(pool, seeded.delivery_id).await.0, 1, "POST 许可仍只有一次");
}

#[tokio::test]
async fn postgres_consignment_dispatch_carrier_write_start_without_own_cash_fails_closed() {
    let seeded = Carrier::purchased(Mode::Strict { own: 0 }).await;
    let pool = seeded.pool();
    let before = ledger(pool).await;
    // 反例：原 UID 无自有余额可借，起写整笔回滚：零许可、零起写事实、零账本回执、资金零痕迹。
    let (attempt, denied) = seeded.start().await;
    assert_eq!(denied.expect_err("自有不足"), ConsignmentError::Unavailable);
    assert_eq!(send_writes(pool, seeded.delivery_id).await, (0, 0, 0));
    assert_eq!(ledger(pool).await, before);
    assert_eq!(carrier(pool).await, None);
    let status: String = sqlx::query_scalar("SELECT status FROM public.deliveries WHERE id = $1")
        .bind(seeded.delivery_id)
        .fetch_one(pool)
        .await
        .expect("投递状态");
    assert_eq!(status, "claimed");

    // 暂不可用不占死命令 ID：补足自有后同一命令得到唯一许可。
    fund_own(pool, OWN).await;
    let token: Uuid = sqlx::query_scalar("SELECT lease_token FROM public.deliveries WHERE id = $1")
        .bind(seeded.delivery_id)
        .fetch_one(pool)
        .await
        .expect("租约");
    let evidence = fresh_evidence(pool, seeded.mode, attempt, token).await;
    let started =
        seeded.service().mark_cash_write_started(attempt, token, Some(binding()), &evidence).await;
    assert!(matches!(started, Ok(WriteStart::FirstCommitted { .. })), "{started:?}");
    assert_eq!(own(pool).await, OWN - 1);
    assert_eq!(send_writes(pool, seeded.delivery_id).await, (1, 1, 1));
}

#[tokio::test]
async fn postgres_consignment_dispatch_carrier_write_start_rollback_leaves_no_funding_trace() {
    let seeded = Carrier::purchased(Mode::Strict { own: OWN }).await;
    let pool = seeded.pool();
    let before = ledger(pool).await;
    // 故障注入：起写事务在资金占用之后的订单阶段事件处失败。
    inject_order_event_failure(pool).await;
    let (_attempt, failed) = seeded.start().await;
    assert!(failed.is_err(), "{failed:?}");
    assert_eq!(ledger(pool).await, before, "资金占用随起写一并回滚");
    assert_eq!(own(pool).await, OWN);
    assert_eq!(carrier(pool).await, None);
    assert_eq!(send_writes(pool, seeded.delivery_id).await, (0, 0, 0));
    clear_order_event_failure(pool).await;
}

#[tokio::test]
async fn postgres_consignment_dispatch_carrier_sent_evidence_spends_the_principal() {
    let seeded = Carrier::purchased(Mode::Strict { own: OWN }).await;
    let pool = seeded.pool();
    let (attempt, started) = seeded.start().await;
    assert!(matches!(started, Ok(WriteStart::FirstCommitted { .. })), "{started:?}");
    let sent = RecordEvidenceInput {
        attempt,
        category: EvidenceCategory::SentBinding,
        remote_observation_id: "9101".to_owned(),
        observed_at: at("2026-01-01T05:00:00Z"),
        validator_version: "fake-validator-v1".to_owned(),
        evidence_digest: [7; 32],
    };
    let service = seeded.service();

    // 故障注入：证据事务在资金结清之后的订单阶段事件处失败，Sent 绑定与 spent 一并回滚。
    let before = ledger(pool).await;
    inject_order_event_failure(pool).await;
    assert!(service.record_evidence(sent.clone()).await.is_err());
    clear_order_event_failure(pool).await;
    assert_eq!(ledger(pool).await, before);
    assert_eq!(carrier(pool).await.map(|row| row.0), Some("in_flight".to_owned()));

    // 正例：Sent 绑定同事务把占用转 spent（已证外达），金额与绑定投递不变，自有不回补。
    assert!(!service.record_evidence(sent.clone()).await.expect("Sent 证据").replayed);
    assert_eq!(carrier(pool).await, Some(("spent".to_owned(), 1, Some(seeded.delivery_id))));
    assert_eq!(entries(pool, "spent").await, (1, 1));
    assert_eq!(own(pool).await, OWN - 1);
    // 重放不重复结清。
    let after = ledger(pool).await;
    assert!(service.record_evidence(sent).await.expect("重放").replayed);
    assert_eq!(ledger(pool).await, after);
}

#[tokio::test]
async fn postgres_consignment_dispatch_record_only_carrier_write_start_issues_record_carrier() {
    let seeded = Carrier::purchased(Mode::RecordOnly).await;
    let pool = seeded.pool();
    let (_attempt, started) = seeded.start().await;
    assert!(matches!(started, Ok(WriteStart::FirstCommitted { .. })), "{started:?}");
    // record_only 无个人余额：首封由专用 record_carrier 来源向 carrier 发行 1，不借任何自有。
    let sources: (i64, i64) = sqlx::query_as(
        "SELECT count(*), COALESCE(sum(issued_amount), 0)::bigint
         FROM public.settlement_funding_sources
         WHERE uid = $1 AND kind = 'record_carrier' AND source_key = $2",
    )
    .bind(UID)
    .bind(seeded.delivery_id)
    .fetch_one(pool)
    .await
    .expect("载体来源");
    assert_eq!(sources, (1, 1));
    assert_eq!(entries(pool, "transfer").await, (0, 0));
    assert_eq!(carrier(pool).await, Some(("in_flight".to_owned(), 1, Some(seeded.delivery_id))));
    assert_eq!(send_writes(pool, seeded.delivery_id).await, (1, 1, 1));
}
