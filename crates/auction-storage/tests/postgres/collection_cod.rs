//! COD 与载体付款在“等待领取”拆分后的完整路径及历史迁移回归，以运行账号写入：
//! - 当前行为场景的付款投递经投递端口按 Worker 的真实顺序推进（领取 → 发送围栏带冻结 Send 身份 →
//!   `succeed` 绑定唯一 Sent MID），数据库据此判定冻结发送证明成立。载荷、canonical 主题与成功
//!   metadata 与生产生成方及传输层逐字段一致；存在受保护资金账户时，载体付款寄出的 1 Credit
//!   与 Worker 一样只经现金门（同 session 证据）预留；
//! - 恢复记录在付款期内时计为等待买家付款（`waiting_payment = 1`），不进入等待领取、积压或
//!   长期未领取清单，即使寄出已远超长期未领取阈值；
//! - 未建立资金账户时，领取证明经旧满足入口满足并解除依赖；存在受保护资金账户时旧入口拒绝；
//! - 资金化正向路径：两类付款都直接调用真实 `SettlementInternalFulfillmentStore::satisfy_collected`，
//!   以合成凭据、匹配的运行代际、record_only 账户与早于发送的开户来源执行实收证明与资金入账
//!   校验；同一证明重放不重复入账；错 MID、失效代际与晚于发送的开户来源都整体回滚；
//! - 异世界内部交付核验实收或内部付款的同 UID 证明；仅有历史领取状态或不同 UID 均不能交付；
//! - 全部场景都是当前生产形状：场次快照冻结默认结算策略 v2（hold_curve_v2），义务版本、结算
//!   明细的 policy 标签与竞得装备 COD 载荷的 `pricing_mode` 按生产写入口径与之一致，资金准入
//!   走 v2 价格变量校验，而不是缺标签时的 v1 兼容读路径；
//! - 反例：付款投递已成功但没有冻结发送证明时不计为等待付款。
//!
//! 入口见 `mise run test-db`。

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::assert_matches;
use std::sync::Arc;
use std::time::{Duration as StdDuration, Instant};

use auction_application::delivery::{
    DeliveryClaimExclusions, DeliveryFenceOutcome, DeliveryPreparedExpectation,
    DeliverySendCredentialBinding, DeliverySendExpectation, DeliverySendResult, DeliveryWorkStore,
};
use auction_application::operations::{
    OperationsAlertCollectionOverdueRead, OperationsAlertFulfillmentSnapshot,
    OperationsAlertSnapshot, OperationsAlertSourceReader,
};
use auction_application::public::format_deadline;
use auction_application::settlement::{
    FundingError, INTERNAL_EVIDENCE_PROTOCOL, InternalEvidencePurpose, MailAccountEvidenceBinding,
    MailRuntimeBinding, SettlementFulfillmentClaim, SettlementFulfillmentProof,
    SettlementFulfillmentReconcileStatus, SettlementFulfillmentStore, SettlementInternalCommand,
    SettlementInternalCommitOutcome, SettlementInternalEvidenceSnapshot,
    SettlementInternalFulfillmentStore, VerifiedMailEquipmentPresence,
    VerifiedMailSentCollectionObservation, canonical_settlement_fulfillment_subject,
};
use auction_domain::{
    RecoveryPolicy, SettlementId, SettlementMode, SettlementObligationKind, SettlementWorld,
};
use auction_storage::{DatabaseStore, postgres_migrate_to};
use auction_time::{Duration, Timestamp};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::support;
use crate::support::fixtures::{policy_snapshot, seed_user, stable_id};

const SEED: &str = "collection-cod";
const WINNER: &str = "collection-cod-winner";
const SELLER: &str = "collection-cod-seller";
const EQUIPMENT_NAME: &str = "Collection cod equipment";
/// 场次快照冻结的当前默认结算策略（`SettlementPolicy::default()`）：version 2 / hold_curve_v2。
/// 夹具按生产结算的写入口径取用：各义务记冻结版本，结算明细的 `adjustment_metadata` 只记
/// `{"policy": 冻结模式}`，竞得装备 COD 载荷因此带 v2 的 `pricing_mode` 变量。
const SETTLEMENT_POLICY_VERSION: i32 = 2;
const SETTLEMENT_MODE: SettlementMode = SettlementMode::HoldCurveV2;
/// 同一快照冻结的恢复策略版本（`RecoveryPolicy::default()`）；恢复记录按生产口径记这个版本。
const RECOVERY_POLICY_VERSION: i32 = 1;
/// 冻结 v2 拆分：300,000 ppm 锚点以 4C 补偿覆盖折扣，买方付 996C，卖方仍收 1000C。
const END_PRICE: i64 = 1_000;
const HOLD_PPM: i64 = 300_000;
const ADJUSTMENT_PPM: i64 = -4_000;
const TAX_CREDITS: i64 = 0;
const COMPENSATION_CREDITS: i64 = 4;
const BUYER_CREDITS: i64 = END_PRICE - COMPENSATION_CREDITS;
const SELLER_CREDITS: i64 = END_PRICE;
const THRESHOLD: StdDuration = StdDuration::from_secs(259_200);
/// 合成邮件账号：资金账户 UID、凭据的预期玩家名与初始运行代际。
const FUNDING_UID: &str = "7002";
const BOT_NAME: &str = "Collection Cod Bot";
const MAIL_GENERATION: i64 = 7;

/// 付款义务的两种形态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Payment {
    /// main 场次：COD 邮件附装备，收件人付款即取得装备。
    EquipmentCod,
    /// isekai 场次：在 main 寄 1 Credit 载体 COD，付款后再在异世界交付装备。
    CarrierCod,
}

impl Payment {
    fn world(self) -> &'static str {
        match self {
            Self::EquipmentCod => "main",
            Self::CarrierCod => "isekai",
        }
    }

    fn obligation_kind(self) -> SettlementObligationKind {
        match self {
            Self::EquipmentCod => SettlementObligationKind::WinnerEquipmentCod,
            Self::CarrierCod => SettlementObligationKind::WinnerPaymentCarrierCod,
        }
    }

    fn carrier_credit(self) -> i64 {
        i64::from(self == Self::CarrierCod)
    }

    /// 付款邮件的 Sent MID；同一 world 内唯一。
    fn external_mail_id(self) -> &'static str {
        match self {
            Self::EquipmentCod => "7400401",
            Self::CarrierCod => "7400402",
        }
    }

    /// 生产发送与 Sent 证据共用的 canonical 主题。
    fn subject(self, payment: Uuid) -> String {
        canonical_settlement_fulfillment_subject(
            self.obligation_kind(),
            SettlementId::from_uuid(payment),
        )
        .expect("付款义务必须有 canonical 主题")
    }
}

/// 一个付款场景的标识。
struct CodCase {
    payment: Uuid,
    handoff: Option<Uuid>,
    payout: Uuid,
    lot: Uuid,
    delivery: Uuid,
    external_equipment_id: String,
}

/// 写入已结算场次、lot 与冻结结算明细、付款义务及其依赖（卖方货款受阻于付款或交付、买方退款
/// 为或有）、付款投递（尚在队列）、各履约行与付款期内的恢复记录。`payment_deadline` 是恢复记录
/// 当前的付款期限（`payment_deadline_at`）；付款邮件正文按生成方格式写首寄期限 D0，即恢复记录
/// `created_at` + 冻结恢复策略（`RECOVERY_POLICY_VERSION`）的付款期限，与资金化准入重建期望载荷
/// 的口径一致。
async fn seed_case(pool: &PgPool, payment: Payment, payment_deadline: Timestamp) -> CodCase {
    seed_case_with_delivery(pool, payment, payment_deadline, true, WINNER).await
}

async fn seed_internal_case(
    pool: &PgPool,
    payment: Payment,
    payment_deadline: Timestamp,
) -> CodCase {
    seed_case_with_delivery(pool, payment, payment_deadline, false, BOT_NAME).await
}

