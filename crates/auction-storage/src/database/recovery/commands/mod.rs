//! 恢复命令的 PostgreSQL 装配；应用规则、幂等、追加事实与审计在同一事务内完成。

use auction_application::core::ApplicationError;
use auction_application::core::ApplicationFuture;
use auction_application::recovery::{
    RecoveryCommandRepository, RecoveryConfirmationCommand, RecoveryConfirmationResult,
    RecoveryResolutionAdvance, RecoveryResolutionStore,
};
use auction_time::TimeDelta;

mod binding;
pub(crate) use binding::send_attribution_blocker;
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
mod superseded;

use self::cod_return::advance_cod_return_postgres;
use self::confirm::{advance_relist_postgres, complete_refund_postgres, confirm_postgres};
use crate::database::DatabaseStore;

impl RecoveryResolutionStore for DatabaseStore {
    fn advance_due_relist(
        &self,
        now: auction_time::Timestamp,
        retry_after: TimeDelta,
    ) -> ApplicationFuture<'_, Option<RecoveryResolutionAdvance>> {
        Box::pin(async move {
            if now.timestamp_nanos_opt().is_none()
                || retry_after <= TimeDelta::zero()
                || retry_after > TimeDelta::hours(24)
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
        now: auction_time::Timestamp,
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
        now: auction_time::Timestamp,
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
