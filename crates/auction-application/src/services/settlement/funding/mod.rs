//! 资金对账用例：默认只读，显式授权后才取得真实证据和提交guard。

use crate::{
    FundingCashGateProvider, FundingCommitOutcome, FundingError, FundingPreview,
    FundingReconciliationCommand, FundingReconciliationReader, FundingReconciliationResult,
    FundingReconciliationWriter, InternalEvidencePurpose, InternalEvidenceRequest,
    SettlementInternalEvidenceReader,
};
use auction_domain::SettlementWorld;
use std::{sync::Arc, time::Instant};

/// 装配根注入同一个真实evidence reader与完整发送门；不读取配置或调用SQL。
pub struct FundingReconciliationService {
    reader: Arc<dyn FundingReconciliationReader>,
    writer: Arc<dyn FundingReconciliationWriter>,
    evidence: Arc<dyn SettlementInternalEvidenceReader>,
    cash_gate: Arc<dyn FundingCashGateProvider>,
}

impl FundingReconciliationService {
    pub fn new(
        reader: Arc<dyn FundingReconciliationReader>,
        writer: Arc<dyn FundingReconciliationWriter>,
        evidence: Arc<dyn SettlementInternalEvidenceReader>,
        cash_gate: Arc<dyn FundingCashGateProvider>,
    ) -> Self {
        Self { reader, writer, evidence, cash_gate }
    }

    /// 只调用数据库READ ONLY，不取得owner、不产生远端现金额度。
    pub async fn preview(&self, expected_uid: String) -> Result<FundingPreview, FundingError> {
        self.reader.preview_funding(expected_uid).await
    }

    /// 相同命令先重放；未知提交不换ID，不释放guard后继续外发。
    pub async fn apply(
        &self,
        command: FundingReconciliationCommand,
        deadline: Instant,
    ) -> Result<FundingReconciliationResult, FundingError> {
        command.validate()?;
        if let Some(result) = self.reader.replay_funding(command.clone()).await? {
            return Ok(result);
        }
        let gate = self.cash_gate.funding_cash_gate().ok_or(FundingError::Unsupported)?;
        let mut prepared = self
            .evidence
            .prepare_internal_evidence(InternalEvidenceRequest {
                operation_id: command.command_id,
                purpose: InternalEvidencePurpose::FundingReconciliation {
                    command_id: command.command_id,
                    expected_account_version: u64::try_from(command.expected_version)
                        .map_err(|_| FundingError::Invalid)?,
                },
                world: SettlementWorld::Main,
                equipment: None,
                observe_main_cash: true,
                deadline,
            })
            .await
            .map_err(|_| FundingError::Unproven)?;
        prepared.check_current().await.map_err(|_| FundingError::Unproven)?;
        let commit = prepared.begin_commit().map_err(|_| FundingError::Unproven)?;
        match self.writer.reconcile_funding(&command, commit.snapshot(), gate).await {
            Ok(FundingCommitOutcome::Committed(result)) => {
                commit.confirm_committed();
                Ok(result)
            }
            Ok(FundingCommitOutcome::RolledBack(error)) => {
                commit.confirm_rolled_back();
                Err(error)
            }
            Err(error) => Err(error), // guard Drop同步撤销owner；原命令是唯一恢复入口。
        }
    }
}
