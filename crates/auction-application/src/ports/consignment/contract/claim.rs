//! 寄售义务的专用领取端口：与拍卖领取并列，不改变拍卖领取类型。

use auction_domain::consignment::{ConsignmentContract, ConsignmentObligation};
use auction_domain::{SettlementId, SettlementWorld};
use auction_time::Timestamp;
use uuid::Uuid;

use crate::ports::consignment::errors::ConsignmentFuture;

/// 一次寄售领取：义务、合同与发送所需的冻结事实。
///
/// 买家与卖家的冻结身份在 prepare 前须由 users owner 复核（第 6 期实现），本期不 prepare。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsignmentObligationClaim {
    pub obligation: ConsignmentObligation,
    pub contract: ConsignmentContract,
    /// carrier、卖款与退款在 main 派发，其余在义务世界。
    pub dispatch_world: SettlementWorld,
    /// 同事务读取的冻结快照名称。
    pub equipment_name: String,
    pub external_equipment_id: String,
    pub claim_token: Uuid,
    pub claim_fencing: i64,
    pub claimed_at: Timestamp,
    pub lease_until: Timestamp,
    pub retry_count: u32,
}

/// 来源损坏转人工对账的原因；损坏义务不发送、不自动重试。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimIsolation {
    SourceMissing,
    WorldMismatch,
    AmountMismatch,
    RecipientMismatch,
    Other(&'static str),
}

/// 寄售义务领取端口。
pub trait ConsignmentFulfillmentClaims: Send + Sync {
    /// 与拍卖领取同一机制（按义务 UUID 行锁跳过已锁、登记 claim 与 fence、先提交释放），
    /// 只领寄售来源且前置已 satisfied 的 ready 义务；来源损坏的义务转人工对账并返回其余。
    fn claim_consignment_obligations(
        &self,
        now: Timestamp,
        worker_id: &str,
        lease_until: Timestamp,
        limit: u16,
    ) -> ConsignmentFuture<'_, Vec<ConsignmentObligationClaim>>;

    /// 释放未起写的领取；本期没有任何调用会把寄售领取推进到 prepare。
    fn release_consignment_claim(
        &self,
        obligation_id: SettlementId,
        claim_token: Uuid,
        claim_fencing: i64,
        now: Timestamp,
    ) -> ConsignmentFuture<'_, ()>;
}
