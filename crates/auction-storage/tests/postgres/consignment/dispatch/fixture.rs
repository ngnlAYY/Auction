//! 派发夹具：购买事务写序（回执、合同图、首轮、position、挂单）与“首派已起写”存量构造器，供第 7 期复用。
//!
//! 合同图用不经 `write_graph` 的原始 SQL 写入（图写入端口的第 6 期兼容由 6A2 修复），
//! 义务 ID 与 SQL 夹具一致；首轮原语经 `test_purchase_with_first_round` 在同一事务里运行。

use auction_application::consignment::{
    AttemptReceipt, ConsignmentError, DispatchService, WriteStart,
};
use auction_application::delivery::DeliverySendCredentialBinding;
use auction_domain::SettlementObligationKind;
use auction_domain::consignment::dispatch::{
    AttemptKey, DispatchRoundKey, FrozenDispatchIdentity, dispatch_ref,
};
use auction_domain::consignment::{
    ConsignmentContract, ConsignmentContractCore, ConsignmentObligationGraph,
    ConsignmentSalePolicy, FeeBps, FundingAccountingMode, ListingPrice, ListingVersion,
    SaleFundingFreeze, SaleTerms, SaleTermsInput, build_consignment_obligations,
};
use auction_domain::{CommandId, ConsignmentContractId, SettlementId, SettlementWorld, UserId};
use auction_storage::{
    DatabaseStore, DispatchWorkRepository, DownstreamDispatchContentSource,
    DownstreamDispatchRequest, FirstDispatchContent, FirstDispatchContentSource,
    FirstDispatchRequest, WithdrawalDispatchRequest,
};
use auction_time::Timestamp;
use sqlx::PgPool;
use uuid::Uuid;

use super::super::contracts::fixture::{
    CONTRACT, Graph, OBLIGATION_1, OBLIGATION_2, OBLIGATION_3, seed_main_custody,
};
use super::super::deposit::support::{
    ACCEPTED, ASSET, ATTEMPT, CREDENTIAL, DECISION, LISTING, POSITION, PROOF, USER_1, USER_2,
    succeeds,
};
use crate::support::TestDatabase;

/// 取得证明后上架（deposit_accepted -> available）的回执命令。
const AVAILABLE_COMMAND: &str = "c6200000-0000-5000-8000-000000000001";
/// 买家购买命令（v4），也是合同的来源命令。
pub(super) const PURCHASE_COMMAND: &str = "c6100000-0000-4000-8000-000000000001";
const PURCHASED_AT: &str = "2026-01-01T04:00:00Z";
pub(super) const PRICE: i64 = 101;
/// 首轮 Send 的 Bot UID 与冻结文本；均为明显的假值。
pub(super) const BOT_UID: &str = "12345";

pub(super) fn uuid(text: &str) -> Uuid {
    Uuid::parse_str(text).expect("夹具 UUID")
}

pub(super) fn at(text: &str) -> Timestamp {
    Timestamp::parse_from_rfc3339(text).expect("夹具时间")
}

/// 首轮键：合同、付款义务、第 1 轮。
pub(super) fn round_key() -> DispatchRoundKey {
    DispatchRoundKey { contract_id: uuid(CONTRACT), obligation_id: uuid(OBLIGATION_1), round: 1 }
}

/// 起写必带的凭据绑定：夹具凭据版本 1、运行时 generation 1。
pub(in crate::consignment) fn binding() -> DeliverySendCredentialBinding {
    DeliverySendCredentialBinding {
        credential_id: uuid(CREDENTIAL),
        credential_version: 1,
        runtime_generation: 1,
    }
}

/// 起写锁验认 Worker 侧的 `mooglemail:<world>` 凭据命名空间，并要求运行时 generation 已提交。
pub(in crate::consignment) async fn prepare_send_credential(pool: &PgPool) {
    succeeds(
        pool,
        &format!(
            "UPDATE public.credential_keys SET scope = 'mooglemail:main' WHERE id = '{CREDENTIAL}';
             INSERT INTO public.mail_runtime_generation (scope, generation, activated_at)
             VALUES ('mail_cookie', 1, clock_timestamp()) ON CONFLICT (scope) DO NOTHING;"
        ),
    )
    .await;
}

/// 假内容来源：与首轮夹具同一模板与主题，正文摘要取 `body`，标的装备默认是规范装备。
pub(in crate::consignment) struct FakeContent {
    /// 正文摘要的填充字节。
    pub(super) body: u8,
    /// 为真时附件换成别的装备（渲染与合同不符的反例）。
    pub(super) wrong_equipment: bool,
}

