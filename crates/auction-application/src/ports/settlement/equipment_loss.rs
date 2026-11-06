//! 装备损失的显式运维处置；外部补偿声明仅分配人工责任，不伪造收款或发送证明。

use crate::errors::ApplicationError;
use crate::ports::auction::ApplicationFuture;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// 稳定命令绑定人工声明、冻结金额与三个可变 owner 版本。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EquipmentLossCommand {
    pub command_id: Uuid,
    pub lot_id: Uuid,
    pub actor: String,
    /// 主机身份不改变原命令的幂等摘要。
    #[serde(skip)]
    pub operator_identity: Option<crate::operator_audit::CliOperatorIdentity>,
    pub reason: String,
    pub funding_uid: String,
    pub expected_recovery_version: u64,
    pub expected_payment_version: u64,
    pub expected_delivery_fencing_version: i64,
    pub expected_seller_amount: i64,
    pub buyer_compensation_external: bool,
}

impl EquipmentLossCommand {
    /// 输入必须规范；不修剪后改变已经确认的请求。
    pub fn validate(&self) -> Result<(), ApplicationError> {
        auction_domain::FundingAccountKey::new(&self.funding_uid)?;
        if self.command_id.is_nil()
            || self.lot_id.is_nil()
            || !auction_domain::valid_text(&self.actor, 128)
            || !auction_domain::valid_text(&self.reason, 2048)
            || self.expected_recovery_version == 0
            || self.expected_payment_version == 0
            || i64::try_from(self.expected_recovery_version).is_err()
            || i64::try_from(self.expected_payment_version).is_err()
            || self.expected_delivery_fencing_version < 0
            || self.expected_delivery_fencing_version == i64::MAX
            || self.expected_seller_amount <= 0
            || !self.buyer_compensation_external
        {
            return Err(ApplicationError::FulfillmentDependencyMismatch);
        }
        Ok(())
    }

    /// 摘要包含所有操作参数；相同 ID 不允许更换声明、金额或版本。
    pub fn request_digest(&self) -> Result<[u8; 32], ApplicationError> {
        Ok(Sha256::digest(serde_json::to_vec(self)?).into())
    }
}

/// 安全只读投影；不返回原邮件载荷、凭据或外部原文。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EquipmentLossPreview {
    pub eligible: bool,
    pub blockers: Vec<String>,
    pub lot_id: Uuid,
    pub funding_uid: Option<String>,
    pub expected_recovery_version: u64,
    pub expected_payment_version: u64,
    pub expected_delivery_fencing_version: i64,
    pub seller_amount: i64,
    pub buyer_amount: i64,
}

/// 提交结果保留原投递身份；卖家只是释放到正常队列，未宣称已经付款。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EquipmentLossResult {
    pub command_id: Uuid,
    pub lot_id: Uuid,
    pub payment_delivery_id: Uuid,
    pub seller_obligation_id: Uuid,
    pub seller_amount: i64,
    pub replayed: bool,
}

/// 单事务处置端口；原请求重放先于当前业务状态检查。
pub trait SettlementEquipmentLossPort: Send + Sync {
    fn preview_equipment_loss(&self, lot_id: Uuid) -> ApplicationFuture<'_, EquipmentLossPreview>;
    fn resolve_equipment_loss(
        &self,
        command: EquipmentLossCommand,
    ) -> ApplicationFuture<'_, EquipmentLossResult>;
}
