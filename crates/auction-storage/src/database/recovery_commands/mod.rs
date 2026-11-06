//! 恢复命令的 PostgreSQL 装配；应用规则、幂等、追加事实与审计在同一事务内完成。

use auction_application::auction::ApplicationFuture;
use auction_application::core::ApplicationError;
use auction_application::recovery::{
    RecoveryCommandRepository, RecoveryConfirmationCommand, RecoveryConfirmationResult,
    RecoveryResolutionAdvance, RecoveryResolutionStore,
};
use chrono::Duration as ChronoDuration;

#[cfg(feature = "postgres")]
mod cod_return;
mod confirm;
#[cfg(feature = "postgres")]
mod delivery_retry;
#[cfg(feature = "postgres")]
pub(crate) use delivery_retry::{
    lock_recovery_delivery_retry_actions, lock_recovery_delivery_retry_scope,
    reset_after_delivery_retry, resume_after_delivery_proof,
};
mod mapping;
mod relist;
mod scope;

#[cfg(feature = "postgres")]
use cod_return::*;
#[cfg(feature = "postgres")]
use confirm::*;
#[cfg(feature = "postgres")]
use mapping::*;
#[cfg(feature = "postgres")]
use relist::*;
#[cfg(feature = "postgres")]
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
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => advance_relist_postgres(store.pool(), now, retry_after)
                    .await
                    .map_err(ApplicationError::from),
            }
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
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => advance_cod_return_postgres(store.pool(), now)
                    .await
                    .map_err(ApplicationError::from),
            }
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
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => complete_refund_postgres(store.pool(), now)
                    .await
                    .map_err(ApplicationError::from),
            }
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

            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    confirm_postgres(store.pool(), command).await.map_err(ApplicationError::from)
                }
            }
        })
    }
}
