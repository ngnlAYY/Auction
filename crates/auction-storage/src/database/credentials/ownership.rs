//! 外部 Cookie 的 Worker 进程生命周期所有权。

#[cfg(feature = "postgres")]
use crate::database::storage_error_from;

use std::{
    fmt,
    sync::atomic::{AtomicBool, Ordering},
};

use auction_application::{
    AdminCredentialKind, AdminCredentialRuntimeOwnership, ApplicationFuture,
};
use futures_util::lock::Mutex;
use sqlx::Connection;
use thiserror::Error;

use super::super::DatabaseStore;
use super::common::storage_error;

type OwnershipConnection = Mutex<Option<sqlx::pool::PoolConnection<sqlx::Postgres>>>;

/// 持有一种外部 Cookie 运行时所有权的 PostgreSQL session；guard 销毁必须关闭物理连接，
/// 不能把仍持有 session advisory lock 的连接放回池中。
pub struct CredentialRuntimeOwnership {
    connection: OwnershipConnection,
    valid: AtomicBool,
    credential_kind: AdminCredentialKind,
    mail_binding: Option<auction_application::MailRuntimeBinding>,
}

impl CredentialRuntimeOwnership {
    /// 只记录首次撤销的闭集原因；失效状态不能通过后续成功探测恢复。
    fn invalidate(&self, reason: &'static str) {
        if self.valid.swap(false, Ordering::AcqRel) {
            tracing::error!(
                credential_kind = self.credential_kind.as_str(),
                reason,
                "credential runtime ownership permanently revoked"
            );
        }
        // 连接已 close_on_drop；在飞检查持有连接时会观察 sticky 位并关闭。
        if let Some(mut slot) = self.connection.try_lock() {
            let _ = slot.take();
        }
    }
}

impl AdminCredentialRuntimeOwnership for CredentialRuntimeOwnership {
    fn check(&self) -> ApplicationFuture<'_, ()> {
        // guard 由返回的 future 持有，连首次 poll 前取消也按检查失败处理。
        let guard =
            OwnershipCheckGuard { ownership: self, completed: false, reason: "check_cancelled" };
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
            let mut connection = slot.take().ok_or_else(|| {
                guard.reason = "session_unavailable";
                storage_error("外部 Cookie 所有权会话不可用")
            })?;
            sqlx::query_scalar::<_, i32>("SELECT 1").fetch_one(&mut *connection).await.map_err(
                |error| {
                    guard.reason = "probe_failed";
                    storage_error_from(error, "外部 Cookie 所有权会话检查失败")
                },
            )?;
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

    fn mail_runtime_binding(&self) -> Option<auction_application::MailRuntimeBinding> {
        self.is_valid().then_some(self.mail_binding).flatten()
    }

    fn revoke_internal_use(&self) {
        self.invalidate("internal_use_revoked");
    }
}

/// 失败或取消的检查永久撤销本 guard；正在另一检查中的连接由该检查退出时关闭。
struct OwnershipCheckGuard<'a> {
    ownership: &'a CredentialRuntimeOwnership,
    completed: bool,
    reason: &'static str,
}

impl Drop for OwnershipCheckGuard<'_> {
    fn drop(&mut self) {
        if !self.completed {
            self.ownership.invalidate(self.reason);
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
    /// 使用专用 PostgreSQL session 为本 Worker 预留一种外部 Cookie；
    /// 锁名只含稳定的凭据类型，不含 Cookie、密文或指纹。每种 Cookie 全局最多一条 active 行，
    /// 第二个同类 Worker 必须启动失败，避免远端会话并发写入。
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
                // 显式提交确认后才返回 owner；不确定错误都关闭原持锁 session。内部业务事务锁相同行
                // 直到 commit，新进程须等其完成才可外呼，配置/keyenv Cookie 同样不能绕过此 fence。
                let mail_binding = if kind == AdminCredentialKind::MailCookie {
                    let mut transaction = connection
                        .begin()
                        .await
                        .map_err(|_| CredentialRuntimeOwnershipError::Unavailable)?;
                    let generation = sqlx::query_scalar::<_, i64>(
                        "INSERT INTO public.mail_runtime_generation (scope, generation, activated_at)
                         VALUES ('mail_cookie', 1, clock_timestamp())
                         ON CONFLICT (scope) DO UPDATE
                         SET generation = public.mail_runtime_generation.generation + 1,
                             activated_at = clock_timestamp()
                         RETURNING generation",
                    )
                    .fetch_one(&mut *transaction)
                    .await
                    .map_err(|_| CredentialRuntimeOwnershipError::Unavailable)?;
                    transaction
                        .commit()
                        .await
                        .map_err(|_| CredentialRuntimeOwnershipError::Unavailable)?;
                    Some(auction_application::MailRuntimeBinding { generation })
                } else {
                    None
                };
                Ok(CredentialRuntimeOwnership {
                    connection: Mutex::new(Some(connection)),
                    valid: AtomicBool::new(true),
                    credential_kind: kind,
                    mail_binding,
                })
            }
        }
    }
}
