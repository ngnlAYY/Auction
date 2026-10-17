//! 原合法退款的外部资金用途接缝；不产生新退款权利、不替代原prepare的授权。

use super::{
    SettlementFulfillmentService, internal::evidence_request, validation::repository_error,
};
use crate::{
    ApplicationError, SettlementFulfillmentClaim,
    SettlementFulfillmentPreparationOutcome as Outcome, SettlementInternalCommand,
    SettlementInternalCommitOutcome,
};
use chrono::{DateTime, Duration, Utc};

impl SettlementFulfillmentService {
    pub(super) async fn prepare_funded_refund(
        &self,
        claim: SettlementFulfillmentClaim,
        recipient_version: Option<u64>,
        prepared_at: DateTime<Utc>,
        retry_delay: Duration,
        deadline: std::time::Instant,
    ) -> Result<Outcome, ApplicationError> {
        let Some((store, reader)) = &self.internal else {
            return self.defer_internal(claim, retry_delay, "internal_unsupported").await;
        };
        let command = SettlementInternalCommand::external_refund(claim.clone(), recipient_version)?;
        if store.replay_internal(command.clone()).await? {
            return self
                .prepare_external(claim, prepared_at, retry_delay)
                .await
                .map(Box::new)
                .map(Outcome::External);
        }
        let mut request = evidence_request(&command, self.clock.now(), true, true)?;
        request.deadline = request.deadline.min(deadline);
        let mut prepared = match reader.prepare_internal_evidence(request).await {
            Ok(prepared) => prepared,
            Err(_) => {
                return self
                    .defer_internal(claim, retry_delay, "refund_funding_evidence_wait")
                    .await;
            }
        };
        if prepared.check_current().await.is_err() {
            return self.defer_internal(claim, retry_delay, "refund_funding_evidence_wait").await;
        }
        let guard = prepared.begin_commit().map_err(|_| repository_error("退款证据预算失效"))?;
        let committed = match store.prepare_external_refund(&command, guard.snapshot()).await {
            Ok(SettlementInternalCommitOutcome::Committed) => {
                guard.confirm_committed();
                true
            }
            Ok(SettlementInternalCommitOutcome::FundingDeferred) => {
                guard.confirm_committed();
                false
            }
            Ok(SettlementInternalCommitOutcome::RolledBack(_)) => {
                guard.confirm_rolled_back();
                false
            }
            Err(_) => return Err(repository_error("退款用途提交结果未知，须查询原稳定命令")),
        };
        // 下一阶段旧余额/custody读取会取得原非重入guard；先确定事务并释放本次组合证据。
        drop(prepared);
        if committed {
            self.prepare_external(claim, prepared_at, retry_delay)
                .await
                .map(Box::new)
                .map(Outcome::External)
        } else {
            self.defer_internal(claim, retry_delay, "refund_funding_wait").await
        }
    }
}
