//! 外部 Cookie 的 Worker 进程生命周期所有权。

use std::{
    fmt,
    sync::atomic::{AtomicBool, Ordering},
};

use auction_application::{
    AdminCredentialKind, AdminCredentialRuntimeOwnership, ApplicationFuture,
};
use futures_util::lock::Mutex;
use thiserror::Error;

use super::super::DatabaseStore;
use super::common::storage_error;

type OwnershipConnection = Mutex<Option<sqlx::pool::PoolConnection<sqlx::Postgres>>>;

/// 持有一种外部 Cookie 运行时所有权的 PostgreSQL session。
///
/// guard 销毁时必须关闭物理连接，不能把仍持有 session advisory lock 的连接放回池中。
pub struct CredentialRuntimeOwnership {
    connection: OwnershipConnection,
    valid: AtomicBool,
}

impl AdminCredentialRuntimeOwnership for CredentialRuntimeOwnership {
    fn check(&self) -> ApplicationFuture<'_, ()> {
        // guard 由返回的 future 持有，连首次 poll 前取消也按检查失败处理。
        let guard = OwnershipCheckGuard { ownership: self, completed: false };
        Box::pin(async move {
            let mut guard = guard;
            if !self.is_valid() {
                return Err(storage_error("外部 Cookie 运行时所有权已失效"));
            }
            let mut slot = self.connection.lock().await;
            if !self.is_valid() {
                return Err(storage_error("外部 Cookie 运行时所有权已失效"));
            }
            // 原连接由本 future 独占；调用方超时取消会关闭它，不把半途检查的连接放回 slot。
            let mut connection =
                slot.take().ok_or_else(|| storage_error("外部 Cookie 所有权会话不可用"))?;
            sqlx::query_scalar::<_, i32>("SELECT 1")
                .fetch_one(&mut *connection)
                .await
                .map_err(|_| storage_error("外部 Cookie 所有权会话检查失败"))?;
            // 并发等待检查可能已经取消或超时；不能由此次成功探测恢复 sticky 失效状态。
            if !self.is_valid() {
                return Err(storage_error("外部 Cookie 运行时所有权已失效"));
            }
            *slot = Some(connection);
            drop(slot);
            // slot 锁已经释放，再次观察并发取消；若它发生在恢复 slot 的瞬间，本守卫负责关闭。
            if !self.is_valid() {
                return Err(storage_error("外部 Cookie 运行时所有权已失效"));
            }
            guard.completed = true;
            Ok(())
        })
    }

    fn is_valid(&self) -> bool {
        self.valid.load(Ordering::Acquire)
    }
}

/// 失败或取消的检查永久撤销本 guard；正在另一检查中的连接由该检查退出时关闭。
struct OwnershipCheckGuard<'a> {
    ownership: &'a CredentialRuntimeOwnership,
    completed: bool,
}

impl Drop for OwnershipCheckGuard<'_> {
    fn drop(&mut self) {
        if !self.completed {
            self.ownership.valid.store(false, Ordering::Release);
            // 取消等待者时连接可能在闲置 slot，也可能已被在飞检查 take；不阻塞析构。
            if let Some(mut slot) = self.ownership.connection.try_lock() {
                let _ = slot.take();
            }
        }
    }
}

impl Drop for CredentialRuntimeOwnership {
    fn drop(&mut self) {
        let connection = self.connection.get_mut().take();
        if let Some(mut connection) = connection {
            connection.close_on_drop();
        }
    }
}

impl fmt::Debug for CredentialRuntimeOwnership {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("CredentialRuntimeOwnership").finish_non_exhaustive()
    }
}

/// 外部 Cookie 运行时所有权获取失败。
#[derive(Debug, Error)]
pub enum CredentialRuntimeOwnershipError {
    #[error("外部 Cookie 运行时所有权已被另一个 Worker 占用")]
    AlreadyOwned,
    #[error("外部 Cookie 运行时所有权不可用")]
    Unavailable,
}

impl DatabaseStore {
    /// 使用一个专用 PostgreSQL session 为本 Worker 预留一种外部 Cookie。
    ///
    /// 锁名只包含稳定的凭据类型，不包含 Cookie、密文或指纹。由于每种外部 Cookie 全局最多
    /// 一个 active 行，同类集成的第二个 Worker 必须启动失败，避免远端会话并发写入。
    pub async fn acquire_credential_runtime_ownership(
        &self,
        kind: AdminCredentialKind,
    ) -> Result<CredentialRuntimeOwnership, CredentialRuntimeOwnershipError> {
        match self {
            DatabaseStore::Postgres(store) => {
                let mut connection = store
                    .pool()
                    .acquire()
                    .await
                    .map_err(|_| CredentialRuntimeOwnershipError::Unavailable)?;
                // 后续任一路径提前返回时都关闭 session，以释放 advisory lock。
                connection.close_on_drop();
                let lock_name = format!("auction/credential-runtime-owner/v1/{}", kind.as_str());
                let acquired = sqlx::query_scalar::<_, bool>(
                    "SELECT pg_try_advisory_lock(hashtextextended($1, 0))",
                )
                .bind(lock_name)
                .fetch_one(&mut *connection)
                .await
                .map_err(|_| CredentialRuntimeOwnershipError::Unavailable)?;
                if !acquired {
                    return Err(CredentialRuntimeOwnershipError::AlreadyOwned);
                }
                Ok(CredentialRuntimeOwnership {
                    connection: Mutex::new(Some(connection)),
                    valid: AtomicBool::new(true),
                })
            }
        }
    }
}
