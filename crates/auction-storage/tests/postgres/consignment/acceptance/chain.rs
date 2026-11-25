//! 两世界端到端验收的贯通夹具（PIAC E19、PIWP）：代管上架之后的每一步都走真实写入口——购买、派发 executor
//! 的各轮（计数假传输）、付款证据、COD 实收认证、买家确领原件（`prove_handoff`，同事务放行卖款）、下游首轮
//! 生产者（isekai 原件交割、卖款、退款）、退款转换、受益人现金确领证据。
//!
//! 只有代管上架以 SQL 造前置（与全部第 6 期夹具相同，原始 HTML → 决定的两世界贯通见
//! `crates/auction-integrations/tests/consignment_envelope.rs` 的 G3 用例）。确领与现金确领证据用 `test-db` 构建
//! 才认的已签收验证器版本（生产两类验证器都未签收）。传输是计数替身，不连接任何外部服务；夹具值都是明显的假值。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use auction_application::consignment::{
    ConsignmentError, ConsignmentFuture, DispatchService, HandoffReceipt, PostPermit,
    ProveHandoffInput, PurchaseCommand, PurchaseStore, RecordEvidenceInput,
};
use auction_application::core::RequestHash;
use auction_application::settlement::{
    CashLeg, CashPhase, CertifyCod, CertifyReceipt, CodLeg, ConsignmentFundingPreflight,
    ConsignmentFundingStore, ConvertRefund, DispatchFreeze, FundingError, FundingFuture,
    FundingPreflightFact, ReceiptKey, RefundReceipt, SettleCash,
};
use auction_domain::consignment::dispatch::{
    AttemptKey, DispatchRoundKey, EvidenceCategory, RoundOrigin,
};
use auction_domain::consignment::funding::FundingMode;
use auction_domain::consignment::purchase::PurchaseInput;
use auction_domain::consignment::{
    ConsignmentSalePolicy, ListingPrice, ListingQuote, ListingVersion,
};
use auction_domain::{CommandId, SettlementWorld, UserId};
use auction_jobs::consignment::{
    ConsignmentDispatchRuntime, DispatchExecution, DispatchRound, DispatchTransport, DispatchWork,
    PreparedDispatch, SentObservation,
};
use auction_storage::{DatabaseStore, DownstreamRoundRepository, PurchaseRepository};
use auction_time::Timestamp;
use sqlx::{AssertSqlSafe, PgPool};
use tokio::sync::Notify;
use uuid::Uuid;

use super::super::contracts::fixture::{WINDOW, seed_main_custody};
use super::super::deposit::support::{
    CREDENTIAL, LISTING, USER_2, accepted_envelope, attempt_start_proof_and_custody, succeeds,
};
use super::super::dispatch::carrier_funding::{BALANCE, UID, cash_evidence};
use super::super::dispatch::fixture::{CONTENT, binding, prepare_send_credential, work_store};
use super::super::dispatch::worker_chain::Chain;
use super::super::funding::cod_fixture::{cash, mutation};
use super::super::ledger136::fixture::{PRICE, available_sql};
use super::super::returns::fixture::SIGNED_VALIDATOR;
use crate::support::TestDatabase;

/// 严格现金账户开户边界来源（最近一次现金 opening）。
const BOUNDARY_SOURCE: &str = "c6a00000-0000-4000-8000-000000000001";
/// 确领类证据的观察时刻：固定值，使同一证据的重放摘要不变。
const CLAIM_OBSERVED_AT: &str = "2026-01-03T00:00:00Z";
/// 原 UID 的自有余额（isekai 载体本金从这里借出 1）。
pub(in crate::consignment) const OWN: i64 = 10;

/// 两个结算世界。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(in crate::consignment) enum World {
    Main,
    Isekai,
}

impl World {
    /// 付款义务种类与 COD 收款腿。
    pub(in crate::consignment) const fn payment(self) -> (&'static str, CodLeg) {
        match self {
            Self::Main => ("winner_equipment_cod", CodLeg::Original),
            Self::Isekai => ("winner_payment_carrier_cod", CodLeg::Carrier),
        }
    }

    const fn settlement(self) -> SettlementWorld {
        match self {
            Self::Main => SettlementWorld::Main,
            Self::Isekai => SettlementWorld::Isekai,
        }
    }