async fn seed_case_with_delivery(
    pool: &PgPool,
    payment: Payment,
    payment_deadline: Timestamp,
    create_delivery: bool,
    recipient_name: &str,
) -> CodCase {
    let world = payment.world();
    let id = |suffix: &str| stable_id(SEED, &format!("{world}-{suffix}"));
    let winner = id("winner");
    seed_user(pool, winner, recipient_name).await;
    let ended = Timestamp::now() - Duration::days(1);
    let auction = id("auction");
    let command = id("command");
    let policy = policy_snapshot(ended, END_PRICE, 100);
    // 下面的义务版本、明细标签与载荷变量都按冻结快照写；v2 的 300,000 ppm 锚点以 4C 补偿覆盖折扣。
    // 版本、模式或对应锚点漂移时失败关闭，不退回历史 v1 兼容读路径。
    let frozen = &policy["settlement"];
    assert_eq!(
        (&frozen["version"], &frozen["mode"], &frozen["anchors"][6]),
        (
            &json!(SETTLEMENT_POLICY_VERSION),
            &json!(SETTLEMENT_MODE.as_str()),
            &json!({"hold_ppm": HOLD_PPM, "adjustment_ppm": ADJUSTMENT_PPM}),
        ),
        "场次快照冻结当前默认结算策略 v2，300,000 ppm 锚点为 -4,000 ppm"
    );
    assert_eq!(
        policy["recovery"]["version"],
        json!(RECOVERY_POLICY_VERSION),
        "场次快照冻结当前默认恢复策略 v1"
    );
    sqlx::query(
        "INSERT INTO public.auctions
            (id, world_id, sequence, title, status, scheduled_open_at, preview_scheduled_at,
             scheduled_end_at, effective_end_at, preview_opened_at, opened_at, policy_snapshot)
         VALUES ($1, $2, 7, 'collection cod', 'settled', $3, $3, $3, $3, $3, $3, $4)",
    )
    .bind(auction)
    .bind(world)
    .bind(ended)
    .bind(&policy)
    .execute(pool)
    .await
    .expect("写入已结算场次");
    sqlx::query(
        "INSERT INTO public.auction_commands
            (command_id, command_kind, aggregate_id, request_hash, result_code)
         VALUES ($1, 'collection-cod-settlement', $2, $3, 'accepted')",
    )
    .bind(command)
    .bind(auction)
    .bind([6_u8; 32].as_slice())
    .execute(pool)
    .await
    .expect("写入结算命令");
    let equipment = id("equipment");
    let external_equipment_id =
        if payment == Payment::EquipmentCod { "9400401" } else { "9400402" }.to_owned();
    sqlx::query(
        "INSERT INTO public.equipment
            (id, world_id, external_equipment_id, name, category, attributes, collected_at)
         VALUES ($1, $2, $3, $4, 'weapon', '{}'::jsonb, $5)",
    )
    .bind(equipment)
    .bind(world)
    .bind(&external_equipment_id)
    .bind(EQUIPMENT_NAME)
    .bind(ended - Duration::days(2))
    .execute(pool)
    .await
    .expect("写入测试装备");
    let lot = id("lot");
    sqlx::query(
        "INSERT INTO public.lots
            (id, auction_id, equipment_id, lot_number, status, start_price, current_price,
             effective_end_at, category_key, category_number)
         VALUES ($1, $2, $3, 1, 'sold', $4, $4, $5, 'weapon', 1)",
    )
    .bind(lot)
    .bind(auction)
    .bind(equipment)
    .bind(END_PRICE)
    .bind(ended)
    .execute(pool)
    .await
    .expect("写入测试 lot");
    // 与生产结算写入（`settlement/facts.rs`）一致：metadata 只含冻结模式的 policy 标签。
    sqlx::query(
        "INSERT INTO public.lot_settlements
            (lot_id, end_price, listed_price, buyer_amount, seller_amount, tax_amount,
             compensation_amount, hold_ppm, adjustment_ppm, adjustment_metadata)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(lot)
    .bind(END_PRICE)
    .bind(BUYER_CREDITS)
    .bind(BUYER_CREDITS)
    .bind(SELLER_CREDITS)
    .bind(TAX_CREDITS)
    .bind(COMPENSATION_CREDITS)
    .bind(HOLD_PPM)
    .bind(ADJUSTMENT_PPM)
    .bind(json!({ "policy": SETTLEMENT_MODE.as_str() }))
    .execute(pool)
    .await
    .expect("写入冻结结算明细");

    // 与生产结算写入一致：同一 lot 的全部义务都记场次冻结的结算策略版本。
    let obligation =
        |kind: SettlementObligationKind, state: &str, buyer: bool, amount: i64, prerequisite| {
            let obligation_id = id(kind.as_str());
            let state = state.to_owned();
            async move {
                sqlx::query(
                    "INSERT INTO public.settlement_obligations
                        (id, auction_id, lot_id, world_id, origin_command_id, policy_version,
                         obligation_kind, business_state, recipient_user_id,
                         recipient_game_identity, amount, carrier_credit, equipment_id,
                         prerequisite_obligation_id)
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)",
                )
                .bind(obligation_id)
                .bind(auction)
                .bind(lot)
                .bind(world)
                .bind(command)
                .bind(SETTLEMENT_POLICY_VERSION)
                .bind(kind.as_str())
                .bind(&state)
                .bind(buyer.then_some(winner))
                .bind((!buyer).then_some(SELLER))
                .bind(amount)
                .bind(i64::from(kind == SettlementObligationKind::WinnerPaymentCarrierCod))
                .bind(equipment)
                .bind(prerequisite)
                .execute(pool)
                .await
                .expect("写入测试义务");
                obligation_id
            }
        };
    let payment_id =
        obligation(payment.obligation_kind(), "ready", true, BUYER_CREDITS, None).await;
    let handoff = match payment {
        Payment::EquipmentCod => None,
        Payment::CarrierCod => Some(
            obligation(
                SettlementObligationKind::WinnerEquipmentHandoff,
                "blocked",
                true,
                0,
                Some(payment_id),
            )
            .await,
        ),
    };
    let payout = obligation(
        SettlementObligationKind::SellerPayout,
        "blocked",
        false,
        SELLER_CREDITS,
        Some(handoff.unwrap_or(payment_id)),
    )
    .await;
    let refund = obligation(
        SettlementObligationKind::BuyerRefund,
        "contingent",
        true,
        BUYER_CREDITS,
        Some(payment_id),
    )
    .await;

    // 付款投递：与结算生成方当前写出的形状逐字段一致——正文带付款期限，竞得装备 COD 另带由
    // 冻结结算明细重建的价格变量，冻结模式为 v2 时含 `pricing_mode`（`fulfillment/intent.rs`）；
    // 激活后排队等待投递 Worker 领取。
    let delivery = id("delivery");
    let cod = BUYER_CREDITS + payment.carrier_credit();
    // 恢复记录显式写入建立时刻，正文期限取由它与冻结恢复策略版本推出的 D0（生产同一函数）；
    // 当前期限由各用例给出，只决定轮询与等待付款口径，不参与正文与载荷重建。
    let recovery_created_at = Timestamp::now();
    let deadline = format_deadline(
        RecoveryPolicy::initial_payment_deadline(
            recovery_created_at,
            u32::try_from(RECOVERY_POLICY_VERSION).expect("恢复策略版本为正"),
        )
        .expect("冻结恢复策略版本的首寄期限"),
    );
    let payload = match payment {
        Payment::EquipmentCod => json!({
            "schema_version": 1,
            "operation": "send",
            "world": "main",
            "template": "settlement_winner_equipment_cod",
            "variables": {
                "reference": payment_id.to_string(),
                "amount": cod.to_string(),
                "item_name": EQUIPMENT_NAME,
                "payment_deadline": deadline,
                "hold_ppm": HOLD_PPM.to_string(),
                "adjustment_ppm": ADJUSTMENT_PPM.to_string(),
                "winning_bid": END_PRICE.to_string(),
                "pricing_mode": "hold_curve_v2",
                "adjusted_price": END_PRICE.to_string(),
                "pool_credit": COMPENSATION_CREDITS.to_string()
            },
            "credits": 0,
            "cod": cod,
            "equipment_secret_refs": [{"kind": "auction_equipment_key", "id": equipment}]
        }),
        Payment::CarrierCod => json!({
            "schema_version": 1,
            "operation": "send",
            "world": "main",
            "template": "settlement_payment_carrier",
            "variables": {
                "reference": payment_id.to_string(),
                "amount": cod.to_string(),
                "item_name": EQUIPMENT_NAME,
                "payment_deadline": deadline
            },
            "credits": 1,
            "cod": cod,
            "equipment_secret_refs": []
        }),
    };
    if create_delivery {
        sqlx::query(
            "INSERT INTO public.deliveries
                (id, command_id, request_digest, origin_kind, origin_id, recipient, payload, status,
                 activation_command_id, activation_request_digest, activated_at)
             VALUES ($1, $2, decode(repeat('a1', 32), 'hex'), 'settlement', $3, $4, $5, 'pending',
                     $6, decode(repeat('a2', 32), 'hex'), now())",
        )
        .bind(delivery)
        .bind(id("prepare"))
        .bind(payment_id.to_string())
        .bind(recipient_name)
        .bind(payload)
        .bind(id("activate"))
        .execute(pool)
        .await
        .expect("写入付款投递");
    }

    let fulfillment = |obligation_id: Uuid, state: &str, dispatch: &str, delivery: Option<Uuid>| {
        let state = state.to_owned();
        let dispatch = dispatch.to_owned();
        async move {
            sqlx::query(
                "INSERT INTO public.settlement_obligation_fulfillments
                    (obligation_id, execution_state, dispatch_world, delivery_operation,
                     delivery_id, prepare_command_id, prepare_request_digest,
                     activate_command_id, activate_request_digest)
                 VALUES ($1, $2, $3, 'send', $4, $5,
                         CASE WHEN $4::uuid IS NULL THEN NULL
                              ELSE decode(repeat('a1', 32), 'hex') END,
                         $6,
                         CASE WHEN $4::uuid IS NULL THEN NULL
                              ELSE decode(repeat('a2', 32), 'hex') END)",
            )
            .bind(obligation_id)
            .bind(&state)
            .bind(&dispatch)
            .bind(delivery)
            .bind(delivery.map(|_| id("prepare")))
            .bind(delivery.map(|_| id("activate")))
            .execute(pool)
            .await
            .expect("写入测试履约");
        }
    };
    fulfillment(
        payment_id,
        if create_delivery { "enqueued" } else { "pending" },
        "main",
        create_delivery.then_some(delivery),
    )
    .await;
    if let Some(handoff) = handoff {
        fulfillment(handoff, "pending", "isekai", None).await;
    }
    fulfillment(payout, "pending", "main", None).await;
    fulfillment(refund, "pending", "main", None).await;

    sqlx::query(
        "INSERT INTO public.settlement_recoveries
            (id, auction_id, lot_id, world_id, equipment_id, winner_user_id,
             payment_obligation_id, equipment_obligation_id, seller_payout_obligation_id,
             buyer_refund_obligation_id, policy_version, payment_deadline_at, stage,
             created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, 'awaiting_payment', $13, $13)",
    )
    .bind(id("recovery"))
    .bind(auction)
    .bind(lot)
    .bind(world)
    .bind(equipment)
    .bind(winner)
    .bind(payment_id)
    .bind(handoff)
    .bind(payout)
    .bind(refund)
    .bind(RECOVERY_POLICY_VERSION)
    .bind(payment_deadline)
    .bind(recovery_created_at)
    .execute(pool)
    .await
    .expect("写入付款期内的恢复记录");
    CodCase { payment: payment_id, handoff, payout, lot, delivery, external_equipment_id }
}

