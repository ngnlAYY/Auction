//! 内部满足与真实 COD 资金化的窄事务端口；不把内部成功伪装成 delivery。

use auction_domain::{SettlementId, legacy_natural_key_uuid};
use uuid::Uuid;

use crate::ports::auction::ApplicationFuture;
use crate::ports::core::RequestHash;
use crate::ports::settlement::{
    FundingError, FundingFuture, SettlementFulfillmentClaim, SettlementFulfillmentPhaseResult,
    SettlementFulfillmentProof, SettlementFulfillmentResult, SettlementInternalEvidenceSnapshot,
};

/// 保留旧 prepare 返回值；新版 Worker 只在 External 分支继续原 prepare→activate 链。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettlementFulfillmentPreparationOutcome {
    External(Box<SettlementFulfillmentPhaseResult<SettlementFulfillmentResult>>),
    InternallySatisfied { obligation_id: SettlementId },
}

/// 稳定业务命令不含 claim、generation 或观察时间；这些仍由完整证据单独核验。
#[derive(Debug, Clone)]
pub struct SettlementInternalCommand {
    pub command_id: Uuid,
    pub request_digest: [u8; 32],
    pub claim: SettlementFulfillmentClaim,
    /// 当前 users owner 版本；冻结名称收件人为空。
    pub recipient_version: Option<u64>,
}

impl SettlementInternalCommand {
    /// 原义务与动作唯一命令；重领不换 ID，账号变更由存储账本的 UID 冲突门处理。
    pub fn new(
        claim: SettlementFulfillmentClaim,
        recipient_version: Option<u64>,
        collected: bool,
    ) -> Result<Self, crate::errors::ApplicationError> {
        Self::for_action(
            claim,
            recipient_version,
            if collected { "collected_funding_v1" } else { "internal_fulfillment_v1" },
        )
    }

    /// 只为原恢复门已激活的外部退款预留用途，不满足义务或激活contingent。
    pub fn external_refund(
        claim: SettlementFulfillmentClaim,
        recipient_version: Option<u64>,
    ) -> Result<Self, crate::errors::ApplicationError> {
        Self::for_action(claim, recipient_version, "external_refund_funding_v1")
    }

    fn for_action(
        claim: SettlementFulfillmentClaim,
        recipient_version: Option<u64>,
        action: &str,
    ) -> Result<Self, crate::errors::ApplicationError> {
        let command_id =
            legacy_natural_key_uuid(action, &[claim.obligation_id.as_uuid().as_bytes()]);
        let request_digest = RequestHash::from_request(
            action,
            claim.obligation_id.as_uuid(),
            &serde_json::json!({
                "auction":claim.auction_id,"lot":claim.lot_id,"kind":claim.kind,
                "world":claim.world,"equipment":claim.equipment_id,
                "equipment_world":claim.equipment_world,"external_eid":claim.external_equipment_id,
                "recipient_user":claim.recipient_user_id,"recipient":claim.recipient,
                "amount":claim.amount,"carrier":claim.carrier_credit,
                "prerequisite":claim.prerequisite_id,"delivery":claim.delivery_id,
                "origin_command":claim.origin_command_id,"operation":claim.operation.as_str(),
                "submission":claim.submission_id,"submission_source":claim.submission_source_kind,
                "submission_mail":claim.submission_external_mail_id,"equipment_name":claim.equipment_name,
                "delivery_identity":claim.delivery_snapshot.as_ref().map(|d| serde_json::json!({
                    "id":d.id,"prepare":d.prepare_command_id,"prepare_digest":d.prepare_request_hash.as_bytes(),
                    "activate":d.activate_command_id,"activate_digest":d.activate_request_hash.map(|h| h.as_bytes()),
                    "origin_kind":d.origin_kind,"origin_id":d.origin_id,"recipient":d.recipient,
                    "payload":d.payload,"external_reference":d.external_reference
                }))
            }),
        )?
        .as_bytes();
        Ok(Self { command_id, request_digest, claim, recipient_version })
    }
}

/// Err 代表事务结果未知；只有以下闭集结果允许确认 opaque commit guard。
pub enum SettlementInternalCommitOutcome {
    Committed,
    /// 现金门拒绝已确定提交（缺口时冻结，已有在途/未知时保留占用）；义务保持原状。
    FundingDeferred,
    RolledBack(FundingError),
}

/// 不接收管理员拼装证明；调用者必须借用活跃 opaque prepared 的完整 snapshot。
pub trait SettlementInternalFulfillmentStore: Send + Sync {
    /// 经显式修复的旧自寄付款仅可由原证明 UID 内部执行；损坏证明必须返回错误。
    fn legacy_self_reconciliation_uid(
        &self,
        _obligation_id: Uuid,
    ) -> ApplicationFuture<'_, Option<String>> {
        Box::pin(async { Ok(None) })
    }
    /// 只确认原完整负载；已存在但无法精确重建的摘要返回 FulfillmentDependencyMismatch，
    /// 调用方必须持久隔离，不得当作未执行再次资金化。数据库不可用不能伪装成负载冲突。
    fn replay_internal(&self, command: SettlementInternalCommand) -> ApplicationFuture<'_, bool>;

    fn satisfy_internal<'a>(
        &'a self,
        command: &'a SettlementInternalCommand,
        evidence: &'a SettlementInternalEvidenceSnapshot,
    ) -> FundingFuture<'a, SettlementInternalCommitOutcome>;

    fn prepare_external_refund<'a>(
        &'a self,
        command: &'a SettlementInternalCommand,
        evidence: &'a SettlementInternalEvidenceSnapshot,
    ) -> FundingFuture<'a, SettlementInternalCommitOutcome>;

    fn satisfy_collected<'a>(
        &'a self,
        command: &'a SettlementInternalCommand,
        proof: &'a SettlementFulfillmentProof,
        evidence: &'a SettlementInternalEvidenceSnapshot,
    ) -> FundingFuture<'a, SettlementInternalCommitOutcome>;
}
