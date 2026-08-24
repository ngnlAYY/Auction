//! PostgreSQL delivery outbox 的 claim、发送中 fencing 和终态写入适配器。
//!
//! 传输结果只在 claim 的 token/fencing 仍有效时写回。租约过期后的响应不能覆盖新 Worker 的
//! 结果；如果远端结果已经无法确定，调用方必须把记录置为 `manual_reconcile`。

use auction_application::{
    ApplicationError, DeliverySendResult, DeliveryTransportError, DeliveryWorkClaim,
    DeliveryWorkStore,
};
use chrono::{DateTime, Utc};

#[cfg(feature = "postgres")]
use auction_application::RequestHash;
#[cfg(feature = "postgres")]
use uuid::Uuid;

use super::DatabaseStore;

#[cfg(feature = "postgres")]
mod claims;
mod mapping;
#[cfg(feature = "postgres")]
mod outcome;
#[cfg(feature = "postgres")]
mod state;

#[cfg(feature = "postgres")]
use super::delivery_audit::{
    CLAIM, DeliveryAudit, FAIL, OUTCOME_UNKNOWN, RECONCILE_EXPIRED_SENDING, RELEASE, START_SENDING,
    SUCCEED, append as append_delivery_audit, worker_state_metadata,
};
#[cfg(feature = "postgres")]
use claims::*;
use mapping::*;
#[cfg(feature = "postgres")]
use outcome::*;
#[cfg(feature = "postgres")]
use state::*;

#[cfg(feature = "postgres")]
use sqlx::{Postgres, Row, postgres::PgRow};

#[cfg(feature = "postgres")]
const EXPIRED_SENDING_RECONCILE_LIMIT: i64 = 100;

impl DeliveryWorkStore for DatabaseStore {
    fn claim_due(
        &self,
        worker_id: String,
        now: DateTime<Utc>,
        lease_until: DateTime<Utc>,
        sending_grace: std::time::Duration,
        limit: u16,
    ) -> auction_application::ApplicationFuture<'_, Vec<DeliveryWorkClaim>> {
        Box::pin(async move {
            if worker_id.is_empty()
                || worker_id != worker_id.trim()
                || limit == 0
                || lease_until <= now
                || sending_grace.is_zero()
                || sending_grace > std::time::Duration::from_secs(86_400)
            {
                return Err(repository_error("delivery 领取参数无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    claim_postgres(store.pool(), &worker_id, now, lease_until, sending_grace, limit)
                        .await
                        .map_err(ApplicationError::from)
                }
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(ApplicationError::CapabilityUnavailable(
                    "delivery outbox 仅支持 PostgreSQL 完整档",
                )),
            }
        })
    }

    fn start_sending(
        &self,
        _claim: DeliveryWorkClaim,
        _started_at: DateTime<Utc>,
    ) -> auction_application::ApplicationFuture<'_, ()> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => start_sending_postgres(store.pool(), &_claim, _started_at)
                    .await
                    .map_err(ApplicationError::from),
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(ApplicationError::CapabilityUnavailable(
                    "delivery outbox 仅支持 PostgreSQL 完整档",
                )),
            }
        })
    }

    fn release(
        &self,
        _claim: DeliveryWorkClaim,
        _released_at: DateTime<Utc>,
    ) -> auction_application::ApplicationFuture<'_, ()> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => release_postgres(store.pool(), &_claim, _released_at)
                    .await
                    .map_err(ApplicationError::from),
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(ApplicationError::CapabilityUnavailable(
                    "delivery outbox 仅支持 PostgreSQL 完整档",
                )),
            }
        })
    }

    fn renew(
        &self,
        _claim: DeliveryWorkClaim,
        _renewed_at: DateTime<Utc>,
        _lease_until: DateTime<Utc>,
    ) -> auction_application::ApplicationFuture<'_, ()> {
        Box::pin(async move {
            if _lease_until <= _renewed_at {
                return Err(repository_error("delivery 续租时间无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    renew_postgres(store.pool(), &_claim, _renewed_at, _lease_until)
                        .await
                        .map_err(ApplicationError::from)
                }
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(ApplicationError::CapabilityUnavailable(
                    "delivery outbox 仅支持 PostgreSQL 完整档",
                )),
            }
        })
    }

    fn succeed(
        &self,
        _claim: DeliveryWorkClaim,
        result: DeliverySendResult,
        _completed_at: DateTime<Utc>,
    ) -> auction_application::ApplicationFuture<'_, ()> {
        Box::pin(async move {
            if result.external_reference.is_empty()
                || result.external_reference != result.external_reference.trim()
                || !result.result_metadata.is_object()
            {
                return Err(repository_error("delivery 成功结果无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    succeed_postgres(store.pool(), &_claim, &result, _completed_at)
                        .await
                        .map_err(ApplicationError::from)
                }
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(ApplicationError::CapabilityUnavailable(
                    "delivery outbox 仅支持 PostgreSQL 完整档",
                )),
            }
        })
    }

    fn fail(
        &self,
        _claim: DeliveryWorkClaim,
        error: DeliveryTransportError,
        _completed_at: DateTime<Utc>,
    ) -> auction_application::ApplicationFuture<'_, ()> {
        Box::pin(async move {
            let failure_code = bounded_text(error.failure_code, 64);
            let safe_detail = bounded_text(error.safe_detail, 512);
            let external_reference = error.external_reference.map(|value| bounded_text(value, 512));
            let mut result_metadata = error.result_metadata;
            if failure_code.is_empty()
                || failure_code != failure_code.trim()
                || safe_detail.is_empty()
                || safe_detail != safe_detail.trim()
                || external_reference.as_ref().is_some_and(|value| {
                    value.is_empty() || value != value.trim() || value.chars().any(char::is_control)
                })
                || !result_metadata.is_object()
            {
                return Err(repository_error("delivery 失败结果无效"));
            }
            let Some(metadata) = result_metadata.as_object_mut() else {
                return Err(repository_error("delivery 失败 metadata 必须是 object"));
            };
            metadata.insert("safe_detail".to_owned(), safe_detail.clone().into());
            metadata.insert("outcome_unknown".to_owned(), error.outcome_unknown.into());
            if serde_json::to_vec(&result_metadata)
                .map_err(|_| repository_error("delivery 失败 metadata 无法序列化"))?
                .len()
                > 16 * 1024
            {
                return Err(repository_error("delivery 失败 metadata 超出边界"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => fail_postgres(
                    store.pool(),
                    &_claim,
                    &failure_code,
                    error.outcome_unknown,
                    external_reference.as_deref(),
                    &result_metadata,
                    _completed_at,
                )
                .await
                .map_err(ApplicationError::from),
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(ApplicationError::CapabilityUnavailable(
                    "delivery outbox 仅支持 PostgreSQL 完整档",
                )),
            }
        })
    }
}
