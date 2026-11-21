//! 解析工作端口及短事务的隔离、重试与提交边界。

use std::time::Duration;

use auction_application::consignment::{
    AutoCloseCandidate, AutoCloseListingCommand, ConsignmentError, ConsignmentFuture,
    ListingAdmission, ListingLookupClaim, ListingMaintenance, ListingMaintenancePage,
    ListingWorkerReadiness, LookupAbandonment, LookupClaimRequest, LookupDispatch, LookupFence,
    LookupLeaseUpdate, LookupPermitStatus, LookupWorkStore,
};
use auction_application::core::{RequestHash, TransactionPolicy};
use auction_domain::CommandId;
use auction_time::Timestamp;
use futures_util::future::BoxFuture;
use sqlx::PgConnection;
use tokio::time::error::Elapsed;

use super::{admission, claim, lease, reclaim};
use crate::database::DatabaseStore;
use crate::database::backend::{retry_conflicts, set_read_only_limits};
use crate::database::consignment::commands::errors::WriteError;

/// 用应用层提供的规范命令构造器为存储回收准备终结身份。
pub(super) struct ReapCommands {
    pub(super) deadline: fn(AutoCloseCandidate) -> AutoCloseListingCommand,
    pub(super) exhausted: fn(LookupFence) -> (CommandId, RequestHash),
}

struct ConsignmentLookupStore {
    database: DatabaseStore,
    commands: ReapCommands,
}

impl DatabaseStore {
    /// 装配解析工作端口，回收的两个规范命令构造器由应用层提供。
    pub fn consignment_lookup(
        &self,
        deadline: fn(AutoCloseCandidate) -> AutoCloseListingCommand,
        exhausted: fn(LookupFence) -> (CommandId, RequestHash),
    ) -> impl LookupWorkStore + use<> {
        ConsignmentLookupStore {
            database: self.clone(),
            commands: ReapCommands { deadline, exhausted },
        }
    }
}

impl LookupWorkStore for ConsignmentLookupStore {
    fn claim(
        &self,
        request: LookupClaimRequest,
    ) -> ConsignmentFuture<'_, Option<ListingLookupClaim>> {
        Box::pin(async move {
            transact(&self.database, &request, |conn, request| {
                Box::pin(claim::claim(conn, request))
            })
            .await
        })
    }

    fn renew(&self, fence: LookupFence) -> ConsignmentFuture<'_, LookupLeaseUpdate<Timestamp>> {
        Box::pin(async move {
            transact(&self.database, &fence, |conn, &fence| Box::pin(lease::renew(conn, fence)))
                .await
        })
    }

    fn confirm_dispatch<'a>(
        &'a self,
        claim: &'a ListingLookupClaim,
        paused: bool,
    ) -> ConsignmentFuture<'a, LookupDispatch> {
        Box::pin(async move {
            transact(&self.database, &(claim, paused), |conn, &(claim, paused)| {
                Box::pin(lease::confirm_dispatch(conn, claim, paused))
            })
            .await
        })
    }

    fn abandon(
        &self,
        fence: LookupFence,
        status: LookupAbandonment,
    ) -> ConsignmentFuture<'_, LookupLeaseUpdate<()>> {
        Box::pin(async move {
            reclaim::abandon(&self.database, &self.commands, fence, status)
                .await
                .map_err(WriteError::into_application)
        })
    }

    fn reap_expired(&self) -> ConsignmentFuture<'_, u32> {
        Box::pin(async move {
            reclaim::reap_expired(&self.database, &self.commands)
                .await
                .map_err(WriteError::into_application)
        })
    }

    fn permit_status(&self) -> ConsignmentFuture<'_, LookupPermitStatus> {
        Box::pin(async move {
            admission::permit_status(&self.database).await.map_err(WriteError::into_application)
        })
    }

    fn record_worker_readiness(
        &self,
        service_epoch: u64,
        readiness: ListingWorkerReadiness,
    ) -> ConsignmentFuture<'_, ()> {
        Box::pin(async move {
            if readiness == ListingWorkerReadiness::Unavailable {
                return Ok(());
            }
            let service_epoch = i64::try_from(service_epoch).map_err(|error| {
                WriteError::from(sqlx::Error::Decode(Box::new(error))).into_application()
            })?;
            transact(&self.database, &service_epoch, |conn, &epoch| {
                Box::pin(admission::record_ready(conn, epoch))
            })
            .await
        })
    }
}

impl ListingMaintenance for DatabaseStore {
    fn scan_candidates(&self) -> ConsignmentFuture<'_, ListingMaintenancePage> {
        // 2R 接入公平补扫前，失败关闭，避免把缺少扫描能力报告成没有存量。
        Box::pin(async { Err(ConsignmentError::Unavailable) })
    }

    fn sync_admission(&self, enabled: bool) -> ConsignmentFuture<'_, ListingAdmission> {
        Box::pin(async move {
            transact(self, &enabled, |conn, &enabled| Box::pin(admission::sync(conn, enabled)))
                .await
        })
    }
}

/// 按单例与工作行锁串行化短操作。明确中止才重试，未知提交不得授予执行权。
pub(super) async fn transact<T, I>(
    store: &DatabaseStore,
    input: &I,
    operation: impl for<'a> Fn(&'a mut PgConnection, &'a I) -> BoxFuture<'a, Result<T, WriteError>>,
) -> Result<T, ConsignmentError> {
    retry_conflicts(
        store.pool(),
        store.transaction_owner,
        usize::from(TransactionPolicy::serializable().max_attempts),
        |error| matches!(error, WriteError::RetryConflict),
        || async {
            // 单例和暂停名称锁取得后，下一条语句需要看见持有者最新提交的事实。
            let mut tx = store.pool().begin_with("BEGIN ISOLATION LEVEL READ COMMITTED").await?;
            set_read_only_limits(&mut tx)
                .await
                .map_err(|(_setting, error)| WriteError::from(error))?;
            sqlx::query("SET LOCAL transaction_timeout = '10s'").execute(&mut *tx).await?;
            let result = operation(&mut tx, input).await?;
            // 不变量 4、5：无法确认提交就不返回许可，也不自动重新领取。
            tokio::time::timeout(Duration::from_secs(5), tx.commit())
                .await
                .map_err(|Elapsed { .. }| ConsignmentError::OutcomeUnknown)?
                .map_err(|error| {
                    if error.as_database_error().is_none()
                        || error.as_database_error().is_some_and(|db| {
                            db.code().is_some_and(|code| {
                                code.starts_with("08")
                                    || matches!(
                                        code.as_ref(),
                                        "40003" | "57P01" | "57P02" | "57P05"
                                    )
                            })
                        })
                    {
                        ConsignmentError::OutcomeUnknown.into()
                    } else {
                        WriteError::from(error)
                    }
                })?;
            Ok(result)
        },
    )
    .await
    .map_err(WriteError::into_application)
}
