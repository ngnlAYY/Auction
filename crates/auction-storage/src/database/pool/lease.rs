//! 应用连接租用与事务持有守卫；仅包装所有权，不改变 SQLx 提交、回滚或取消行为。

use std::ops::{Deref, DerefMut};

use auction_observability::{DatabaseConflictSite, DatabaseConnectionGuard};
use sqlx::{Database, Transaction, pool::PoolConnection};

/// 普通或长期所有权连接；close_on_drop 仍交给原始 PoolConnection 执行。
pub(crate) struct TrackedConnection<DB: Database> {
    inner: PoolConnection<DB>,
    guard: Option<DatabaseConnectionGuard>,
}

impl<DB: Database> TrackedConnection<DB> {
    pub(super) fn new(inner: PoolConnection<DB>, guard: Option<DatabaseConnectionGuard>) -> Self {
        Self { inner, guard }
    }

    pub(crate) fn close_on_drop(&mut self) {
        self.inner.close_on_drop();
    }

    pub(super) fn into_parts(self) -> (PoolConnection<DB>, Option<DatabaseConnectionGuard>) {
        (self.inner, self.guard)
    }
}

impl<DB: Database> Deref for TrackedConnection<DB> {
    type Target = DB::Connection;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<DB: Database> DerefMut for TrackedConnection<DB> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

/// SQLx 原事务与持有计量一起移动；计量失败不会触碰事务结果。
pub struct TrackedTransaction<'connection, DB: Database> {
    inner: Transaction<'connection, DB>,
    guard: Option<DatabaseConnectionGuard>,
}

impl<'connection, DB: Database> TrackedTransaction<'connection, DB> {
    pub(super) fn new(
        inner: Transaction<'connection, DB>,
        guard: Option<DatabaseConnectionGuard>,
    ) -> Self {
        Self { inner, guard }
    }

    /// 长期 owner 已持有连接时，不再重复增加普通循环计量。
    pub(crate) fn borrowed(inner: Transaction<'connection, DB>) -> Self {
        Self::new(inner, None)
    }

    /// 原生 SQLx 辅助回调借用同一事务，持有守卫仍由外层 owner 保管。
    pub(crate) fn native(&mut self) -> &mut Transaction<'connection, DB> {
        &mut self.inner
    }

    /// 原样提交；守卫覆盖 await、取消和错误映射前的 SQLSTATE 投影。
    pub async fn commit(self) -> Result<(), sqlx::Error> {
        let Self { inner, guard } = self;
        let result = inner.commit().await;
        if let Err(error) = &result {
            super::observe_sql_error(error, DatabaseConflictSite::Unknown);
        }
        drop(guard);
        result
    }

    /// 原样回滚；只有调用方收到成功才算已确认回滚。
    pub async fn rollback(self) -> Result<(), sqlx::Error> {
        let Self { inner, guard } = self;
        let result = inner.rollback().await;
        drop(guard);
        result
    }
}

impl<DB: Database> Deref for TrackedTransaction<'_, DB> {
    type Target = DB::Connection;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<DB: Database> DerefMut for TrackedTransaction<'_, DB> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}
