//! 旧 self-send 的显式核验；只恢复已证明在本账号库存中的原义务，不生成外部成功。

use std::time::Instant;

use auction_domain::{EquipmentId, SettlementWorld};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    FundingError, FundingFuture, InternalEquipmentTarget, InternalEvidencePurpose,
    InternalEvidenceRequest, RequestHash, SettlementInternalEvidenceSnapshot,
};

/// 流拍装备留存与明确自寄拒绝后的付款恢复互不混用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LegacySelfReconciliationAction {
    KeepUnsold,
    ReopenPayment,
}

impl LegacySelfReconciliationAction {
    /// 追加命令事实中的稳定动作名称。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::KeepUnsold => "keep_unsold",
            Self::ReopenPayment => "reopen_payment",
        }
    }
}

/// 无秘密的完整数据库预览；摘要冻结原义务、投递、履约、装备和用户版本。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegacySelfReconciliationTarget {
    pub obligation_id: Uuid,
    pub delivery_id: Uuid,
    pub action: LegacySelfReconciliationAction,
    pub world: SettlementWorld,
    pub equipment_id: EquipmentId,
    pub external_equipment_id: String,
    pub recipient: String,
    pub expected_delivery_fencing: i64,
    pub expected_fulfillment_version: i64,
    pub expected_obligation_version: i64,
    pub recipient_version: Option<i64>,
    pub snapshot_digest: [u8; 32],
}

impl LegacySelfReconciliationTarget {
    /// 供装配根选择原世界网关，不引入领域 crate 的额外依赖。
    pub const fn world_name(&self) -> &'static str {
        self.world.as_str()
    }
}

/// 同 ID/同完整负载重放原结果；观察时间、会话 generation 不进入请求摘要。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LegacySelfReconciliationCommand {
    pub command_id: Uuid,
    pub actor: String,
    pub expected_uid: String,
    pub reason: String,
    pub target: LegacySelfReconciliationTarget,
}

impl LegacySelfReconciliationCommand {
    /// 验证调用身份、版本和目标形状；数据库仍会重新锁定并核对完整预览。
    pub fn validate(&self) -> Result<(), FundingError> {
        let target = &self.target;
        let valid_text = |s: &str, max| {
            !s.is_empty() && s.len() <= max && s == s.trim() && !s.chars().any(char::is_control)
        };
        if self.command_id.is_nil()
            || !valid_text(&self.actor, 128)
            || !valid_text(&self.reason, 512)
            || !crate::valid_external_positive_id(&self.expected_uid)
            || target.obligation_id.is_nil()
            || target.delivery_id.is_nil()
            || target.equipment_id.is_nil()
            || !crate::valid_external_positive_id(&target.external_equipment_id)
            || !crate::valid_mail_player_name(&target.recipient)
            || target.expected_delivery_fencing < 0
            || target.expected_fulfillment_version <= 0
            || target.expected_obligation_version <= 0
            || target.recipient_version.is_some_and(|version| version <= 0)
            || target.snapshot_digest == [0; 32]
        {
            return Err(FundingError::Invalid);
        }
        Ok(())
    }

    /// 绑定完整显式命令，禁止同命令 ID 更换目标、理由或操作者。
    pub fn request_digest(&self) -> Result<[u8; 32], FundingError> {
        self.validate()?;
        RequestHash::from_request("legacy_self_reconciliation_v1", self.command_id, self)
            .map(|hash| hash.as_bytes())
            .map_err(|_| FundingError::Invalid)
    }

    /// 精确人工确认同时绑定义务、UID、原版本和原快照摘要。
    pub fn confirmation(&self) -> String {
        let digest = crate::ports::lower_hex(&self.target.snapshot_digest);
        format!(
            "REPAIR LEGACY SELF SEND {} {} {} {}",
            self.target.obligation_id,
            self.expected_uid,
            self.target.expected_fulfillment_version,
            digest
        )
    }

    /// 必须通过原网关取得新鲜身份、干净 compose 和精确装备正向库存；不执行远端写入。
    pub fn evidence_request(&self, deadline: Instant) -> InternalEvidenceRequest {
        InternalEvidenceRequest {
            operation_id: self.command_id,
            purpose: InternalEvidencePurpose::LegacySelfReconciliation {
                command_id: self.command_id,
                obligation_id: self.target.obligation_id,
                expected_fulfillment_version: self.target.expected_fulfillment_version,
            },
            world: self.target.world,
            equipment: Some(InternalEquipmentTarget {
                equipment_id: self.target.equipment_id,
                external_equipment_id: self.target.external_equipment_id.clone(),
                world: self.target.world,
            }),
            observe_main_cash: false,
            deadline,
        }
    }
}

/// 恢复付款不表示付款已完成。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LegacySelfReconciliationResult {
    pub command_id: Uuid,
    pub delivery_id: Uuid,
    pub obligation_id: Uuid,
    pub action: LegacySelfReconciliationAction,
    pub fulfillment_state: String,
    pub fulfillment_version: i64,
}

/// 只有确定提交/回滚可以释放 opaque guard；普通错误表示结果未知。
pub enum LegacySelfReconciliationOutcome {
    Committed(LegacySelfReconciliationResult),
    RolledBack(FundingError),
}

/// 预览与原命令查询不取得远端 owner，也不改变领取/状态。
pub trait LegacySelfReconciliationReader: Send + Sync {
    /// 已提交命令恢复原目标，使当前投影变化后仍可按原 ID 重放。
    fn legacy_self_command_target(
        &self,
        command_id: Uuid,
    ) -> FundingFuture<'_, Option<LegacySelfReconciliationTarget>>;
    /// 从同一只读快照读取当前可修复目标。
    fn preview_legacy_self(
        &self,
        obligation_id: Uuid,
    ) -> FundingFuture<'_, LegacySelfReconciliationTarget>;
    /// 完整负载匹配后返回原结果；不要求当前会话或目标仍为 manual。
    fn replay_legacy_self(
        &self,
        command: LegacySelfReconciliationCommand,
    ) -> FundingFuture<'_, Option<LegacySelfReconciliationResult>>;
}

/// 调用方持有 opaque commit guard 至确定提交/回滚；Err 按原 command_id 查询，不换 ID。
pub trait LegacySelfReconciliationWriter: Send + Sync {
    /// 最多三次整事务重试；证据、业务变更、稳定结果及追加审计同事务提交。
    fn reconcile_legacy_self<'a>(
        &'a self,
        command: &'a LegacySelfReconciliationCommand,
        evidence: &'a SettlementInternalEvidenceSnapshot,
    ) -> FundingFuture<'a, LegacySelfReconciliationOutcome>;
}