/// 合成的同 session 邮件账号：active mail_cookie 凭据（main 范围、带预期玩家名），认证 UID 即
/// record_only 资金账户。
struct Funding {
    credential: Uuid,
}

/// 同 session 观察到的已全部领取的 Sent 邮件。
struct SentObservation<'a> {
    external_mail_id: &'a str,
    recipient: &'a str,
    subject: String,
    sent_at: Timestamp,
}

impl Funding {
    fn credential_binding(&self, generation: i64) -> DeliverySendCredentialBinding {
        DeliverySendCredentialBinding {
            credential_id: self.credential,
            credential_version: 1,
            runtime_generation: generation,
        }
    }

    /// 生产中只由持有会话的可信适配器生成的证据快照，这里以合成事实构造：同一凭据版本、给定
    /// 运行代际与资金账户 UID；record_only 不观察现金。各级摘要都用生产 `compute_digest` 计算。
    fn evidence(
        &self,
        generation: i64,
        operation_id: Uuid,
        purpose: InternalEvidencePurpose,
        sent: Option<SentObservation<'_>>,
    ) -> SettlementInternalEvidenceSnapshot {
        let observed =
            Timestamp::now().duration_trunc(Duration::microseconds(1)).expect("截断到微秒");
        let mut identity = MailAccountEvidenceBinding {
            credential_id: self.credential,
            credential_version: 1,
            runtime: MailRuntimeBinding { generation },
            authenticated_uid: FUNDING_UID.to_owned(),
            player_name: BOT_NAME.to_owned(),
            world: SettlementWorld::Main,
            observed_at: observed,
            protocol_version: INTERNAL_EVIDENCE_PROTOCOL,
            digest: [0; 32],
        };
        identity.digest = identity.compute_digest();
        let sent_collection = sent.map(|sent| {
            let mut observation = VerifiedMailSentCollectionObservation {
                binding: identity.clone(),
                external_mail_id: sent.external_mail_id.to_owned(),
                recipient: sent.recipient.to_owned(),
                subject: sent.subject,
                sent_at: sent.sent_at,
                observed_at: observed,
                remaining_credits: 0,
                cod_pending: 0,
                remaining_equipment_ids: Vec::new(),
                collected: true,
                digest: [0; 32],
            };
            observation.digest = observation.compute_digest();
            observation
        });
        let mut evidence = SettlementInternalEvidenceSnapshot {
            operation_id,
            purpose,
            identity,
            equipment: None,
            main_cash: None,
            sent_collection,
            observed_from: observed,
            expires_at: observed + Duration::seconds(60),
            digest: [0; 32],
        };
        evidence.digest = evidence.compute_digest();
        evidence
    }

    fn equipment_evidence(
        &self,
        command: &SettlementInternalCommand,
        uid: &str,
    ) -> SettlementInternalEvidenceSnapshot {
        let claim = &command.claim;
        let mut evidence = self.evidence(
            MAIL_GENERATION,
            command.command_id,
            InternalEvidencePurpose::SettlementClaim {
                obligation_id: claim.obligation_id.as_uuid(),
                claim_token: claim.claim_token,
                claim_fencing: claim.claim_fencing,
                state_version: claim.state_version,
            },
            None,
        );
        evidence.identity.authenticated_uid = uid.to_owned();
        evidence.identity.world = claim.world;
        evidence.identity.digest = evidence.identity.compute_digest();
        let mut equipment = VerifiedMailEquipmentPresence {
            binding: evidence.identity.clone(),
            equipment_id: claim.equipment_id,
            external_equipment_id: claim.external_equipment_id.clone(),
            category: "weapon_1handed".to_owned(),
            observed_at: evidence.observed_from,
            digest: [0; 32],
        };
        equipment.binding.world = claim.equipment_world;
        equipment.binding.digest = equipment.binding.compute_digest();
        equipment.digest = equipment.compute_digest();
        evidence.equipment = Some(equipment);
        evidence.digest = evidence.compute_digest();
        evidence
    }

    /// 按真实 claim 绑定的实收证据：purpose 取 claim 的令牌、fencing 与状态版本，operation_id
    /// 即稳定实收命令 ID；Sent 观察给出 MID、收件人、canonical 主题与寄出时刻。
    fn collected_evidence(
        &self,
        generation: i64,
        command: &SettlementInternalCommand,
        payment: Payment,
        external_mail_id: &str,
    ) -> SettlementInternalEvidenceSnapshot {
        let claim = &command.claim;
        let sent_at = claim
            .delivery_snapshot
            .as_ref()
            .and_then(|delivery| delivery.completed_at)
            .expect("付款投递已寄出");
        self.evidence(
            generation,
            command.command_id,
            InternalEvidencePurpose::SettlementClaim {
                obligation_id: claim.obligation_id.as_uuid(),
                claim_token: claim.claim_token,
                claim_fencing: claim.claim_fencing,
                state_version: claim.state_version,
            },
            Some(SentObservation {
                external_mail_id,
                recipient: &claim.recipient,
                subject: payment.subject(claim.obligation_id.as_uuid()),
                sent_at,
            }),
        )
    }
}

/// 资金化前置：合成凭据、匹配的运行代际，以及与仅记账初始化写入形状相同的 record_only 账户和
/// 开户来源（开户时没有既有责任，金额为 0）。开户来源的时刻就是资金来源边界。
async fn seed_funding(pool: &PgPool) -> Funding {
    seed_funding_in_scope(pool, "mooglemail:main").await
}

async fn seed_funding_in_scope(pool: &PgPool, scope: &str) -> Funding {
    let credential = stable_id(SEED, "credential");
    sqlx::query(
        "INSERT INTO public.credential_keys
            (id, credential_kind, scope, label, ciphertext, nonce, encryption_key_id, fingerprint,
             status, state_version, player_name)
         VALUES ($1, 'mail_cookie', $2, 'collection-cod', $3, $4, 'test-key',
                 'collection-cod-credential', 'active', 1, $5)",
    )
    .bind(credential)
    .bind(scope)
    .bind(vec![1_u8])
    .bind(vec![0_u8; 12])
    .bind(BOT_NAME)
    .execute(pool)
    .await
    .expect("写入合成邮件凭据");
    sqlx::query(
        "INSERT INTO public.mail_runtime_generation (scope, generation, activated_at)
         VALUES ('mail_cookie', $1, now())",
    )
    .bind(MAIL_GENERATION)
    .execute(pool)
    .await
    .expect("写入邮件运行代际");
    sqlx::query(
        "INSERT INTO public.settlement_funding_accounts
            (uid, accounting_mode, status, state_version)
         VALUES ($1, 'record_only', 'ready', 1)",
    )
    .bind(FUNDING_UID)
    .execute(pool)
    .await
    .expect("写入 record_only 资金账户");
    sqlx::query(
        "INSERT INTO public.settlement_funding_sources
            (id, uid, kind, source_key, amount, proof_digest)
         VALUES ($1, $2, 'record_opening', $3, 0, $4)",
    )
    .bind(stable_id(SEED, "opening-source"))
    .bind(FUNDING_UID)
    .bind(stable_id(SEED, "opening-command"))
    .bind([0x0e_u8; 32].as_slice())
    .execute(pool)
    .await
    .expect("写入开户来源");
    Funding { credential }
}

async fn current_generation(pool: &PgPool) -> i64 {
    sqlx::query_scalar(
        "SELECT generation FROM public.mail_runtime_generation WHERE scope = 'mail_cookie'",
    )
    .fetch_one(pool)
    .await
    .expect("读取邮件运行代际")
}