    /// 付款腿的买家确领证据类别：main 买家确领原件，isekai 载体付款。
    pub(in crate::consignment) const fn payment_evidence(self) -> EvidenceCategory {
        match self {
            Self::Main => EvidenceCategory::OriginalClaim,
            Self::Isekai => EvidenceCategory::CarrierPayment,
        }
    }
}

/// 资金预检替身：严格现金、UID 7001、开户边界早于原 Send。
struct ReadyPreflight;

impl ConsignmentFundingPreflight for ReadyPreflight {
    fn preflight(&self, _world: SettlementWorld) -> FundingFuture<'_, FundingPreflightFact> {
        let ready = FundingPreflightFact::Ready {
            uid: UID.to_owned(),
            mode: FundingMode::StrictCash,
            boundary_at: Timestamp::parse_from_rfc3339("2026-01-02T00:00:00Z").expect("夹具边界"),
            boundary_source_id: Some(Uuid::parse_str(BOUNDARY_SOURCE).expect("夹具来源")),
            account_version: 0,
        };
        Box::pin(async move { Ok(ready) })
    }
}

/// 计数替身：冻结了 Credits 的轮在准备时给出同会话资金证据（新鲜、足额、冻结 UID），每次执行记一次 POST
/// 并给出 Sent 证明。
pub(in crate::consignment) struct Remote {
    pool: PgPool,
    pub(in crate::consignment) posts: AtomicUsize,
}

impl DispatchTransport for Remote {
    fn prepare<'a>(
        &'a self,
        work: &'a DispatchWork,
        attempt: AttemptKey,
        claim_token: Uuid,
    ) -> ConsignmentFuture<'a, Option<PreparedDispatch>> {
        Box::pin(async move {
            let cash_evidence = if work.frozen.credits > 0 {
                let now = Timestamp::now();
                Some(
                    cash_evidence(
                        &self.pool,
                        attempt,
                        claim_token,
                        &binding(),
                        UID,
                        Some(BALANCE),
                        now,
                    )
                    .await,
                )
            } else {
                None
            };
            Ok(Some(PreparedDispatch {
                credential: binding(),
                cash_evidence,
                handle: Box::new(()),
            }))
        })
    }

    fn execute(
        &self,
        _prepared: PreparedDispatch,
        _permit: PostPermit,
    ) -> ConsignmentFuture<'_, DispatchExecution> {
        let post = self.posts.fetch_add(1, Ordering::SeqCst) + 1;
        Box::pin(async move {
            Ok(DispatchExecution::Proven(SentObservation {
                sent_mid: format!("920000{post}"),
                observed_at: Timestamp::now(),
                validator_version: "fake-validator".to_owned(),
                evidence_digest: [7; 32],
            }))
        })
    }
}

/// 一张贯通中的订单：克隆库、两种账号、派发服务与计数传输。
pub(in crate::consignment) struct Order {
    _database: TestDatabase,
    pub(in crate::consignment) world: World,
    pub(in crate::consignment) owner: DatabaseStore,
    pub(in crate::consignment) runtime: DatabaseStore,
    pub(in crate::consignment) service: Arc<DispatchService>,
    pub(in crate::consignment) remote: Arc<Remote>,
}

/// isekai 代管上架：季窗仍在开放期（准入截止在 7 天后、季末在 10 天后），其余同 `seed_isekai_custody`。
async fn seed_isekai_open_custody(pool: &PgPool) {
    succeeds(
        pool,
        &format!(
            "INSERT INTO public.consignment_season_windows
                (id, season_id, world_id, timezone, start_utc, end_utc, admission_cutoff_utc,
                 calendar_version, preclose_days)
             VALUES ('{WINDOW}', 'fake-season', 'isekai', 'UTC',
                     transaction_timestamp() - interval '20 days',
                     transaction_timestamp() + interval '10 days',
                     transaction_timestamp() + interval '7 days',
                     decode(repeat('79', 32), 'hex'), 3);"
        ),
    )
    .await;
    let accepted =
        accepted_envelope().replace("'main'", "'isekai'").replace("|main|", "|isekai|").replace(
            " 'isekai', '42',\n                 NULL, 'awaiting_take'",
            &format!(" 'isekai', '42',\n                 '{WINDOW}', 'awaiting_take'"),
        );
    assert!(accepted.contains(WINDOW), "isekai 夹具必须绑定季窗");
    succeeds(pool, &accepted).await;
    succeeds(pool, &attempt_start_proof_and_custody()).await;
}

