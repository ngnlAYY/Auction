//! PostgreSQL recovery 确认事务适配器。
//!
//! A14 只允许在锁定 recovery、原拍卖、lot/equipment、相关 obligation 和 action 后执行。命令
//! 账本、退款激活、guarded relist、状态版本、审计以及预览目录事件都在同一 Serializable
//! 事务内完成；SQLite Lite 明确返回能力缺失。

#[cfg(feature = "postgres")]
use std::collections::BTreeMap;

use auction_application::{
    ApplicationError, ApplicationFuture, RecoveryCommandRepository, RecoveryConfirmationCommand,
    RecoveryConfirmationResult, RecoveryResolutionAdvance, RecoveryResolutionStore,
};
#[cfg(feature = "postgres")]
use auction_application::{RecoveryCommandError, RecoveryConfirmationAction};
#[cfg(feature = "postgres")]
use auction_domain::{CommandId, legacy_natural_key_uuid};
use chrono::Duration as ChronoDuration;
#[cfg(feature = "postgres")]
use chrono::{DateTime, Utc};
#[cfg(feature = "postgres")]
use serde_json::{Value, json};
#[cfg(feature = "postgres")]
use sqlx::{Postgres, Row, Transaction};
#[cfg(feature = "postgres")]
use uuid::Uuid;

#[path = "commands/confirm.rs"]
mod confirm;
#[path = "commands/mapping.rs"]
mod mapping;
#[path = "commands/relist.rs"]
mod relist;
#[path = "commands/scope.rs"]
mod scope;

#[cfg(feature = "postgres")]
use confirm::*;
#[cfg(feature = "postgres")]
use mapping::*;
#[cfg(feature = "postgres")]
use relist::*;
#[cfg(feature = "postgres")]
use scope::*;

#[cfg(feature = "postgres")]
use crate::database::audit_health::AuditWriteResult as _;

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
                    auction_application::RepositoryError::Failed(
                        "recovery relist 调度参数无效".to_owned(),
                    ),
                ));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => advance_relist_postgres(store.pool(), now, retry_after)
                    .await
                    .map_err(ApplicationError::from),
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(ApplicationError::CapabilityUnavailable(
                    "recovery relist Worker 仅支持 PostgreSQL 完整档",
                )),
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
                    auction_application::RepositoryError::Failed(
                        "recovery refund 调度时间无效".to_owned(),
                    ),
                ));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => complete_refund_postgres(store.pool(), now)
                    .await
                    .map_err(ApplicationError::from),
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(ApplicationError::CapabilityUnavailable(
                    "recovery refund Worker 仅支持 PostgreSQL 完整档",
                )),
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
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(ApplicationError::CapabilityUnavailable(
                    "recovery 确认仅支持 PostgreSQL 完整档",
                )),
            }
        })
    }
}