/// 按 Worker 的真实顺序经投递端口寄出付款邮件：领取 → 以冻结 Send 身份进入 sending → 以绑定
/// 唯一 Sent MID 的成功证明收口。给出资金夹具时 Send 身份带凭据绑定，载体付款的 1 Credit 经
/// 现金门以同 session 证据预留；其余走无现金证据的围栏。成功 metadata 与传输层写出的形状一致。
async fn send_through_fence(
    store: &DatabaseStore,
    payment: Payment,
    case: &CodCase,
    funding: Option<&Funding>,
) {
    let now = Timestamp::now();
    let sending_grace = StdDuration::from_secs(60);
    DeliveryWorkStore::begin_claim_cycle(store, now, sending_grace).await.expect("领取前清理");
    let claims = DeliveryWorkStore::claim_due(
        store,
        "collection-cod-delivery".to_owned(),
        now,
        now + Duration::minutes(2),
        sending_grace,
        DeliveryClaimExclusions::default(),
        10,
    )
    .await
    .expect("领取投递");
    let claim =
        claims.into_iter().find(|claim| claim.delivery_id == case.delivery).expect("领取付款投递");
    let funded = match funding {
        Some(funding) => Some((funding, current_generation(store.postgres_pool()).await)),
        None => None,
    };
    let carrier = payment.carrier_credit();
    let expectation = Arc::new(DeliverySendExpectation {
        world_id: "main".to_owned(),
        recipient: claim.recipient.clone(),
        subject: payment.subject(case.payment),
        body_digest: [0x5a; 32],
        credits: carrier,
        cod: BUYER_CREDITS + carrier,
        equipment_ids: match payment {
            Payment::EquipmentCod => vec![case.external_equipment_id.clone()],
            Payment::CarrierCod => Vec::new(),
        },
        credential: funded.map(|(funding, generation)| funding.credential_binding(generation)),
    });
    let prepared = DeliveryPreparedExpectation::Send(Arc::clone(&expectation));
    let outcome = match (payment, funded) {
        (Payment::CarrierCod, Some((funding, generation))) => {
            let evidence = funding.evidence(
                generation,
                claim.command_id,
                InternalEvidencePurpose::DeliveryClaim {
                    delivery_id: claim.delivery_id,
                    claim_token: claim.lease_token,
                    claim_fencing: claim.fencing_version,
                    request_hash: claim.request_hash,
                },
                None,
            );
            store
                .start_sending_authorized_until(
                    claim.clone(),
                    Timestamp::now(),
                    &evidence,
                    Instant::now() + StdDuration::from_secs(30),
                    Some(&prepared),
                )
                .await
        }
        _ => {
            store
                .start_sending_authorized(claim.clone(), Timestamp::now(), None, Some(&prepared))
                .await
        }
    }
    .expect("发送围栏必须确定完成");
    assert_matches!(outcome, DeliveryFenceOutcome::Sending, "合法冻结身份必须进入 sending");
    store
        .succeed(
            claim.clone(),
            DeliverySendResult {
                external_reference: format!("mooglemail:main:send:{}", payment.external_mail_id()),
                result_metadata: json!({
                    "operation": "send",
                    "template": claim.payload["template"],
                    "external_reference_proven": true,
                    "sent_verified": true,
                    "proof": "sent_exact_payload_v1",
                    "settlement_obligation_id": case.payment.to_string(),
                    "send_expectation": serde_json::to_value(&*expectation).expect("冻结身份"),
                    "sending_fencing_version": claim.fencing_version,
                }),
            },
            Timestamp::now(),
        )
        .await
        .expect("合法 Sent 证明必须收口");
}

async fn frozen_sent_proof(pool: &PgPool, delivery: Uuid) -> bool {
    sqlx::query_scalar(
        "SELECT public.delivery_has_frozen_sent_proof(id, result_metadata, external_reference)
         FROM public.deliveries WHERE id = $1",
    )
    .bind(delivery)
    .fetch_one(pool)
    .await
    .expect("读取冻结发送证明判定")
}

/// 以 `at` 观察履约水位与长期未领取清单（带清单读取）。
async fn observe(store: &DatabaseStore, at: Timestamp) -> OperationsAlertSnapshot {
    store
        .snapshot(at, OperationsAlertCollectionOverdueRead::CountWithItems(THRESHOLD))
        .await
        .expect("读取告警快照")
}

/// 断言用的分组：等待付款、等待领取、待处理、受阻、等待前置、长期未领取超期件数。
fn grouping(fulfillment: &OperationsAlertFulfillmentSnapshot) -> [i64; 6] {
    [
        fulfillment.waiting_payment,
        fulfillment.awaiting_collection,
        fulfillment.pending,
        fulfillment.blocked,
        fulfillment.waiting_prerequisite,
        fulfillment.collection_overdue,
    ]
}

/// 领取付款义务的履约（买家付款后的 Sent 领取阶段）。
async fn claim_payment(store: &DatabaseStore, case: &CodCase) -> SettlementFulfillmentClaim {
    claim_obligation(store, case.payment).await
}

async fn claim_obligation(store: &DatabaseStore, obligation: Uuid) -> SettlementFulfillmentClaim {
    let now =
        Timestamp::now().duration_trunc(Duration::microseconds(1)).expect("领取时间精确到微秒");
    SettlementFulfillmentStore::claim_due(
        store,
        "collection-cod-fulfillment".to_owned(),
        now,
        now + Duration::minutes(10),
        10,
    )
    .await
    .expect("领取到期履约")
    .into_iter()
    .find(|claim| claim.obligation_id.as_uuid() == obligation)
    .expect("领取目标履约")
}

async fn internal_command(store: &DatabaseStore, obligation: Uuid) -> SettlementInternalCommand {
    let claim = claim_obligation(store, obligation).await;
    let recipient_version: i64 =
        sqlx::query_scalar("SELECT state_version FROM public.users WHERE id = $1")
            .bind(claim.recipient_user_id.expect("内部履约收件人"))
            .fetch_one(store.postgres_pool())
            .await
            .expect("读取收件人版本");
    let recipient_version = u64::try_from(recipient_version).expect("收件人版本必须为非负数");
    SettlementInternalCommand::new(claim, Some(recipient_version), false)
        .expect("构造稳定内部履约命令")
}

/// 模拟付款投递后赢家改绑至 bot，保留原投递的冻结收件人与实收证明。
async fn rebind_winner_to_bot(pool: &PgPool, case: &CodCase) {
    let changed = sqlx::query(
        "UPDATE public.users
         SET username = $2, state_version = state_version + 1, updated_at = now()
         WHERE id = (SELECT recipient_user_id FROM public.settlement_obligations WHERE id = $1)",
    )
    .bind(case.payment)
    .bind(BOT_NAME)
    .execute(pool)
    .await
    .expect("改绑测试赢家");
    assert_eq!(changed.rows_affected(), 1);
}

/// 买家付款后的 Sent 领取证明。生产按已发布的 Sent 证明格式计算摘要（应用层）；存储只要求
/// 非零并原样保存，这里用固定摘要。
fn collected_proof(external_mail_id: &str, observed_at: Timestamp) -> SettlementFulfillmentProof {
    SettlementFulfillmentProof {
        kind: "mooglemail_sent_collected".to_owned(),
        reference: external_mail_id.to_owned(),
        digest: [0x3c; 32],
        observed_at,
    }
}

/// 不经资金化、直接走旧满足入口 `SettlementFulfillmentStore::satisfy`：只在未建立受保护资金
/// 账户时可用，受保护时必须拒绝。
async fn satisfy_by_legacy_entry(
    store: &DatabaseStore,
    case: &CodCase,
) -> Result<SettlementFulfillmentReconcileStatus, auction_application::core::ApplicationError> {
    let claim = claim_payment(store, case).await;
    let snapshot = claim.delivery_snapshot.as_ref().expect("投递快照");
    assert!(snapshot.external_reference_proven, "付款投递的 Sent 引用已证明");
    let reference = snapshot
        .external_reference
        .as_deref()
        .and_then(|reference| reference.rsplit(':').next())
        .expect("Sent MID")
        .to_owned();
    store
        .satisfy(claim, collected_proof(&reference, Timestamp::now()))
        .await
        .map(|result| result.status)
}

async fn assert_fulfillment_completion_clock(pool: &PgPool, obligation: Uuid) {
    let shared: bool = sqlx::query_scalar(
        "SELECT (completed_at = updated_at) IS TRUE
         FROM public.settlement_obligation_fulfillments WHERE obligation_id = $1",
    )
    .bind(obligation)
    .fetch_one(pool)
    .await
    .expect("读取付款履约完成时间");
    assert!(shared, "付款履约的完成与更新必须共用一个时刻");
}

/// 以真实资金化入口执行一次实收；结果类型不实现 Debug，转成稳定标签再断言。
async fn fund_collected(
    store: &DatabaseStore,
    command: &SettlementInternalCommand,
    evidence: &SettlementInternalEvidenceSnapshot,
    external_mail_id: &str,
) -> String {
    let proof = collected_proof(external_mail_id, evidence.observed_from);
    let outcome =
        SettlementInternalFulfillmentStore::satisfy_collected(store, command, &proof, evidence)
            .await;
    match outcome {
        Ok(SettlementInternalCommitOutcome::Committed) => "committed".to_owned(),
        Ok(SettlementInternalCommitOutcome::FundingDeferred) => "funding_deferred".to_owned(),
        Ok(SettlementInternalCommitOutcome::RolledBack(error)) => format!("rolled_back({error:?})"),
        Err(error) => format!("unknown({error:?})"),
    }
}

