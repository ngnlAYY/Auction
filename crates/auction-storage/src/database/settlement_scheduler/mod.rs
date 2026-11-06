//! 结算 workflow 的持久调度端口；领取、租约和完成状态由 PostgreSQL 拥有。

use auction_application::auction::ApplicationFuture;
use auction_application::core::ApplicationError;
use auction_application::settlement::{SettlementWorkClaim, SettlementWorkStore};
use chrono::{DateTime, Utc};

use super::DatabaseStore;

#[cfg(feature = "postgres")]
mod claim;
#[cfg(feature = "postgres")]
mod resolution;
#[cfg(feature = "postgres")]
mod wakeup;

impl SettlementWorkStore for DatabaseStore {
    fn claim_due(
        &self,
        worker_id: String,
        now: DateTime<Utc>,
        lease_until: DateTime<Utc>,
        limit: u16,
    ) -> ApplicationFuture<'_, Vec<SettlementWorkClaim>> {
        Box::pin(async move {
            if worker_id.is_empty() || worker_id != worker_id.trim() || limit == 0 {
                return Err(repository_error("结算 Worker 领取参数无效"));
            }
            if lease_until <= now {
                return Err(repository_error("结算 Worker 租约必须晚于当前时间"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    claim::claim_postgres(store.pool(), &worker_id, now, lease_until, limit)
                        .await
                        .map_err(ApplicationError::from)
                }
            }
        })
    }

    fn next_claimable_at(
        &self,
        now: DateTime<Utc>,
    ) -> ApplicationFuture<'_, Option<DateTime<Utc>>> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => wakeup::next_claimable_postgres(store.pool(), now)
                    .await
                    .map_err(ApplicationError::from),
            }
        })
    }

    fn complete(
        &self,
        _claim: SettlementWorkClaim,
        _completed_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    resolution::complete_postgres(store.pool(), &_claim, _completed_at)
                        .await
                        .map_err(ApplicationError::from)
                }
            }
        })
    }

    fn fail(
        &self,
        _claim: SettlementWorkClaim,
        _retry_at: DateTime<Utc>,
        failure_code: String,
        safe_detail: String,
    ) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            let failure_code = bounded_text(failure_code, 128);
            let safe_detail = bounded_text(safe_detail, 512);
            if !valid_settlement_failure_code(&failure_code) || safe_detail.is_empty() {
                return Err(repository_error("结算 Worker 失败记录不能为空"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    resolution::fail_postgres(store.pool(), &_claim, _retry_at, &failure_code)
                        .await
                        .map_err(ApplicationError::from)
                }
            }
        })
    }
}

fn repository_error(message: &str) -> ApplicationError {
    auction_application::core::RepositoryError::Failed(message.to_owned()).into()
}

fn bounded_text(value: String, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

fn valid_settlement_failure_code(value: &str) -> bool {
    matches!(
        value,
        "internal"
            | "persistence"
            | "timeout"
            | "canceled"
            | "invalid_state"
            | "settlement_mismatch"
            | "task_panic_terminal"
    )
}

#[cfg(feature = "postgres")]
fn map_storage_error(error: sqlx::Error) -> auction_application::core::RepositoryError {
    let context = match &error {
        sqlx::Error::Database(database) if database.code().as_deref() == Some("40001") => {
            "PostgreSQL 结算 workflow 发生序列化冲突"
        }
        _ => "PostgreSQL 结算 workflow 持久化失败",
    };
    auction_application::core::RepositoryError::Failed(crate::database::sql_error_context(
        &error, context,
    ))
}

#[cfg(feature = "postgres")]
fn repository_error_owned(message: &str) -> auction_application::core::RepositoryError {
    auction_application::core::RepositoryError::Failed(message.to_owned())
}