impl Order {
    /// 代管中的件已上架（available 版本 5）、寄售闸门打开、资金账户与开户边界就绪、发信凭据就绪。
    pub(in crate::consignment) async fn available(world: World) -> Self {
        let database = TestDatabase::create().await.expect("克隆模板库");
        let owner = database.maintenance_store().await.expect("以维护账号连接");
        let runtime = database.store().await.expect("以运行账号连接");
        let pool = owner.postgres_pool();
        match world {
            World::Main => seed_main_custody(pool).await,
            World::Isekai => seed_isekai_open_custody(pool).await,
        }
        succeeds(pool, &available_sql()).await;
        succeeds(
            pool,
            &format!(
                "UPDATE public.consignment_lookup_control
                 SET admitting = true, enabled_epoch = 1,
                     worker_ready_until = clock_timestamp() + interval '1 hour' WHERE id = 1;
                 INSERT INTO public.settlement_funding_accounts (uid, status) VALUES ('{UID}', 'ready');
                 INSERT INTO public.settlement_funding_sources
                    (id, uid, kind, source_key, amount, proof_digest)
                 VALUES ('{BOUNDARY_SOURCE}', '{UID}', 'reconciliation',
                         'c6a00000-0000-4000-8000-0000000000f1', 0, decode(repeat('a1', 32), 'hex'));
                 INSERT INTO public.settlement_funding_allocations (id, uid, purpose, amount)
                 VALUES (gen_random_uuid(), '{UID}', 'proprietary', {OWN});"
            ),
        )
        .await;
        prepare_send_credential(pool).await;
        let service = Arc::new(DispatchService::new(Arc::new(runtime.clone())));
        let remote = Arc::new(Remote { pool: pool.clone(), posts: AtomicUsize::new(0) });
        Self { _database: database, world, owner, runtime, service, remote }
    }

    pub(in crate::consignment) fn pool(&self) -> &PgPool {
        self.owner.postgres_pool()
    }

    /// 买家 USER_2 以版本 5、标价 P 的玩家命令经生产购买仓库成交。
    pub(in crate::consignment) async fn purchase(&self) -> Result<(), ConsignmentError> {
        let repository = PurchaseRepository::new(
            self.runtime.clone(),
            Arc::new(ReadyPreflight),
            Arc::new(CONTENT),
            ConsignmentSalePolicy::new(7, 3, 5).expect("策略"),
        );
        let command = PurchaseCommand {
            command_id: CommandId::from_uuid(Uuid::from_u128(
                0xe190_0000_0000_4000_8000_0000_0000_0001,
            )),
            request_hash: RequestHash::new([0xe1; 32]),
            buyer_user_id: UserId::from_uuid(Uuid::parse_str(USER_2).expect("夹具买家")),
            input: PurchaseInput {
                listing_id: Uuid::parse_str(LISTING).expect("夹具挂单"),
                expected_version: ListingVersion::new(5).expect("版本"),
                expected_price: ListingPrice::new(PRICE).expect("价格"),
            },
            request_id: None,
        };
        repository.purchase(command).await.map(drop)
    }

    /// 停用寄售（软关闭）：闸门关闭、启用代数前进，与 `deposit/disposition.rs` 的停用续办例同一写法。
    pub(in crate::consignment) async fn soft_close(&self) {
        succeeds(
            self.pool(),
            "UPDATE public.consignment_lookup_control SET admitting = false, enabled_epoch = 2
             WHERE id = 1;",
        )
        .await;
    }

    /// 生产派发 executor 跑一轮（生产领取 → 轮衔接 → 尝试 → 起写 → 一次 POST → Sent 证据）。
    pub(in crate::consignment) async fn run_executor(&self) -> DispatchRound {
        let executor = ConsignmentDispatchRuntime {
            work: Arc::new(Chain {
                store: self.runtime.clone(),
                work: work_store(&self.runtime, CONTENT),
            }),
            service: self.service.clone(),
            transport: self.remote.clone(),
            worker_id: "fake-worker".to_owned(),
            lease_duration: Duration::from_secs(300),
            wakeup: Arc::new(Notify::new()),
            paused: Arc::new(|| false),
        };
        executor.run_once().await.expect("executor 一轮")
    }