/// 资金化会触及的全部持久事实；整体相等即没有任何写入留下。
#[derive(Debug, PartialEq, Eq)]
struct LedgerState {
    /// 付款履约：执行状态、状态版本、证明种类与证明引用。
    payment: (String, i64, Option<String>, Option<String>),
    /// 本 lot 全部义务的（种类，业务状态）。
    obligations: Vec<(String, String)>,
    internal_commands: Vec<(String, String)>,
    sources: Vec<(String, i64)>,
    receipts: Vec<(Uuid, String)>,
    funding_lots: Vec<(String, String)>,
    allocations: Vec<(String, i64, String)>,
    entries: Vec<(String, i64)>,
    funding_commands: Vec<String>,
    account_version: i64,
    funded_audits: i64,
}

async fn ledger_state(pool: &PgPool, case: &CodCase) -> LedgerState {
    LedgerState {
        payment: sqlx::query_as(
            "SELECT execution_state, state_version, proof_kind, proof_reference
             FROM public.settlement_obligation_fulfillments WHERE obligation_id = $1",
        )
        .bind(case.payment)
        .fetch_one(pool)
        .await
        .expect("读取付款履约"),
        obligations: sqlx::query_as(
            "SELECT obligation_kind, business_state FROM public.settlement_obligations
             WHERE lot_id = $1 ORDER BY obligation_kind",
        )
        .bind(case.lot)
        .fetch_all(pool)
        .await
        .expect("读取义务"),
        internal_commands: sqlx::query_as(
            "SELECT action, uid FROM public.settlement_internal_commands ORDER BY action",
        )
        .fetch_all(pool)
        .await
        .expect("读取内部命令"),
        sources: sqlx::query_as(
            "SELECT kind, amount FROM public.settlement_funding_sources ORDER BY kind, amount",
        )
        .fetch_all(pool)
        .await
        .expect("读取资金来源"),
        receipts: sqlx::query_as(
            "SELECT delivery_id, external_mail_id FROM public.settlement_funding_receipts
             ORDER BY external_mail_id",
        )
        .fetch_all(pool)
        .await
        .expect("读取实收收据"),
        funding_lots: sqlx::query_as(
            "SELECT source_kind, disposition FROM public.settlement_funding_lots ORDER BY lot_id",
        )
        .fetch_all(pool)
        .await
        .expect("读取 lot 资金绑定"),
        allocations: sqlx::query_as(
            "SELECT purpose, amount, state FROM public.settlement_funding_allocations
             ORDER BY purpose, amount",
        )
        .fetch_all(pool)
        .await
        .expect("读取资金分配"),
        entries: sqlx::query_as(
            "SELECT kind, amount FROM public.settlement_funding_entries ORDER BY kind, amount",
        )
        .fetch_all(pool)
        .await
        .expect("读取资金分录"),
        funding_commands: sqlx::query_scalar(
            "SELECT action FROM public.settlement_funding_commands ORDER BY action",
        )
        .fetch_all(pool)
        .await
        .expect("读取资金命令"),
        account_version: sqlx::query_scalar(
            "SELECT state_version FROM public.settlement_funding_accounts WHERE uid = $1",
        )
        .bind(FUNDING_UID)
        .fetch_one(pool)
        .await
        .expect("读取资金账户版本"),
        funded_audits: sqlx::query_scalar(
            "SELECT count(*) FROM public.audit_records
             WHERE action = 'auction.fulfillment.collected_funded'",
        )
        .fetch_one(pool)
        .await
        .expect("读取实收审计"),
    }
}

/// COD 与载体付款：经投递 fence 建立冻结发送证明后，付款期内计为等待买家付款；寄出远超长期
/// 未领取阈值也不进入等待领取、积压或清单。未建立资金账户时，买家付款领取后由领取证明经旧
/// 满足入口满足，依赖解除，等待付款归零。
#[tokio::test]
async fn postgres_collection_cod_with_frozen_send_proof_waits_for_payment() {
    for payment in [Payment::EquipmentCod, Payment::CarrierCod] {
        let database = support::TestDatabase::create().await.expect("克隆模板库");
        let store = database.store().await.expect("以运行账号连接");
        let pool = store.postgres_pool();
        let now = Timestamp::now();
        let case = seed_case(pool, payment, now + Duration::days(9)).await;
        send_through_fence(&store, payment, &case, None).await;
        assert!(frozen_sent_proof(pool, case.delivery).await, "{payment:?} 冻结发送证明成立");

        // 8 天后观察：寄出时长远超 72 小时阈值，付款期仍未到。
        let later = now + Duration::days(8);
        let snapshot = observe(&store, later).await;
        let fulfillment = snapshot.fulfillment.expect("履约水位");
        // 载体付款的异世界交付受阻于付款，按既有口径计受阻；货款都按等待前置计。
        let blocked = i64::from(payment == Payment::CarrierCod);
        assert_eq!(
            grouping(&fulfillment),
            [1, 0, 0, blocked, 1, 0],
            "{payment:?}：等待付款 1，不进等待领取、积压或超期"
        );
        assert!(snapshot.collection_overdue_items.is_empty(), "{payment:?} 不进长期未领取清单");

        assert_eq!(
            satisfy_by_legacy_entry(&store, &case).await.expect("领取证明满足付款义务"),
            SettlementFulfillmentReconcileStatus::Satisfied,
            "{payment:?}"
        );
        let snapshot = observe(&store, Timestamp::now()).await;
        let fulfillment = snapshot.fulfillment.expect("履约水位");
        assert_eq!(fulfillment.waiting_payment, 0, "{payment:?} 付款领取后不再等待付款");
        let unblocked: String = sqlx::query_scalar(
            "SELECT business_state FROM public.settlement_obligations WHERE id = $1",
        )
        .bind(match payment {
            Payment::EquipmentCod => case.payout,
            Payment::CarrierCod => case.handoff.expect("异世界交付义务"),
        })
        .fetch_one(pool)
        .await
        .expect("读取依赖义务");
        assert_eq!(unblocked, "ready", "{payment:?} 付款满足后解除直接依赖");

        pool.close().await;
    }
}

/// 付款证明必须由原付款产生并绑定同 UID，内部交付才能满足并释放货款。
#[tokio::test]
async fn postgres_internal_isekai_handoff_requires_matching_payment_proof() {
    #[derive(Debug)]
    enum PaymentProof {
        Internal,
        Collected,
        LegacyCollected,
    }

    let payment = Payment::CarrierCod;
    for proof in [PaymentProof::Internal, PaymentProof::Collected, PaymentProof::LegacyCollected] {
        let database = support::TestDatabase::create().await.expect("克隆模板库");
        let store = database.store().await.expect("以运行账号连接");
        let pool = store.postgres_pool();
        let deadline = Timestamp::now() + Duration::days(1);
        let (case, funding) = match proof {
            PaymentProof::Internal => {
                let case = seed_internal_case(pool, payment, deadline).await;
                let funding = seed_funding_in_scope(pool, "mooglemail:shared").await;
                let command = internal_command(&store, case.payment).await;
                let evidence = funding.equipment_evidence(&command, FUNDING_UID);
                assert_internal_commit(&store, &command, &evidence).await;
                (case, funding)
            }
            PaymentProof::Collected => {
                let case = seed_case(pool, payment, deadline).await;
                let funding = seed_funding_in_scope(pool, "mooglemail:shared").await;
                send_through_fence(&store, payment, &case, Some(&funding)).await;
                let claim = claim_payment(&store, &case).await;
                let command =
                    SettlementInternalCommand::new(claim, None, true).expect("构造稳定实收命令");
                let mid = payment.external_mail_id();
                let evidence = funding.collected_evidence(MAIL_GENERATION, &command, payment, mid);
                assert_eq!(fund_collected(&store, &command, &evidence, mid).await, "committed");
                rebind_winner_to_bot(pool, &case).await;
                (case, funding)
            }
            PaymentProof::LegacyCollected => {
                let case = seed_case(pool, payment, deadline).await;
                send_through_fence(&store, payment, &case, None).await;
                assert_eq!(
                    satisfy_by_legacy_entry(&store, &case).await.expect("历史领取满足付款义务"),
                    SettlementFulfillmentReconcileStatus::Satisfied,
                );
                let funding = seed_funding_in_scope(pool, "mooglemail:shared").await;
                rebind_winner_to_bot(pool, &case).await;
                (case, funding)
            }
        };

        let handoff = case.handoff.expect("异世界交付义务");
        let command = internal_command(&store, handoff).await;
        assert_eq!(command.claim.prerequisite_id, Some(SettlementId::from_uuid(case.payment)));
        let legacy = matches!(proof, PaymentProof::LegacyCollected);
        // 构造完整的库存证明。历史路径缺付款 UID，其余路径给出与原付款不同的 UID。
        let uid = if legacy { FUNDING_UID } else { "7003" };
        let evidence = funding.equipment_evidence(&command, uid);
        let before = ledger_state(pool, &case).await;
        let outcome =
            SettlementInternalFulfillmentStore::satisfy_internal(&store, &command, &evidence).await;
        match outcome {
            Ok(SettlementInternalCommitOutcome::RolledBack(FundingError::Unproven)) => {}
            Ok(SettlementInternalCommitOutcome::RolledBack(error)) | Err(error) => {
                panic!("{proof:?} 缺少同 UID 付款证明应拒绝交付：{error:?}")
            }
            Ok(SettlementInternalCommitOutcome::Committed)
            | Ok(SettlementInternalCommitOutcome::FundingDeferred) => {
                panic!("{proof:?} 缺少同 UID 付款证明应整体回滚")
            }
        }
        assert_eq!(ledger_state(pool, &case).await, before);
        let pending: bool = sqlx::query_scalar(
            "SELECT execution_state = 'pending' AND proof_digest IS NULL
             FROM public.settlement_obligation_fulfillments WHERE obligation_id = $1",
        )
        .bind(handoff)
        .fetch_one(pool)
        .await
        .expect("读取被拒绝的交付履约");
        assert!(pending);

        if !legacy {
            let evidence = funding.equipment_evidence(&command, FUNDING_UID);
            assert_internal_commit(&store, &command, &evidence).await;
            let fulfilled: bool = sqlx::query_scalar(
                "SELECT (f.execution_state = 'satisfied' AND f.proof_kind = 'bot_internal_v1'
                    AND f.proof_reference = $2 AND f.proof_digest = $3 AND f.delivery_id IS NULL
                    AND payout.business_state = 'ready') IS TRUE
                 FROM public.settlement_obligation_fulfillments f
                 JOIN public.settlement_obligations payout ON payout.id = $4
                 WHERE f.obligation_id = $1",
            )
            .bind(handoff)
            .bind(command.command_id.to_string())
            .bind(evidence.digest.as_slice())
            .bind(case.payout)
            .fetch_one(pool)
            .await
            .expect("读取交付证明与货款状态");
            assert!(fulfilled, "同 UID 的付款证明允许内部交付并释放货款");
        }
        pool.close().await;
    }
}