impl FirstDispatchContentSource for FakeContent {
    fn content(
        &self,
        request: &FirstDispatchRequest<'_>,
    ) -> Result<FirstDispatchContent, ConsignmentError> {
        let equipment = if self.wrong_equipment { "43" } else { request.canonical_eid };
        Ok(FirstDispatchContent {
            template: "consignment_cod".to_owned(),
            subject: "fake subject".to_owned(),
            body_digest: [self.body; 32],
            equipment_ids: vec![equipment.to_owned()],
        })
    }

    fn withdrawal_content(
        &self,
        request: &WithdrawalDispatchRequest<'_>,
    ) -> Result<FirstDispatchContent, ConsignmentError> {
        let equipment = if self.wrong_equipment { "43" } else { request.canonical_eid };
        Ok(FirstDispatchContent {
            template: "consignment_equipment_withdrawal".to_owned(),
            subject: "fake withdrawal subject".to_owned(),
            body_digest: [0xd2; 32],
            equipment_ids: vec![equipment.to_owned()],
        })
    }
}

/// 下游首轮（isekai 原件交割、卖款、退款）的假内容：模板按义务种类，交割附规范装备、现金腿不带装备。
impl DownstreamDispatchContentSource for FakeContent {
    fn downstream_content(
        &self,
        request: &DownstreamDispatchRequest<'_>,
    ) -> Result<FirstDispatchContent, ConsignmentError> {
        let (template, equipment_ids) = match request.obligation.kind {
            SettlementObligationKind::WinnerEquipmentHandoff => {
                let equipment = if self.wrong_equipment { "43" } else { request.canonical_eid };
                ("consignment_equipment_handoff", vec![equipment.to_owned()])
            }
            SettlementObligationKind::SellerPayout => ("consignment_seller_payout", Vec::new()),
            SettlementObligationKind::BuyerRefund => ("consignment_buyer_refund", Vec::new()),
            _ => return Err(ConsignmentError::Unavailable),
        };
        Ok(FirstDispatchContent {
            template: template.to_owned(),
            subject: format!("fake {template} subject"),
            body_digest: [self.body; 32],
            equipment_ids,
        })
    }
}

/// 派发与补寄工作来源（被测端口），内容来源为 [`FakeContent`]。
pub(in crate::consignment) fn work_store(
    store: &DatabaseStore,
    content: FakeContent,
) -> DispatchWorkRepository {
    DispatchWorkRepository::new(store.clone(), std::sync::Arc::new(content))
}

/// 默认的假内容：正文摘要 0xe2、附件为规范装备。
pub(in crate::consignment) const CONTENT: FakeContent =
    FakeContent { body: 0xe2, wrong_equipment: false };

/// 首轮的冻结身份；`cod` 与付款义务金额一致。
pub(super) fn frozen(cod: i64) -> FrozenDispatchIdentity {
    FrozenDispatchIdentity {
        // 首轮 COD 的权利人是受益买方：起写授权核对轮上冻结的权利人与合同冻结买方一致。
        rights_holder_identity: "FakeSellerTwo".to_owned(),
        bot_uid: BOT_UID.to_owned(),
        world: SettlementWorld::Main,
        subject: "fake subject".to_owned(),
        template: "consignment_cod".to_owned(),
        body_digest: [0xd1; 32],
        round_ref: dispatch_ref(&round_key()),
        d0: at(PURCHASED_AT),
        credits: 0,
        cod,
        equipment_ids: vec!["42".to_owned()],
    }
}

/// 与 SQL 夹具同 ID 的 main Sale 图：付款、卖款、退款三条义务。
pub(super) fn sale_graph() -> ConsignmentObligationGraph {
    let core = ConsignmentContractCore {
        contract_id: ConsignmentContractId::from_uuid(uuid(CONTRACT)),
        origin_command_id: CommandId::from_uuid(uuid(PURCHASE_COMMAND)),
        listing_id: uuid(LISTING),
        position_id: uuid(POSITION),
        generation: 1,
        custody_equipment_id: uuid(ASSET),
        world: SettlementWorld::Main,
        canonical_eid: "42".to_owned(),
        season_window_id: None,
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
            uid: "7001".to_owned(),
            mode: FundingAccountingMode::StrictCash,
            boundary_at: at("2026-01-02T00:00:00Z"),
            boundary_source_id: None,
            account_version: 0,
        },
    })
    .expect("成交条款");
    let mut graph =
        build_consignment_obligations(&ConsignmentContract::Sale { core, terms }).expect("义务图");
    // 义务 ID 对齐原始 SQL 夹具的行。
    for (obligation, id) in
        graph.obligations.iter_mut().zip([OBLIGATION_1, OBLIGATION_2, OBLIGATION_3])
    {
        obligation.id = SettlementId::from_uuid(uuid(id));
    }
    graph
}