    pub(in crate::consignment) fn posts(&self) -> usize {
        self.remote.posts.load(Ordering::SeqCst)
    }

    pub(in crate::consignment) async fn scalar<T>(&self, sql: String) -> T
    where
        T: Send + Unpin + for<'r> sqlx::Decode<'r, sqlx::Postgres> + sqlx::Type<sqlx::Postgres>,
    {
        sqlx::query_scalar(AssertSqlSafe(sql)).fetch_one(self.pool()).await.expect("读取标量")
    }

    pub(in crate::consignment) async fn contract_id(&self) -> Uuid {
        self.scalar("SELECT contract_id FROM public.consignment_sales".to_owned()).await
    }

    /// 本合同某种义务的 ID。
    pub(in crate::consignment) async fn obligation(&self, kind: &str) -> Uuid {
        self.scalar(format!(
            "SELECT id FROM public.settlement_obligations
             WHERE contract_id = (SELECT contract_id FROM public.consignment_sales)
               AND obligation_kind = '{kind}'"
        ))
        .await
    }

    /// 某义务第 1 轮的投递与该轮唯一一次尝试的 fence。
    pub(in crate::consignment) async fn first_round(&self, obligation: Uuid) -> AttemptKey {
        let (delivery_id, fence): (Uuid, i64) = sqlx::query_as(
            "SELECT round.delivery_id, attempt.fence
             FROM public.consignment_dispatch_rounds AS round
             JOIN public.consignment_dispatch_attempts AS attempt
               ON attempt.delivery_id = round.delivery_id
             WHERE round.obligation_id = $1 AND round.round = 1",
        )
        .bind(obligation)
        .fetch_one(self.pool())
        .await
        .expect("首轮尝试");
        AttemptKey { delivery_id, fence: u64::try_from(fence).expect("fence") }
    }

    /// 给付款腿 Sent 尝试记一条远端付款明细（main 买家确领原件、isekai 载体付款）。
    pub(in crate::consignment) async fn record_payment(&self, attempt: AttemptKey) {
        self.service
            .record_evidence(RecordEvidenceInput {
                attempt,
                category: self.world.payment_evidence(),
                remote_observation_id: "fake-cod-paid-1".to_owned(),
                observed_at: Timestamp::now(),
                validator_version: SIGNED_VALIDATOR.to_owned(),
                evidence_digest: [0xe1; 32],
            })
            .await
            .expect("付款证据");
    }

    /// 由持久的冻结事实组装 COD 认证命令，并经生产资金端口提交。
    pub(in crate::consignment) async fn certify(
        &self,
        attempt: AttemptKey,
    ) -> Result<CertifyReceipt, FundingError> {
        let (payment_kind, leg) = self.world.payment();
        let payment = self.obligation(payment_kind).await;
        type Frozen = (Vec<u8>, Vec<u8>, Vec<u8>, String, i64, i64, i64, Vec<String>);
        let (prepare, activation, body, mid, fee, seller_amount, gross, equipment): Frozen =
            sqlx::query_as(
                "SELECT delivery.request_digest, delivery.activation_request_digest, round.body_digest,
                        binding.external_mail_id, sale.fee, sale.seller_amount, round.cod::bigint,
                        ARRAY(SELECT jsonb_array_elements_text(send.expectation->'equipment_ids'))
                 FROM public.consignment_dispatch_rounds AS round
                 JOIN public.deliveries AS delivery ON delivery.id = round.delivery_id
                 JOIN public.delivery_sent_bindings AS binding ON binding.delivery_id = round.delivery_id
                 JOIN public.delivery_send_attempts AS send
                   ON send.delivery_id = round.delivery_id AND send.fencing_version = binding.fencing_version
                 JOIN public.consignment_sales AS sale ON sale.contract_id = round.contract_id
                 WHERE round.delivery_id = $1",
            )
            .bind(attempt.delivery_id)
            .fetch_one(self.pool())
            .await
            .expect("原派发的冻结事实");
        let digest = |bytes: Vec<u8>| <[u8; 32]>::try_from(bytes.as_slice()).expect("32 字节摘要");
        let command = CertifyCod {
            mutation: mutation(0x71),
            contract_id: self.contract_id().await,
            payment_obligation_id: payment,
            original_delivery_id: attempt.delivery_id,
            receipt: ReceiptKey { uid: UID.to_owned(), payment_mail_id: mid, leg },
            world: self.world.settlement(),
            mode: FundingMode::StrictCash,
            price: ListingPrice::new(PRICE).expect("价格"),
            quote: ListingQuote { fee, seller_amount },
            gross,
            freeze: DispatchFreeze {
                prepare_digest: digest(prepare),
                activation_digest: digest(activation),
                body_digest: digest(body),
                equipment_ids: equipment,
            },
            cash: Some(cash(BALANCE)),
            attribution_id: None,
        };
        self.runtime.certify_cod(command).await
    }