async fn assert_internal_commit(
    store: &DatabaseStore,
    command: &SettlementInternalCommand,
    evidence: &SettlementInternalEvidenceSnapshot,
) {
    match SettlementInternalFulfillmentStore::satisfy_internal(store, command, evidence).await {
        Ok(SettlementInternalCommitOutcome::Committed) => {}
        Ok(SettlementInternalCommitOutcome::FundingDeferred) => {
            panic!("有效 record_only 证据不得延期")
        }
        Ok(SettlementInternalCommitOutcome::RolledBack(error)) | Err(error) => {
            panic!("内部履约失败：{error:?}")
        }
    }
}

/// 记录型付款走真实内部履约与资金 owner；来源、补偿与 seller credit 都由生产事务生成。
#[tokio::test]
async fn postgres_record_only_internal_purchase_uses_frozen_lot_sources() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let case =
        seed_internal_case(pool, Payment::EquipmentCod, Timestamp::now() + Duration::days(1)).await;
    let funding = seed_funding(pool).await;
    let command = internal_command(&store, case.payment).await;
    let evidence = funding.equipment_evidence(&command, FUNDING_UID);
    assert_internal_commit(&store, &command, &evidence).await;

    let (payment_amount, compensation_amount): (i64, i64) = sqlx::query_as(
        "SELECT buyer_amount, compensation_amount FROM public.lot_settlements WHERE lot_id = $1",
    )
    .bind(case.lot)
    .fetch_one(pool)
    .await
    .expect("读取冻结资金拆分");
    let (purchase_source, compensation_source): (i64, i64) = sqlx::query_as(
        "SELECT max(source.amount) FILTER (WHERE source.kind = 'record_internal_purchase'),
                max(source.amount) FILTER (WHERE source.kind = 'record_compensation')
         FROM public.settlement_funding_sources AS source
         WHERE source.uid = $1 AND source.source_key = $2",
    )
    .bind(FUNDING_UID)
    .bind(case.payment)
    .fetch_one(pool)
    .await
    .expect("读取内部购买来源");
    assert_eq!((purchase_source, compensation_source), (payment_amount, compensation_amount));
    let (seller_credit, seller_obligation, seller_amount): (Uuid, Uuid, i64) = sqlx::query_as(
        "SELECT allocation.id, allocation.obligation_id, allocation.amount
         FROM public.settlement_funding_allocations AS allocation
         WHERE allocation.uid = $1 AND allocation.lot_id = $2 AND allocation.purpose = 'seller'",
    )
    .bind(FUNDING_UID)
    .bind(case.lot)
    .fetch_one(pool)
    .await
    .expect("读取 seller payout credit allocation");
    assert_eq!((seller_obligation, seller_amount), (case.payout, SELLER_CREDITS));
    let issues_target_seller: Option<bool> = sqlx::query_scalar(
        "SELECT bool_and(entry.credit_id = $2)
         FROM public.settlement_funding_entries AS entry
         JOIN public.settlement_funding_sources AS source ON source.id = entry.source_id
         WHERE entry.command_id = $1 AND entry.kind = 'issue'
           AND source.kind IN ('record_internal_purchase', 'record_compensation')",
    )
    .bind(command.command_id)
    .bind(seller_credit)
    .fetch_one(pool)
    .await
    .expect("检查内部购买发行目标");
    assert_eq!(issues_target_seller, Some(true), "记录型来源只发行至 seller allocation");
    let action: String = sqlx::query_scalar(
        "SELECT action FROM public.settlement_funding_commands
         WHERE command_id = $1 AND target_id = $2",
    )
    .bind(command.command_id)
    .bind(case.lot)
    .fetch_one(pool)
    .await
    .expect("读取内部购买资金命令");
    assert_eq!(action, "internal_purchase");
    pool.close().await;
}

/// 存在受保护资金账户时，旧满足入口拒绝 COD 与载体付款：领取后必须经资金化校验，义务保持
/// 等待付款，不被旧路径直接满足。
#[tokio::test]
async fn postgres_collection_cod_collected_requires_funding_when_protected() {
    for payment in [Payment::EquipmentCod, Payment::CarrierCod] {
        let database = support::TestDatabase::create().await.expect("克隆模板库");
        let store = database.store().await.expect("以运行账号连接");
        let pool = store.postgres_pool();
        let now = Timestamp::now();
        let case = seed_case(pool, payment, now + Duration::days(1)).await;
        send_through_fence(&store, payment, &case, None).await;
        sqlx::query(
            "INSERT INTO public.settlement_funding_accounts (uid, status) VALUES ($1, 'ready')",
        )
        .bind(FUNDING_UID)
        .execute(pool)
        .await
        .expect("建立受保护资金账户");

        assert!(
            satisfy_by_legacy_entry(&store, &case).await.is_err(),
            "{payment:?} 受保护时旧满足入口必须拒绝"
        );
        let state: String = sqlx::query_scalar(
            "SELECT execution_state FROM public.settlement_obligation_fulfillments
             WHERE obligation_id = $1",
        )
        .bind(case.payment)
        .fetch_one(pool)
        .await
        .expect("读取付款履约");
        assert_eq!(state, "enqueued", "{payment:?} 付款义务未被旧路径满足");
        let snapshot = observe(&store, Timestamp::now()).await;
        assert_eq!(snapshot.fulfillment.map(|value| value.waiting_payment), Some(1));

        pool.close().await;
    }
}

