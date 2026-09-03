//! 外部 Cookie 的 Worker 进程生命周期所有权。

use std::{fmt, sync::Mutex};

use auction_application::{AdminCredentialKind, AdminCredentialRuntimeOwnership};
use thiserror::Error;

use super::super::DatabaseStore;

/// 持有一种外部 Cookie 运行时所有权的 PostgreSQL session。
///
/// guard 销毁时必须关闭物理连接，不能把仍持有 session advisory lock 的连接放回池中。
pub struct CredentialRuntimeOwnership {
    connection: Mutex<Option<sqlx::pool::PoolConnection<sqlx::Postgres>>>,
}

impl AdminCredentialRuntimeOwnership for CredentialRuntimeOwnership {}

impl Drop for CredentialRuntimeOwnership {
    fn drop(&mut self) {
        let connection =
            self.connection.get_mut().unwrap_or_else(|poisoned| poisoned.into_inner()).take();
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
                Ok(CredentialRuntimeOwnership { connection: Mutex::new(Some(connection)) })
            }
            #[cfg(feature = "sqlite")]
            DatabaseStore::Sqlite(_) => Err(CredentialRuntimeOwnershipError::Unavailable),
        }
    }
}