    /// 买家确领原件的证据：main 挂在原件 COD 首轮、isekai 挂在原件交割首轮的唯一尝试上；验证器版本由调用方给出
    /// （正例的确领证据由推进循环记录，这里供未签收版本的反例用）。
    pub(in crate::consignment) async fn record_original_claim_by(
        &self,
        attempt: AttemptKey,
        observation: &str,
        validator: &str,
    ) {
        self.service
            .record_evidence(RecordEvidenceInput {
                attempt,
                category: EvidenceCategory::OriginalClaim,
                remote_observation_id: observation.to_owned(),
                observed_at: CLAIM_OBSERVED_AT.parse().expect("夹具时刻"),
                validator_version: validator.to_owned(),
                evidence_digest: [0xe2; 32],
            })
            .await
            .expect("确领证据");
    }

    /// 经生产写端证明买家已确领原件（同事务放行卖款）；`kind` 是原件腿义务种类。
    pub(in crate::consignment) async fn prove_handoff(
        &self,
        kind: &str,
        observation: &str,
        digest: [u8; 32],
    ) -> Result<HandoffReceipt, ConsignmentError> {
        let obligation = self.obligation(kind).await;
        let attempt = self.first_round(obligation).await;
        self.service
            .prove_handoff(ProveHandoffInput {
                key: DispatchRoundKey {
                    contract_id: self.contract_id().await,
                    obligation_id: obligation,
                    round: 1,
                },
                attempt,
                remote_observation_id: observation.to_owned(),
                evidence_digest: digest,
            })
            .await
    }

    /// 下游首轮生产者跑一轮（与 Worker 消费端同一写序：存储备好首轮投递 → 授权首轮）；返回新建的轮数。
    pub(in crate::consignment) async fn produce_rounds(&self) -> usize {
        let repository = DownstreamRoundRepository::new(self.runtime.clone(), Arc::new(CONTENT));
        let prepared = repository.prepare_pending(None, 64).await.expect("下游首轮待办");
        // 键集分页：恰有一项待办时，读满一页（页大小 1）给出游标，游标之后没有更多待办（正例）；
        // 读不满一页没有游标（反例）。
        if let [_] = prepared.items.as_slice() {
            assert_eq!(prepared.next, None, "读不满一页没有游标");
            let page = repository.prepare_pending(None, 1).await.expect("满页");
            let next = page.next.expect("读满一页应给出游标");
            let after = repository.prepare_pending(Some(next), 1).await.expect("游标之后");
            assert!((after.items.is_empty(), after.next.is_none()) == (true, true), "{after:?}");
        }
        let mut created = 0;
        for round in prepared.items {
            let receipt = self
                .service
                .authorize(round.key, RoundOrigin::First, round.delivery_id, round.frozen)
                .await
                .expect("授权下游首轮");
            created += usize::from(!receipt.replayed);
        }
        created
    }

    /// 经生产资金写端把 S 与 F 转为买家退款 P（同事务激活退款义务）。
    pub(in crate::consignment) async fn convert_refund(
        &self,
    ) -> Result<RefundReceipt, FundingError> {
        let (fee, seller_amount): (i64, i64) =
            sqlx::query_as("SELECT fee, seller_amount FROM public.consignment_sales")
                .fetch_one(self.pool())
                .await
                .expect("成交金额");
        self.runtime
            .convert_refund(ConvertRefund {
                mutation: mutation(0x73),
                contract_id: self.contract_id().await,
                refund_obligation_id: self.obligation("buyer_refund").await,
                price: ListingPrice::new(PRICE).expect("价格"),
                quote: ListingQuote { fee, seller_amount },
            })
            .await
    }

