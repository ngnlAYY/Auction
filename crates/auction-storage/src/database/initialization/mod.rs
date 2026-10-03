//! 合并入口的首次空库初始化；不是升级、接管或破坏性重建入口。

use sqlx::{Connection, PgConnection, migrate::Migrate};

/// 首次初始化失败不会回显数据库连接或底层 SQL 错误。
#[derive(Debug, thiserror::Error)]
#[error("首次数据库初始化失败；要求专用空 PostgreSQL 库且无其他客户端，请核对配置及迁移状态")]
pub struct FirstDatabaseInitializationError;

/// 在 SQLx 迁移锁下确认空库与零其他客户端，再应用仓库迁移；绝不清空已有对象。
///
/// 调用者必须先取得本机 supervisor 所有权，且尚未派生任何角色。迁移部分完成时保留现场，
/// 后续普通启动不会再次调用此入口；恢复或升级需要显式运维授权。
pub async fn initialize_empty_postgres(
    database_url: &str,
) -> Result<(), FirstDatabaseInitializationError> {
    let options = crate::deployment_identity::postgres_connect_options(database_url)
        .map_err(|_| FirstDatabaseInitializationError)?;
    let mut connection =
        PgConnection::connect_with(&options).await.map_err(|_| FirstDatabaseInitializationError)?;
    let result = initialize(&mut connection).await;
    // 关闭专用连接同时释放 session 迁移锁，不把持锁连接放回池。
    let closed = connection.close().await;
    result.and(closed.map_err(|_| FirstDatabaseInitializationError))
}

async fn initialize(connection: &mut PgConnection) -> Result<(), FirstDatabaseInitializationError> {
    connection.lock().await.map_err(|_| FirstDatabaseInitializationError)?;
    let occupied: bool = sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
            WHERE n.nspname NOT IN ('pg_catalog', 'information_schema')
              AND n.nspname NOT LIKE 'pg_toast%'
         ) OR EXISTS (
            SELECT 1 FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace
            WHERE n.nspname NOT IN ('pg_catalog', 'information_schema')
         ) OR EXISTS (
            SELECT 1 FROM pg_type t JOIN pg_namespace n ON n.oid = t.typnamespace
            WHERE n.nspname NOT IN ('pg_catalog', 'information_schema')
              AND n.nspname NOT LIKE 'pg_toast%'
         ) OR EXISTS (
            SELECT 1 FROM pg_namespace
            WHERE nspname NOT IN ('pg_catalog', 'information_schema', 'public')
              AND nspname NOT LIKE 'pg_toast%'
         ) OR EXISTS (
            SELECT 1 FROM pg_stat_activity
            WHERE datname = current_database() AND pid <> pg_backend_pid()
              AND backend_type = 'client backend'
         )",
    )
    .fetch_one(&mut *connection)
    .await
    .map_err(|_| FirstDatabaseInitializationError)?;
    if occupied {
        return Err(FirstDatabaseInitializationError);
    }
    // 复用普通迁移的保留字兼容执行视图，保留历史 checksum；本连接已经持有迁移锁。
    let mut migrator =
        super::migrations::postgres_migrator().map_err(|_| FirstDatabaseInitializationError)?;
    migrator.set_locking(false);
    migrator.run(&mut *connection).await.map_err(|_| FirstDatabaseInitializationError)
}