fn available_sql() -> String {
    let body = format!(
        "{{\"schema_version\":2,\"operation\":\"apply_mail_proof\",
          \"disposition\":\"take_proven\",\"command_id\":\"{AVAILABLE_COMMAND}\",
          \"decision_id\":\"{DECISION}\",\"proof_id\":\"{PROOF}\",
          \"listing_changes\":[{{\"listing_id\":\"{LISTING}\",\"listing_version\":5,
          \"state\":\"available\"}}]}}"
    );
    format!(
        "INSERT INTO public.consignment_commands
            (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
             request_digest, target_kind, target_id, listing_id, result_status, result_code,
             result_body, created_at)
         VALUES ('{AVAILABLE_COMMAND}', 2, 'worker', 'fake-worker', 'apply_mail_proof',
                 '{ATTEMPT}', decode(repeat('b1', 32), 'hex'),
                 'original_attempt', '{ATTEMPT}', NULL, 200, 'ok',
                 '{body}', '{ACCEPTED}');
         INSERT INTO public.consignment_events
            (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
             to_state, reason_code, price, occurred_at)
         VALUES ('{LISTING}', 5, '{AVAILABLE_COMMAND}', 'worker', 'available',
                 'deposit_accepted', 'available', NULL, {PRICE}, '{ACCEPTED}');
         UPDATE public.consignment_listings
         SET state = 'available', state_version = 5, updated_at = '{ACCEPTED}'
         WHERE id = '{LISTING}';"
    )
}

/// 购买前半：回执命令与合同图（义务图与履约行）。
fn purchase_head_sql() -> String {
    let body = format!(
        "{{\"schema_version\":2,\"operation\":\"purchase\",\"disposition\":\"purchased\",
          \"command_id\":\"{PURCHASE_COMMAND}\",\"listing_id\":\"{LISTING}\",
          \"listing_version\":6,\"state\":\"sale_pending\",\"contract_id\":\"{CONTRACT}\",
          \"contract_version\":1,\"equipment_id\":\"{ASSET}\",\"world\":\"main\",
          \"price\":{PRICE},\"order_state\":\"awaiting_payment\"}}"
    );
    let graph =
        Graph { price: PRICE, fee: 5, seller_amount: 96, payout_amount: 96, ..Graph::sale_main() };
    format!(
        "INSERT INTO public.consignment_commands
            (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
             request_digest, target_kind, target_id, listing_id, result_status, result_code,
             result_body, created_at)
         VALUES ('{PURCHASE_COMMAND}', 2, 'public_user', '{USER_2}', 'purchase', '{LISTING}',
                 decode(repeat('b2', 32), 'hex'), 'listing', '{LISTING}', '{LISTING}', 201,
                 'ok', '{body}', '{PURCHASED_AT}');{}",
        graph.sql(PURCHASE_COMMAND)
    )
}

/// 购买后半：position in_custody -> committed 与挂单 available -> sale_pending（各带事件）。
fn purchase_tail_sql() -> String {
    format!(
        "UPDATE public.consignment_custody_positions
         SET state = 'committed', version = 3, updated_at = '{PURCHASED_AT}'
         WHERE id = '{POSITION}';
         INSERT INTO public.consignment_custody_events
            (position_id, version, command_id, event_kind, from_state, to_state, proof_id,
             listing_id, listing_version, occurred_at)
         VALUES ('{POSITION}', 3, '{PURCHASE_COMMAND}', 'committed', 'in_custody', 'committed',
                 NULL, '{LISTING}', 6, '{PURCHASED_AT}');
         INSERT INTO public.consignment_events
            (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
             to_state, reason_code, price, occurred_at)
         VALUES ('{LISTING}', 6, '{PURCHASE_COMMAND}', 'public_user', 'purchased', 'available',
                 'sale_pending', NULL, {PRICE}, '{PURCHASED_AT}');
         UPDATE public.consignment_listings
         SET state = 'sale_pending', state_version = 6, updated_at = '{PURCHASED_AT}'
         WHERE id = '{LISTING}';"
    )
}

/// 一个测试库：代管中 -> available，随后可由测试执行购买与首轮。
pub(super) struct Seeded {
    _database: TestDatabase,
    /// 维护账号（夹具与计数）。
    pub(super) owner: DatabaseStore,
    /// 运行账号（被测端口）。
    pub(super) runtime: DatabaseStore,
    /// 首轮的投递；购买之前为空 UUID。
    pub(super) delivery_id: Uuid,
}