    /// 受益人领取 Credits 的证据（现金确领），`validator` 决定是否已签收；返回是否重放。
    pub(in crate::consignment) async fn cash_claim(
        &self,
        attempt: AttemptKey,
        observation: &str,
        validator: &str,
    ) -> Result<bool, ConsignmentError> {
        self.service
            .record_evidence(RecordEvidenceInput {
                attempt,
                category: EvidenceCategory::CashClaim,
                remote_observation_id: observation.to_owned(),
                observed_at: CLAIM_OBSERVED_AT.parse().expect("夹具时刻"),
                validator_version: validator.to_owned(),
                evidence_digest: [0xe3; 32],
            })
            .await
            .map(|receipt| receipt.replayed)
    }

    /// 直接经生产资金端口推进现金腿 claimed（只在已有确领证据时成立）；返回是否重放。
    pub(in crate::consignment) async fn settle_claimed(
        &self,
        kind: &str,
        leg: CashLeg,
        attempt: AttemptKey,
        amount: i64,
    ) -> Result<bool, FundingError> {
        self.runtime
            .settle_cash(SettleCash {
                mutation: mutation(0x72),
                contract_id: self.contract_id().await,
                obligation_id: self.obligation(kind).await,
                delivery_id: attempt.delivery_id,
                leg,
                phase: CashPhase::Claimed,
                amount,
            })
            .await
            .map(|receipt| receipt.replayed)
    }

    /// 切换唯一 active 发信凭据的世界命名空间：isekai 原件交割在 isekai 派发，其余轮在 main。
    pub(in crate::consignment) async fn send_from(&self, world: &str) {
        succeeds(
            self.pool(),
            &format!(
                "UPDATE public.credential_keys SET scope = 'mooglemail:{world}' WHERE id = '{CREDENTIAL}';"
            ),
        )
        .await;
    }

    /// 用途分配的 (状态, 金额)；不存在为 `None`。
    pub(in crate::consignment) async fn allocation(&self, purpose: &str) -> Option<(String, i64)> {
        sqlx::query_as(
            "SELECT state, amount FROM public.settlement_funding_allocations
             WHERE uid = $1 AND purpose = $2",
        )
        .bind(UID)
        .bind(purpose)
        .fetch_optional(self.pool())
        .await
        .expect("用途分配")
    }

    /// 某类资金分录的 (条数, 金额合计)。
    pub(in crate::consignment) async fn entries(&self, kind: &str) -> (i64, i64) {
        sqlx::query_as(
            "SELECT count(*), COALESCE(sum(amount), 0)::bigint FROM public.settlement_funding_entries
             WHERE uid = $1 AND kind = $2",
        )
        .bind(UID)
        .bind(kind)
        .fetch_one(self.pool())
        .await
        .expect("资金分录")
    }

    /// 订单阶段事件数（每个确有事实变化的写入同事务追加一条）。
    pub(in crate::consignment) async fn order_events(&self) -> i64 {
        self.scalar(
            "SELECT count(*) FROM public.realtime_events WHERE kind = 'consignment.order_stage_changed'"
                .to_owned(),
        )
        .await
    }

    /// position 当前状态。
    pub(in crate::consignment) async fn position_state(&self) -> String {
        self.scalar("SELECT state FROM public.consignment_custody_positions".to_owned()).await
    }

    /// 本合同某种义务的业务状态。
    pub(in crate::consignment) async fn business_state(&self, kind: &str) -> String {
        self.scalar(format!(
            "SELECT business_state FROM public.settlement_obligations
             WHERE contract_id = (SELECT contract_id FROM public.consignment_sales)
               AND obligation_kind = '{kind}'"
        ))
        .await
    }

    pub(in crate::consignment) async fn count(&self, table: &str) -> i64 {
        self.scalar(format!("SELECT count(*) FROM public.{table}")).await
    }
}
