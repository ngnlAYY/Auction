//! 缺账户时显式初始化仅账目策略；不要求专用账号声明、个人余额或人为P/C预算。

use serde::Serialize;
use uuid::Uuid;

use super::{FundingCommitOutcome, FundingError, FundingFuture, FundingReconciliationResult};
use crate::diagnostics::{DiagnosticContext, DiagnosticError, DiagnosticLevel, record_error};

/// 命令绑定只读责任预览；旧strict账户不接受隐式转换。
#[derive(Debug, Clone, Serialize)]
pub struct FundingRecordOnlyCommand {
    pub command_id: Uuid,
    pub actor: String,
    pub expected_uid: String,
    pub expected_version: i64,
    pub preview_digest: [u8; 32],
    pub confirm: String,
}

impl FundingRecordOnlyCommand {
    /// 精确确认仅初始化该UID及冻结责任投影，不宣称任何现金实际可用。
    pub fn confirmation(&self) -> String {
        let digest = crate::ports::lower_hex(&self.preview_digest);
        format!(
            "INITIALIZE RECORD ONLY BOT FUNDS {} {} {}",
            self.expected_uid, self.expected_version, digest
        )
    }
    /// 只支持不存在的账户；存在性与预览CAS由最终事务核验。
    pub fn validate(&self) -> Result<(), FundingError> {
        auction_domain::FundingAccountKey::new(&self.expected_uid)
            .map_err(|_| FundingError::Invalid)?;
        if self.command_id.is_nil()
            || self.actor.is_empty()
            || self.actor.len() > 128
            || self.actor.trim() != self.actor
            || self.actor.chars().any(char::is_control)
            || self.expected_version != 0
            || self.preview_digest == [0; 32]
            || self.confirm != self.confirmation()
        {
            return Err(FundingError::Invalid);
        }
        Ok(())
    }
    /// 初始化仍持有真实联合身份、干净compose和同一opaque guard，明确不请求余额。
    pub fn evidence_request(
        &self,
        deadline: std::time::Instant,
    ) -> Result<crate::InternalEvidenceRequest, FundingError> {
        self.validate()?;
        Ok(crate::InternalEvidenceRequest {
            operation_id: self.command_id,
            purpose: crate::InternalEvidencePurpose::FundingRecordOnlyInitialization {
                command_id: self.command_id,
                expected_account_version: 0,
            },
            world: auction_domain::SettlementWorld::Main,
            equipment: None,
            observe_main_cash: false,
            deadline,
        })
    }
    /// 观测时间不参与稳定授权摘要；重试必须保留原命令和预览。
    pub fn request_digest(&self) -> Result<[u8; 32], FundingError> {
        crate::RequestHash::from_request("funding_record_only_initialize_v1", self.command_id, self)
            .map(|hash| hash.as_bytes())
            .map_err(|error| {
                record_error(
                    DiagnosticLevel::Error,
                    error.diagnostic_kind(),
                    "funding_record_only",
                    "hash_request",
                    DiagnosticContext::default(),
                );
                FundingError::Invalid
            })
    }
}

/// 重放先于外部认证；Err保留未知事务guard，确定提交/回滚沿原闭集返回。
pub trait FundingRecordOnlyStore: Send + Sync {
    fn replay_record_only(
        &self,
        command: FundingRecordOnlyCommand,
    ) -> FundingFuture<'_, Option<FundingReconciliationResult>>;
    fn initialize_record_only<'a>(
        &'a self,
        command: &'a FundingRecordOnlyCommand,
        evidence: &'a crate::SettlementInternalEvidenceSnapshot,
    ) -> FundingFuture<'a, FundingCommitOutcome>;
}
