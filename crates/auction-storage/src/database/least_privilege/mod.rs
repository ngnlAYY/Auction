//! 运行账号最小权限的重放入口。
//!
//! 授权定义只有一份：迁移 `000081` 固化的 `public.apply_auction_runtime_privileges()`。
//! 备份以 `--no-owner --no-privileges` 恢复，对象 ACL 与默认权限都不在 dump 中，
//! 因此恢复后必须由迁移 owner 再次执行该函数；`auction-cli db grants` 直接调用这里。

use sqlx::PgPool;

use super::StorageError;

/// 迁移 `000081` 的授权函数是否已存在于当前数据库（旧备份恢复后可能尚未执行该迁移）。
pub async fn runtime_privileges_present(pool: &PgPool) -> Result<bool, StorageError> {
    sqlx::query_scalar::<_, bool>(
        "SELECT to_regprocedure('public.apply_auction_runtime_privileges()') IS NOT NULL",
    )
    .fetch_one(pool)
    .await
    .map_err(|source| StorageError::RuntimePrivileges { source })
}

/// 以当前连接（必须是迁移 owner）重放运行账号授权；函数内的角色前置检查不满足时整体报错。
pub async fn apply_runtime_privileges(pool: &PgPool) -> Result<(), StorageError> {
    // 函数返回 void，不能用行解码；服务端在前面抛出的角色前置错误会以 SQL 错误返回。
    sqlx::query("SELECT public.apply_auction_runtime_privileges()")
        .execute(pool)
        .await
        .map_err(|source| StorageError::RuntimePrivileges { source })?;
    Ok(())
}