impl Seeded {
    /// 代管中的 main 件已上架（available），尚未购买。
    pub(super) async fn available() -> Self {
        let database = TestDatabase::create().await.expect("克隆模板库");
        let owner = database.maintenance_store().await.expect("以维护账号连接");
        let runtime = database.store().await.expect("以运行账号连接");
        seed_main_custody(owner.postgres_pool()).await;
        succeeds(owner.postgres_pool(), &available_sql()).await;
        prepare_send_credential(owner.postgres_pool()).await;
        Self { _database: database, owner, runtime, delivery_id: Uuid::nil() }
    }

    pub(super) fn pool(&self) -> &PgPool {
        self.owner.postgres_pool()
    }

    /// 购买事务写序：回执与合同图 -> 首轮原语 -> position 与挂单。返回 `(轮 ID, 投递 ID)`。
    pub(super) async fn purchase(
        &self,
        frozen: &FrozenDispatchIdentity,
    ) -> Result<(Uuid, Uuid), auction_application::consignment::ConsignmentError> {
        self.owner
            .test_purchase_with_first_round(
                &purchase_head_sql(),
                &sale_graph(),
                frozen,
                &purchase_tail_sql(),
            )
            .await
    }

    /// 已购买并建立首轮。
    pub(super) async fn purchased() -> Self {
        let mut seeded = Self::available().await;
        let (_round_id, delivery_id) =
            seeded.purchase(&frozen(PRICE)).await.expect("购买事务含首轮");
        seeded.delivery_id = delivery_id;
        seeded
    }

    /// 被测用例服务，经运行账号的存储端口。
    pub(super) fn service(&self) -> DispatchService {
        DispatchService::new(std::sync::Arc::new(self.runtime.clone()))
    }

    /// 模拟通用调度器的领取：prepared -> pending（激活）-> claimed，写入租约、fence 与计数。
    pub(super) async fn claim(&self, token: Uuid, fence: i64) {
        let delivery = self.delivery_id;
        sqlx::query(
            "UPDATE public.deliveries
             SET status = 'pending', activation_command_id = gen_random_uuid(),
                 activation_request_digest = decode(repeat('ab', 32), 'hex'),
                 activated_at = statement_timestamp()
             WHERE id = $1 AND status = 'prepared'",
        )
        .bind(delivery)
        .execute(self.pool())
        .await
        .expect("激活投递");
        self.reclaim(token, fence).await;
    }

    /// 换租约与 fence（过租后的新领取）；投递不改状态。
    pub(super) async fn reclaim(&self, token: Uuid, fence: i64) {
        let moved = sqlx::query(
            "UPDATE public.deliveries
             SET status = CASE WHEN status = 'sending' THEN 'sending' ELSE 'claimed' END,
                 lease_token = $2, lease_expires_at = clock_timestamp() + interval '5 minutes',
                 fencing_version = $3, attempt_count = attempt_count + 1,
                 claimed_at = COALESCE(claimed_at, statement_timestamp())
             WHERE id = $1",
        )
        .bind(self.delivery_id)
        .bind(token)
        .bind(fence)
        .execute(self.pool())
        .await
        .expect("领取投递");
        assert_eq!(moved.rows_affected(), 1);
    }
}

/// “首派已起写”的存量：第 7 期跨期验收直接复用。
pub(super) struct Started {
    /// 起写所在的尝试键。
    pub(super) attempt: AttemptKey,
    /// 尝试 ID。
    pub(super) attempt_receipt: AttemptReceipt,
    /// 起写时的领取 token；首次起写携带的许可已被消费，存量只保留事实。
    pub(super) claim_token: Uuid,
}

impl Seeded {
    /// 构造“首派已起写”：购买 -> 领取 -> 登记尝试 -> 起写（许可已消费）。
    pub(super) async fn first_dispatch_started() -> (Self, Started) {
        let seeded = Self::purchased().await;
        let started = seeded.start_first_dispatch(1).await;
        (seeded, started)
    }

    /// 以给定 fence 领取并起写首派，返回起写事实。
    pub(super) async fn start_first_dispatch(&self, fence: u64) -> Started {
        let token = Uuid::new_v4();
        self.claim(token, i64::try_from(fence).expect("fence")).await;
        let attempt = AttemptKey { delivery_id: self.delivery_id, fence };
        let service = self.service();
        let attempt_receipt = service
            .start_attempt(round_key(), attempt, token, frozen(PRICE))
            .await
            .expect("登记尝试");
        let started =
            service.mark_write_started(attempt, token, Some(binding())).await.expect("提交起写");
        assert!(matches!(started, WriteStart::FirstCommitted { .. }), "首派必须得到唯一许可");
        Started { attempt, attempt_receipt, claim_token: token }
    }
}
