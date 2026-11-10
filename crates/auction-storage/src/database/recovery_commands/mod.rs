//! 恢复命令的 PostgreSQL 装配；应用规则、幂等、追加事实与审计在同一事务内完成。

use auction_application::auction::ApplicationFuture;
use auction_application::core::ApplicationError;
use auction_application::recovery::{
    RecoveryCommandRepository, RecoveryConfirmationCommand, RecoveryConfirmationResult,
    RecoveryResolutionAdvance, RecoveryResolutionStore,
};
use chrono::Duration as ChronoDuration;

mod cod_return;
mod confirm;
mod delivery_retry;
pub(crate) use delivery_retry::{
    lock_recovery_delivery_retry_actions, lock_recovery_delivery_retry_scope,
    reset_after_delivery_retry, resume_after_delivery_proof,
};
mod mapping;
mod relist;
mod scope;

use cod_return::*;
use confirm::*;
use mapping::*;
use relist::*;
use scope::*;

use super::DatabaseStore;

impl RecoveryResolutionStore for DatabaseStore {
    fn advance_due_relist(
        &self,
        now: chrono::DateTime<chrono::Utc>,
        retry_after: ChronoDuration,
    ) -> ApplicationFuture<'_, Option<RecoveryResolutionAdvance>> {
        Box::pin(async move {
            if now.timestamp_nanos_opt().is_none()
                || retry_after <= ChronoDuration::zero()
                || retry_after > ChronoDuration::hours(24)
            {
                return Err(ApplicationError::Repository(
                    auction_application::core::RepositoryError::Failed(
                        "recovery relist 调度参数无效".to_owned(),
                    ),
                ));
            }
            advance_relist_postgres(self.pool(), now, retry_after)
                .await
                .map_err(ApplicationError::from)
        })
    }

    fn advance_due_cod_return(
        &self,
        now: chrono::DateTime<chrono::Utc>,
    ) -> ApplicationFuture<'_, Option<RecoveryResolutionAdvance>> {
        Box::pin(async move {
            if now.timestamp_nanos_opt().is_none() {
                return Err(ApplicationError::Repository(
                    auction_application::core::RepositoryError::Failed(
                        "recovery COD 退回调度时间无效".to_owned(),
                    ),
                ));
            }
            advance_cod_return_postgres(self.pool(), now).await.map_err(ApplicationError::from)
        })
    }

    fn complete_due_refund(
        &self,
        now: chrono::DateTime<chrono::Utc>,
    ) -> ApplicationFuture<'_, Option<RecoveryResolutionAdvance>> {
        Box::pin(async move {
            if now.timestamp_nanos_opt().is_none() {
                return Err(ApplicationError::Repository(
                    auction_application::core::RepositoryError::Failed(
                        "recovery refund 调度时间无效".to_owned(),
                    ),
                ));
            }
            complete_refund_postgres(self.pool(), now).await.map_err(ApplicationError::from)
        })
    }
}

impl RecoveryCommandRepository for DatabaseStore {
    fn confirm_recovery(
        &self,
        command: RecoveryConfirmationCommand,
    ) -> ApplicationFuture<'_, RecoveryConfirmationResult> {
        Box::pin(async move {
            if let Err(error) = command.validate() {
                return Err(error.into());
            }

            confirm_postgres(self.pool(), command).await.map_err(ApplicationError::from)
        })
    }
}
