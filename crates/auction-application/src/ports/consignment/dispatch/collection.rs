//! 收款与确领推进的待办（PIWQ）：已成功派发、已绑定 Sent 的寄售轮，以及推进它所需的冻结事实。
//!
//! 待办只描述“哪一轮还差哪一步”；是否已领取由只读 Sent 观察决定，推进只经既有写口（`record_evidence`、
//! `certify_cod`、`prove_handoff`）。冻结 Send 身份含收件人与主题，只用于本次比对，不进日志。

use auction_domain::SettlementWorld;
use auction_domain::consignment::dispatch::{AttemptKey, DispatchRoundKey, EvidenceCategory};
use auction_domain::consignment::funding::FundingMode;
use auction_domain::consignment::{ListingPrice, ListingQuote};
use auction_time::Timestamp;
use uuid::Uuid;

use crate::ports::delivery::DeliverySendExpectation;
use crate::ports::settlement::{DispatchFreeze, ReceiptKey};

/// 待推进的派发腿。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectionLeg {
    /// main 原件 COD：一次领取即付款（P）与原件确领。
    MainOriginalCod,
    /// isekai 载体 COD：买家领取 1 Credit 载体并实付 P + 1。
    CarrierCod,
    /// isekai 原件交割轮：买家确领原件。
    OriginalHandoff,
    /// 无 COD 现金轮（卖款 S、退款 P）：受益人确领 Credits。
    Cash,
}

impl CollectionLeg {
    /// 本腿领取事实对应的证据类别：main 原件 COD 的付款与确领共用同一条原件确领证据（与收款认证读取的类别一致）。
    pub const fn category(self) -> EvidenceCategory {
        match self {
            Self::MainOriginalCod | Self::OriginalHandoff => EvidenceCategory::OriginalClaim,
            Self::CarrierCod => EvidenceCategory::CarrierPayment,
            Self::Cash => EvidenceCategory::CashClaim,
        }
    }
}

/// 已记录且出自已签收验证器的领取证据身份。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectedEvidence {
    /// 远端观察 ID（被领取的那封 Sent 的 MID）。
    pub remote_observation_id: String,
    /// 证据摘要。
    pub evidence_digest: [u8; 32],
}

/// 认证 COD 实收所需的冻结事实：全部取自原派发与成交，Worker 不推算金额。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertifyCodFacts {
    /// Sale 合同。
    pub contract_id: Uuid,
    /// 原付款义务（main 原件 COD、isekai 载体 COD）。
    pub payment_obligation_id: Uuid,
    /// 原付款投递。
    pub original_delivery_id: Uuid,
    /// 实收业务键：成交冻结的资金 UID、原付款 Sent MID 与收款腿。
    pub receipt: ReceiptKey,
    /// 合同世界（决定收款腿）。
    pub world: SettlementWorld,
    /// 成交冻结的资金模式。
    pub mode: FundingMode,
    /// 成交价 P。
    pub price: ListingPrice,
    /// 成交冻结的 S 与 F。
    pub quote: ListingQuote,
    /// 原派发冻结的 COD 毛额（main 为 P，isekai 载体为 P + 1）。
    pub gross: i64,
    /// 原派发的冻结身份。
    pub freeze: DispatchFreeze,
}

/// 一轮待推进的收款或确领。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingCollection {
    /// 轮业务键。
    pub key: DispatchRoundKey,
    /// 该轮已绑定 Sent 的起写尝试。
    pub attempt: AttemptKey,
    /// 派发腿。
    pub leg: CollectionLeg,
    /// 发信世界（isekai 载体与现金轮在 main 发出）。
    pub send_world: SettlementWorld,
    /// 已绑定的 Sent MID。
    pub sent_mid: String,
    /// 起写事务冻结的 `sending_at`。
    pub sending_at: Timestamp,
    /// 起写时冻结的 Send 身份。
    pub expectation: DeliverySendExpectation,
    /// 已记录、出自已签收验证器的领取证据；没有时须先只读观察 Sent。
    pub evidence: Option<CollectedEvidence>,
    /// 付款腿尚未认证时的认证事实；已认证或非付款腿为 `None`。
    pub certify: Option<CertifyCodFacts>,
}

/// 一次扫描内的键集游标：上一页最后一行待办的起写时刻与尝试 ID（按起写先后排序的键）。游标只活在本次扫描里，
/// 不跨轮、不持久，不作为任何事实来源；被跳过的行同样推进游标，异常行不会占住后面的待办。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CollectionCursor {
    /// 起写时刻。
    pub started_at: Timestamp,
    /// 尝试 ID。
    pub attempt_id: Uuid,
}

/// 一页待办。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectionPage {
    /// 本页可推进的待办（冻结事实不全的行已跳过）。
    pub items: Vec<PendingCollection>,
    /// 读满一页时为本页最后一行的游标，读不满即本次扫描结束（`None`）。
    pub next: Option<CollectionCursor>,
}
