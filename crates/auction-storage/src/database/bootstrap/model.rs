//! PostgreSQL bootstrap 的安全结果和失败分类。

use thiserror::Error;

/// PostgreSQL bootstrap 的幂等结果；两种结果都不表示已经执行 schema migration。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PostgresBootstrapResult {
    /// 目标库不存在，本次只执行了 CREATE DATABASE 并完成后置连接验证。
    Created { database_name: String, owner: String },
    /// 目标库已存在，owner 与应用角色一致且应用 URL 可连接；本次未修改数据库。
    AlreadyExists { database_name: String, owner: String },
}

/// PostgreSQL bootstrap 的确定性失败分类。
///
/// 故意不携带 SQLx 原始错误文本，避免驱动诊断意外包含连接 URL、用户名或其他敏感材料。
#[derive(Debug, Error)]
pub enum PostgresDatabaseBootstrapError {
    #[error("PostgreSQL bootstrap 的数据库名无效")]
    InvalidDatabaseName,
    #[error("PostgreSQL bootstrap 的应用角色名无效")]
    InvalidOwner,
    #[error("拒绝操作 PostgreSQL 维护数据库")]
    ProtectedDatabase,
    #[error("无法连接 PostgreSQL 维护库")]
    MaintenanceConnection,
    #[error("维护连接未连接到 postgres 数据库")]
    WrongMaintenanceDatabase,
    #[error("维护连接不是可写 primary")]
    ReadOnlyPrimary,
    #[error("维护角色缺少 CREATEDB 权限")]
    MissingCreateDatabasePrivilege,
    #[error("应用数据库角色不存在")]
    OwnerRoleMissing,
    #[error("维护角色不能把数据库 owner 分配给应用角色")]
    CannotAssignOwner,
    #[error("无法取得 PostgreSQL 数据库 advisory lock")]
    AdvisoryLockBusy,
    #[error("读取 PostgreSQL bootstrap catalog 失败：{stage}")]
    CatalogQuery { stage: &'static str },
    #[error("创建 PostgreSQL 数据库失败")]
    CreateDatabase,
    #[error("目标数据库创建后的 catalog 校验失败")]
    CreatedDatabaseVerification,
    #[error("应用数据库连接失败")]
    ApplicationConnection,
    #[error("应用连接的数据库身份或角色不匹配")]
    ApplicationIdentityMismatch,
}