/// 资金化正向路径（不变量 1、2、6）：两类付款都直接调用真实 `satisfy_collected`。先以错 MID 与
/// 失效运行代际各试一次，整事务回滚、不留任何写入；再以当前代际的同 session 实收证明提交：
/// 付款满足并解除直接依赖，买方实收登记为 collected_payment 来源；冻结补偿份额经 receipt 绑定的
/// record_compensation 来源发行到 seller 分配。同一证明重放不重复入账。
#[tokio::test]
async fn postgres_collection_cod_collected_payment_is_funded_through_satisfy_collected() {
    for payment in [Payment::EquipmentCod, Payment::CarrierCod] {
        let database = support::TestDatabase::create().await.expect("克隆模板库");
        let store = database.store().await.expect("以运行账号连接");
        let pool = store.postgres_pool();
        let case = seed_case(pool, payment, Timestamp::now() + Duration::days(3)).await;
        // 账户与开户来源先于付款发送：寄出时刻晚于资金来源边界。
        let funding = seed_funding(pool).await;
        send_through_fence(&store, payment, &case, Some(&funding)).await;
        assert!(frozen_sent_proof(pool, case.delivery).await, "{payment:?} 冻结发送证明成立");

        let claim = claim_payment(&store, &case).await;
        let command = SettlementInternalCommand::new(claim, None, true).expect("构造稳定实收命令");
        let mid = payment.external_mail_id();
        let before = ledger_state(pool, &case).await;

        // 错 MID：同 session 看到的是另一封已领取的 Sent，原投递的 Sent 绑定不成立。
        let generation = current_generation(pool).await;
        let wrong = funding.collected_evidence(generation, &command, payment, "7400499");
        assert_eq!(
            fund_collected(&store, &command, &wrong, "7400499").await,
            "rolled_back(Unproven)",
            "{payment:?} 错 MID"
        );
        assert_eq!(ledger_state(pool, &case).await, before, "{payment:?} 错 MID 不留任何写入");

        // 失效代际：观察之后邮件运行时换代，旧代际证据在首把锁处失败。
        let stale = funding.collected_evidence(generation, &command, payment, mid);
        sqlx::query(
            "UPDATE public.mail_runtime_generation
             SET generation = generation + 1, activated_at = now() WHERE scope = 'mail_cookie'",
        )
        .execute(pool)
        .await
        .expect("推进邮件运行代际");
        assert_eq!(
            fund_collected(&store, &command, &stale, mid).await,
            "rolled_back(Unproven)",
            "{payment:?} 失效代际"
        );
        assert_eq!(ledger_state(pool, &case).await, before, "{payment:?} 失效代际不留任何写入");

        let evidence =
            funding.collected_evidence(current_generation(pool).await, &command, payment, mid);
        assert_eq!(
            fund_collected(&store, &command, &evidence, mid).await,
            "committed",
            "{payment:?}"
        );

        let after = ledger_state(pool, &case).await;
        assert_eq!(
            after.payment,
            (
                "satisfied".to_owned(),
                before.payment.1 + 1,
                Some("mooglemail_sent_collected".to_owned()),
                Some(mid.to_owned()),
            ),
            "{payment:?} 付款以领取证明满足"
        );
        assert_fulfillment_completion_clock(pool, case.payment).await;
        let state = |kind: SettlementObligationKind| {
            after
                .obligations
                .iter()
                .find(|(obligation, _)| obligation == kind.as_str())
                .map(|(_, state)| state.as_str())
        };
        match payment {
            Payment::EquipmentCod => {
                assert_eq!(state(SettlementObligationKind::SellerPayout), Some("ready"));
            }
            Payment::CarrierCod => {
                assert_eq!(state(SettlementObligationKind::WinnerEquipmentHandoff), Some("ready"));
                assert_eq!(
                    state(SettlementObligationKind::SellerPayout),
                    Some("blocked"),
                    "货款仍等异世界交付"
                );
            }
        }
        assert_eq!(state(SettlementObligationKind::BuyerRefund), Some("contingent"));
        assert_eq!(
            after.internal_commands,
            vec![("collected".to_owned(), FUNDING_UID.to_owned())],
            "{payment:?} 实收命令只记一次"
        );

        let source: (i64, Vec<u8>) = sqlx::query_as(
            "SELECT amount, proof_digest FROM public.settlement_funding_sources
             WHERE uid = $1 AND kind = 'collected_payment' AND source_key = $2",
        )
        .bind(FUNDING_UID)
        .bind(case.payment)
        .fetch_one(pool)
        .await
        .expect("读取实收来源");
        let sent_digest = evidence.sent_collection.as_ref().expect("Sent 观察").digest;
        assert_eq!(source, (BUYER_CREDITS, sent_digest.to_vec()), "{payment:?} 来源即冻结买方金额");
        let receipt: (Uuid, Uuid, String, String) = sqlx::query_as(
            "SELECT payment_obligation_id, delivery_id, external_mail_id, uid
             FROM public.settlement_funding_receipts",
        )
        .fetch_one(pool)
        .await
        .expect("读取实收收据");
        assert_eq!(
            receipt,
            (case.payment, case.delivery, mid.to_owned(), FUNDING_UID.to_owned()),
            "{payment:?} 收据绑定付款、原投递与唯一 Sent MID"
        );
        let lot_binding: (Uuid, String, String) = sqlx::query_as(
            "SELECT payment_obligation_id, source_kind, disposition
             FROM public.settlement_funding_lots WHERE lot_id = $1 AND uid = $2",
        )
        .bind(case.lot)
        .bind(FUNDING_UID)
        .fetch_one(pool)
        .await
        .expect("读取 lot 资金绑定");
        assert_eq!(lot_binding, (case.payment, "collected".to_owned(), "seller".to_owned()));
        let compensation_binding: (i64, String, Uuid, String, Uuid, String, String, Uuid, i64) =
            sqlx::query_as(
                "SELECT compensation.amount, compensation.uid, compensation.source_key,
                        funded.source_kind, receipt.payment_obligation_id, receipt.uid,
                        buyer.kind, buyer.source_key, buyer.amount
                 FROM public.settlement_funding_sources AS compensation
                 JOIN public.settlement_funding_lots AS funded
                   ON funded.lot_id = $1 AND funded.uid = compensation.uid
                 JOIN public.settlement_funding_receipts AS receipt
                   ON receipt.uid = compensation.uid
                  AND receipt.payment_obligation_id = compensation.source_key
                 JOIN public.settlement_funding_sources AS buyer ON buyer.id = receipt.source_id
                 WHERE compensation.uid = $2 AND compensation.kind = 'record_compensation'
                   AND compensation.source_key = $3",
            )
            .bind(case.lot)
            .bind(FUNDING_UID)
            .bind(case.payment)
            .fetch_one(pool)
            .await
            .expect("读取绑定实收收据的记录补偿来源");
        assert_eq!(
            compensation_binding,
            (
                COMPENSATION_CREDITS,
                FUNDING_UID.to_owned(),
                case.payment,
                "collected".to_owned(),
                case.payment,
                FUNDING_UID.to_owned(),
                "collected_payment".to_owned(),
                case.payment,
                BUYER_CREDITS,
            ),
            "记录补偿只绑定同 UID、lot、付款义务与实收来源"
        );
        let allocations: Vec<(String, Option<Uuid>, i64, String)> = sqlx::query_as(
            "SELECT purpose, obligation_id, amount, state FROM public.settlement_funding_allocations
             WHERE lot_id = $1 AND purpose IN ('seller', 'tax') ORDER BY purpose",
        )
        .bind(case.lot)
        .fetch_all(pool)
        .await
        .expect("读取货款与税费分配");
        assert_eq!(
            allocations,
            vec![
                ("seller".to_owned(), Some(case.payout), SELLER_CREDITS, "available".to_owned()),
                ("tax".to_owned(), None, TAX_CREDITS, "available".to_owned()),
            ],
            "{payment:?} 买方金额按冻结明细分给货款与税费"
        );
        let entries: Vec<(String, i64)> = sqlx::query_as(
            "SELECT kind, amount FROM public.settlement_funding_entries
             WHERE command_id = $1 ORDER BY kind, amount",
        )
        .bind(command.command_id)
        .fetch_all(pool)
        .await
        .expect("读取本命令分录");
        assert_eq!(
            entries,
            vec![("issue".to_owned(), COMPENSATION_CREDITS), ("issue".to_owned(), BUYER_CREDITS),],
            "{payment:?} 真实实收与补偿来源都只按冻结金额发行"
        );
        assert_eq!(after.account_version, before.account_version + 1);
        assert_eq!(after.funded_audits, 1);

        // 同一证明重放：内部命令账本认出原负载，事务直接收口，不再入账或改状态。
        assert!(
            SettlementInternalFulfillmentStore::replay_internal(&store, command.clone())
                .await
                .expect("读取内部命令账本"),
            "{payment:?} 原命令已提交"
        );
        let replay =
            funding.collected_evidence(current_generation(pool).await, &command, payment, mid);
        assert_eq!(fund_collected(&store, &command, &replay, mid).await, "committed");
        assert_eq!(ledger_state(pool, &case).await, after, "{payment:?} 重放不重复入账");

        pool.close().await;
    }
}

/// 资金来源边界（不变量 2、6）：付款邮件寄出时还没有资金账户，开户来源晚于寄出。实收事务先写入
/// 付款满足、lot 资金绑定与分配，再在实收准入处因“寄出不晚于来源边界”被拒；整事务回滚，不留
/// 任何写入，付款仍待资金化。
#[tokio::test]
async fn postgres_collection_cod_funding_rolls_back_when_the_send_predates_the_funding_source() {
    for payment in [Payment::EquipmentCod, Payment::CarrierCod] {
        let database = support::TestDatabase::create().await.expect("克隆模板库");
        let store = database.store().await.expect("以运行账号连接");
        let pool = store.postgres_pool();
        let case = seed_case(pool, payment, Timestamp::now() + Duration::days(3)).await;
        send_through_fence(&store, payment, &case, None).await;
        let funding = seed_funding(pool).await;

        let claim = claim_payment(&store, &case).await;
        let command = SettlementInternalCommand::new(claim, None, true).expect("构造稳定实收命令");
        let mid = payment.external_mail_id();
        let before = ledger_state(pool, &case).await;
        let evidence =
            funding.collected_evidence(current_generation(pool).await, &command, payment, mid);
        assert_eq!(
            fund_collected(&store, &command, &evidence, mid).await,
            "rolled_back(Unproven)",
            "{payment:?} 寄出早于资金来源"
        );
        assert_eq!(ledger_state(pool, &case).await, before, "{payment:?} 已执行的写入一并回滚");

        pool.close().await;
    }
}

