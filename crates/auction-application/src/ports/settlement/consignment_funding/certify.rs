//! COD 付款认证命令与回执。

use auction_domain::SettlementWorld;
use auction_domain::consignment::funding::{CodReceiptSplit, FundingMode, TransitPlan};
use auction_domain::consignment::{ListingPrice, ListingQuote};
use auction_time::Timestamp;
use uuid::Uuid;

use crate::ports::settlement::{FundingError, FundingMutation};

/// COD 腿：main 为原件付款，isekai 为载体付款。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CodLeg {
    Original,
    Carrier,
}

impl CodLeg {
    /// 世界决定的腿：main 付款是原件腿，isekai 付款是载体腿。
    pub const fn for_world(world: SettlementWorld) -> Self {
        match world {
            SettlementWorld::Main => Self::Original,
            SettlementWorld::Isekai => Self::Carrier,
        }
    }

    /// 本腿必须正是该世界的收款腿，否则载体金额可能记到原件收据上（不变量 6，失败关闭）。
    /// 真实存储实现读到义务的世界与种类后复用这一条规则，不另写判断。
    pub fn check_world(self, world: SettlementWorld) -> Result<(), FundingError> {
        if self != Self::for_world(world) {
            return Err(FundingError::Invalid);
        }
        Ok(())
    }
}

/// 实收业务键：原 Bot UID、main、原付款 Sent MID 与 COD 腿；版本、边界与来源类型不参与换身份。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ReceiptKey {
    pub uid: String,
    pub payment_mail_id: String,
    pub leg: CodLeg,
}

/// 原派发的冻结身份：Worker 从自己的派发轮读出，存储在锁内与持久的冻结事实逐项比对，
/// 任一项不符都按冲突整笔拒绝（D9 §4.2 第 1 步）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchFreeze {
    /// 原 prepare 命令的请求摘要（`deliveries.request_digest`）。
    pub prepare_digest: [u8; 32],
    /// 原 activate 命令的请求摘要（`deliveries.activation_request_digest`）。
    pub activation_digest: [u8; 32],
    /// 派发轮冻结的正文摘要，亦是 Send 尝试冻结的期望正文摘要。
    pub body_digest: [u8; 32],
    /// 冻结的装备外部 ID 集合；载体腿没有装备，为空。
    pub equipment_ids: Vec<String>,
}

/// 严格现金模式下 Worker 在 Sent 收款后新观察到的 Main 现金；`record_only` 不带。
/// 存储在 UID 账户锁内以数据库时钟复验新鲜度，并核对余额仍覆盖全部现有占用加本笔新发行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FreshCashObservation {
    /// 观察所属的认证 Bot UID，必须等于实收业务键的 UID。
    pub uid: String,
    pub credits: i64,
    pub observed_at: Timestamp,
    pub expires_at: Timestamp,
}

/// 认证一笔 COD 实收；键 = 原付款义务 + 原 delivery。
#[derive(Debug, Clone)]
pub struct CertifyCod {
    pub mutation: FundingMutation,
    pub contract_id: Uuid,
    pub payment_obligation_id: Uuid,
    pub original_delivery_id: Uuid,
    pub receipt: ReceiptKey,
    pub world: SettlementWorld,
    pub mode: FundingMode,
    pub price: ListingPrice,
    pub quote: ListingQuote,
    pub gross: i64,
    pub freeze: DispatchFreeze,
    /// 严格现金必须带新鲜现金观察，`record_only` 必须不带（见 [`CertifyCod::check_cash`]）。
    pub cash: Option<FreshCashObservation>,
    /// 已封存的 opening 份额；`None` 表示已证全部未含（A = 0）。
    pub attribution_id: Option<Uuid>,
}

impl CertifyCod {
    /// 世界与收据腿必须对应，否则载体金额可能记到原件收据上（不变量 6，失败关闭）。
    pub fn check_leg(&self) -> Result<(), FundingError> {
        self.receipt.leg.check_world(self.world)
    }

    /// 现金观察与模式必须对应：严格现金缺观察无法证明现金可支用，`record_only` 不读个人余额
    /// 却带了观察则是不可解释的输入（不变量 6，失败关闭）。
    pub fn check_cash(&self) -> Result<(), FundingError> {
        match (self.mode, &self.cash) {
            (FundingMode::StrictCash, Some(cash)) if cash.uid == self.receipt.uid => Ok(()),
            (FundingMode::RecordOnly, None) => Ok(()),
            _ => Err(FundingError::Invalid),
        }
    }
}

/// 认证结果：收据来源、拆分与过渡用途计划。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertifyReceipt {
    pub contract_id: Uuid,
    pub receipt_source_id: Uuid,
    pub split: CodReceiptSplit,
    pub plan: TransitPlan,
    pub replayed: bool,
}
