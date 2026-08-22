use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

use auction_domain::{
    AuctionId, CommandId, EquipmentId, LotId, SettlementId, SettlementObligationKind,
    SettlementWorld, SubmissionId,
};

use super::{ApplicationFuture, RequestHash};

/// 结算履约当前允许由 Worker 推进的持久阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettlementFulfillmentState {
    Pending,
    Prepared,
}

/// 结算义务交给 delivery owner 时使用的操作闭集。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettlementFulfillmentOperation {
    Send,
    Return,
}

impl SettlementFulfillmentOperation {
    /// 返回迁移表使用的稳定操作名。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Send => "send",
            Self::Return => "return",
        }
    }
}

/// Worker 从数据库领取的一条结算履约执行事实。
///
/// 这里携带的是不含 secret 的义务快照。用户收件人使用 `user:<uuid>` owner reference，
/// 由后续 delivery 适配器解析为外部地址；不能在领取阶段把密钥或 Cookie 复制到任务对象。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementFulfillmentClaim {
    pub obligation_id: SettlementId,
    pub auction_id: AuctionId,
    pub lot_id: LotId,
    pub origin_command_id: CommandId,
    pub world: SettlementWorld,
    pub kind: SettlementObligationKind,
    pub state: SettlementFulfillmentState,
    pub operation: SettlementFulfillmentOperation,
    pub recipient: String,
    pub amount: i64,
    pub carrier_credit: u32,
    pub equipment_id: EquipmentId,
    pub submission_id: Option<SubmissionId>,
    pub prerequisite_id: Option<SettlementId>,
    pub delivery_id: Option<Uuid>,
    pub prepare_command_id: Option<CommandId>,
    pub prepare_request_hash: Option<RequestHash>,
    pub claim_token: Uuid,
    pub claim_fencing: i64,
    pub state_version: u64,
    pub worker_id: String,
    pub claimed_at: DateTime<Utc>,
    pub lease_until: DateTime<Utc>,
    pub attempt_count: u32,
}

/// Delivery owner 写入 prepared outbox 所需的稳定身份和 canonical payload。
#[derive(Debug, Clone, PartialEq)]
pub struct SettlementDeliveryPreparation {
    pub delivery_id: Uuid,
    pub prepare_command_id: CommandId,
    pub request_hash: RequestHash,
    pub recipient: String,
    pub payload: Value,
}

/// 履约准备成功后返回的持久身份。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettlementFulfillmentResult {
    pub obligation_id: SettlementId,
    pub delivery_id: Uuid,
    pub prepare_command_id: CommandId,
}

/// 激活 prepared delivery 后返回的持久身份。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettlementFulfillmentActivationResult {
    pub obligation_id: SettlementId,
    pub delivery_id: Uuid,
    pub activate_command_id: CommandId,
}

/// 结算履约调度的窄存储端口。
///
/// PostgreSQL 完整档负责领取、租约 fencing、prepared delivery 的幂等落表和失败退避。SQLite
/// Lite 没有这组完整履约表时必须显式返回能力缺失，不能退化成内存队列。
pub trait SettlementFulfillmentStore: Send + Sync {
    fn claim_due(
        &self,
        worker_id: String,
        now: DateTime<Utc>,
        lease_until: DateTime<Utc>,
        limit: u16,
    ) -> ApplicationFuture<'_, Vec<SettlementFulfillmentClaim>>;

    fn prepare_delivery(
        &self,
        claim: SettlementFulfillmentClaim,
        preparation: SettlementDeliveryPreparation,
        prepared_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, SettlementFulfillmentResult>;

    fn activate_delivery(
        &self,
        claim: SettlementFulfillmentClaim,
        activate_command_id: CommandId,
        activate_request_hash: RequestHash,
        activated_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, SettlementFulfillmentActivationResult>;

    fn fail(
        &self,
        claim: SettlementFulfillmentClaim,
        retry_at: DateTime<Utc>,
        failure_code: String,
        safe_detail: String,
    ) -> ApplicationFuture<'_, ()>;
}

/// 结算履约用例端口；Worker 只负责调度，不自己拼接或写入 delivery payload。
pub trait SettlementFulfillmentCommandPort: Send + Sync {
    fn prepare(
        &self,
        claim: SettlementFulfillmentClaim,
        prepared_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, SettlementFulfillmentResult>;

    fn activate(
        &self,
        claim: SettlementFulfillmentClaim,
        activated_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, SettlementFulfillmentActivationResult>;
}