/// 反例：付款投递已成功，但没有经投递 fence 留下发送尝试与 Sent 绑定，数据库不认冻结发送证明，
/// 付款义务不计为等待付款。
#[tokio::test]
async fn postgres_collection_cod_without_frozen_send_proof_is_not_waiting_payment() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let now = Timestamp::now();
    let case = seed_case(pool, Payment::EquipmentCod, now + Duration::days(1)).await;
    sqlx::query(
        "UPDATE public.deliveries
         SET status = 'succeeded', fencing_version = 1, attempt_count = 1,
             claimed_at = now(), sending_at = now(), completed_at = now(), updated_at = now(),
             external_reference = 'mooglemail:main:send:7400409',
             result_metadata = jsonb_build_object(
                 'operation', 'send', 'external_reference_proven', true,
                 'sent_recovered', true, 'proof', 'sent_recovered_frozen_v1')
         WHERE id = $1",
    )
    .bind(case.delivery)
    .execute(pool)
    .await
    .expect("写入缺少冻结证明的成功投递");
    assert!(!frozen_sent_proof(pool, case.delivery).await, "没有发送尝试与 Sent 绑定");
    let snapshot = observe(&store, now).await;
    let fulfillment = snapshot.fulfillment.expect("履约水位");
    assert_eq!(fulfillment.waiting_payment, 0, "缺少冻结证明不计为等待付款");
    assert_eq!(fulfillment.awaiting_collection, 0, "COD 不进入等待领取");
    assert_eq!(fulfillment.pending, 1, "按既有口径仍是待处理");

    pool.close().await;
}

/// 迁移 120 验证历史 carrier issue 时必须使用只追加派发历史，而非可变的当前履约指针。
#[tokio::test]
async fn postgres_migration_120_accepts_historical_superseded_record_carrier_issue() {
    let database = support::TestDatabase::create_empty().await.expect("创建未迁移测试库");
    let maintenance = database.maintenance_store().await.expect("连接测试库 owner");
    postgres_migrate_to(maintenance.postgres_pool(), 119).await.expect("迁移到 119");

    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let case = seed_case(pool, Payment::CarrierCod, Timestamp::now() + Duration::days(3)).await;
    sqlx::query(
        "INSERT INTO public.settlement_fulfillment_dispatches
            (obligation_id, dispatch_no, delivery_id, authorized_by, recorded_at)
         VALUES ($1, 1, $2, NULL, now())",
    )
    .bind(case.payment)
    .bind(case.delivery)
    .execute(pool)
    .await
    .expect("登记首寄历史");
    // 迁移只读取历史来源、发行分录及派发身份；直接以 119 的列写入，不调用依赖 131 的资金入口。
    sqlx::query(
        "INSERT INTO public.settlement_funding_accounts
            (uid, accounting_mode, status, state_version)
         VALUES ($1, 'record_only', 'ready', 1)",
    )
    .bind(FUNDING_UID)
    .execute(pool)
    .await
    .expect("写入历史只记账账户");
    let allocation = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.settlement_funding_allocations
            (id, uid, purpose, lot_id, obligation_id, amount)
         VALUES ($1, $2, 'carrier', $3, $4, 1)",
    )
    .bind(allocation)
    .bind(FUNDING_UID)
    .bind(case.lot)
    .bind(case.payment)
    .execute(pool)
    .await
    .expect("写入历史载体分配");
    sqlx::query(
        "WITH source AS (
            INSERT INTO public.settlement_funding_sources
                (id, uid, kind, source_key, amount, proof_digest)
            VALUES ($1, $2, 'record_carrier', $3, 1, decode(repeat('31', 32), 'hex'))
            RETURNING id, uid, amount, proof_digest
         )
         INSERT INTO public.settlement_funding_entries
            (id, command_id, uid, credit_id, source_id, amount, kind, proof_digest)
         SELECT $4, $3, source.uid, $5, source.id, source.amount, 'issue', source.proof_digest
         FROM source",
    )
    .bind(Uuid::new_v4())
    .bind(FUNDING_UID)
    .bind(case.delivery)
    .bind(Uuid::new_v4())
    .bind(allocation)
    .execute(pool)
    .await
    .expect("写入首寄载体来源及发行分录");

    let existing_issue: i64 = sqlx::query_scalar(
        "SELECT count(*)::bigint
         FROM public.settlement_funding_sources AS source
         JOIN public.settlement_funding_entries AS entry ON entry.source_id = source.id
         WHERE source.kind = 'record_carrier' AND source.source_key = $1 AND entry.kind = 'issue'",
    )
    .bind(case.delivery)
    .fetch_one(pool)
    .await
    .expect("确认历史 carrier issue");
    assert_eq!(existing_issue, 1, "首寄必须已有一笔 1-credit carrier issue");

    let now = Timestamp::now();
    let return_mail_id = Uuid::new_v4();
    let external_mail_id = Uuid::new_v4().as_u128().to_string();
    let next_delivery = Uuid::new_v4();
    let next_prepare = Uuid::new_v4();
    let next_activate = Uuid::new_v4();
    let mut transaction = pool.begin().await.expect("开启补寄历史事务");
    sqlx::query(
        "INSERT INTO public.inbound_mail
            (id, mailbox_key, external_mail_id, world_id, mail_kind, content_digest,
             received_at, status, decision_code, processed_at, created_at)
         VALUES ($1, 'main:bot', $2, 'main', 'delivery', decode(repeat('55', 32), 'hex'),
                 $3, 'accepted', 'settlement_recovery_returned', $3, $3)",
    )
    .bind(return_mail_id)
    .bind(&external_mail_id)
    .bind(now)
    .execute(&mut *transaction)
    .await
    .expect("写入补寄授权退件");
    sqlx::query(
        "INSERT INTO public.settlement_return_evidence
            (return_mail_id, match_state, reason_code, obligation_id, delivery_id,
             previous_execution_state, recorded_at)
         VALUES ($1, 'manual_reconcile', 'source_unproven', $2, $3, 'enqueued', $4)",
    )
    .bind(return_mail_id)
    .bind(case.payment)
    .bind(case.delivery)
    .bind(now)
    .execute(&mut *transaction)
    .await
    .expect("写入补寄退件证据");
    sqlx::query(
        "INSERT INTO public.settlement_return_cases
            (return_mail_id, world_id, obligation_id, delivery_id, dispatch_no, status,
             reason_code, plan, resend_limit, resend_dispatch_no, created_at, updated_at)
         VALUES ($1, 'main', $2, $3, 1, 'resend_pending', 'source_unproven', 'resend',
                 3, 2, $4, $4)",
    )
    .bind(return_mail_id)
    .bind(case.payment)
    .bind(case.delivery)
    .bind(now)
    .execute(&mut *transaction)
    .await
    .expect("写入补寄授权案件");
    let cloned = sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
             status, activation_command_id, activation_request_digest, activated_at,
             created_at, updated_at)
         SELECT $1, $2, decode(repeat('b1', 32), 'hex'), origin_kind, origin_id, recipient,
                payload, 'pending', $3, decode(repeat('b2', 32), 'hex'), $4, $4, $4
         FROM public.deliveries WHERE id = $5",
    )
    .bind(next_delivery)
    .bind(next_prepare)
    .bind(next_activate)
    .bind(now)
    .bind(case.delivery)
    .execute(&mut *transaction)
    .await
    .expect("写入补寄投递");
    assert_eq!(cloned.rows_affected(), 1, "必须从首寄载荷复制补寄投递");
    sqlx::query(
        "INSERT INTO public.settlement_fulfillment_dispatches
            (obligation_id, dispatch_no, delivery_id, authorized_by, recorded_at)
         VALUES ($1, 2, $2, $3, $4)",
    )
    .bind(case.payment)
    .bind(next_delivery)
    .bind(return_mail_id)
    .bind(now)
    .execute(&mut *transaction)
    .await
    .expect("登记补寄派发历史");
    sqlx::query(
        "UPDATE public.settlement_obligation_fulfillments
         SET dispatch_no = 2, delivery_id = $2, prepare_command_id = $3,
             prepare_request_digest = decode(repeat('b1', 32), 'hex'),
             activate_command_id = $4,
             activate_request_digest = decode(repeat('b2', 32), 'hex'),
             state_version = state_version + 1, updated_at = $5
         WHERE obligation_id = $1",
    )
    .bind(case.payment)
    .bind(next_delivery)
    .bind(next_prepare)
    .bind(next_activate)
    .bind(now)
    .execute(&mut *transaction)
    .await
    .expect("把当前履约切换到补寄派发");
    transaction.commit().await.expect("提交补寄历史");

    let current_delivery: Uuid = sqlx::query_scalar(
        "SELECT delivery_id FROM public.settlement_obligation_fulfillments
         WHERE obligation_id = $1",
    )
    .bind(case.payment)
    .fetch_one(pool)
    .await
    .expect("读取当前派发");
    assert_ne!(current_delivery, case.delivery, "测试必须让首寄成为已被换代的历史派发");

    store.postgres_pool().close().await;
    postgres_migrate_to(maintenance.postgres_pool(), 120)
        .await
        .expect("迁移 120 必须接受只由派发历史证明的既有 carrier issue");
    let applied: i64 = sqlx::query_scalar(
        "SELECT count(*)::bigint FROM public._sqlx_migrations WHERE version = 120",
    )
    .fetch_one(maintenance.postgres_pool())
    .await
    .expect("确认迁移 120 已应用");
    assert_eq!(applied, 1, "升级路径必须执行迁移 120");
    maintenance.postgres_pool().close().await;
}
