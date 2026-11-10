//! 服务池、维护池与部署连接预算的闭集 TOML 模型。

use serde::{Deserialize, Serialize};

/// 迁移 081 授权对象的固定角色名；Public、Admin 与 Worker 必须共用该账号。
/// 该常量与迁移内的 `auction_runtime` 逐字一致，`config check --strict` 据此拒绝服务误用 owner 账号。
pub const RUNTIME_DATABASE_ROLE: &str = "auction_runtime";

/// 数据库账号分工的非秘密声明；账号与密码本身只存在于各自的敏感 TOML。
/// `owner` 是迁移、建库、重建与恢复使用的 owner 账号名，用于在离线预检中拒绝服务误用；
/// 运行账号固定为 [`RUNTIME_DATABASE_ROLE`]，不可配置，避免与迁移授权对象漂移。
#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct DatabaseRolesConfig {
    /// 可选：迁移 owner 账号名；生产严格预检要求声明，且不得等于运行账号。
    pub owner: Option<String>,
}

impl DatabaseRolesConfig {
    /// 账号名会原样出现在 SQL 标识符位置，只能是无需引号的小写标识符。
    pub(crate) fn validate(&self) -> Result<(), crate::ConfigError> {
        let Some(owner) = self.owner.as_deref() else { return Ok(()) };
        let invalid = || crate::ConfigError::InvalidDatabaseRole("database_roles.owner");
        if owner.len() > 63
            || !owner.starts_with(|first: char| first.is_ascii_lowercase() || first == '_')
            || owner.chars().any(|value| {
                !(value.is_ascii_lowercase() || value.is_ascii_digit() || value == '_')
            })
        {
            return Err(invalid());
        }
        if owner == RUNTIME_DATABASE_ROLE {
            return Err(crate::ConfigError::InvalidDatabaseRole(
                "database_roles.owner 不能与运行账号 auction_runtime 相同；迁移 owner 与服务账号必须分离",
            ));
        }
        Ok(())
    }

    /// 服务账号声明的强校验结果：迁移 owner 缺失或与运行账号冲突都返回原因。
    pub fn service_account_problem(&self, username: &str) -> Option<String> {
        if username == RUNTIME_DATABASE_ROLE {
            return None;
        }
        match self.owner.as_deref() {
            Some(owner) if owner == username => Some(format!(
                "服务数据库账号 {username} 是迁移 owner；Public/Admin/Worker 只能使用 {RUNTIME_DATABASE_ROLE}"
            )),
            _ => Some(format!(
                "服务数据库账号 {username} 不是迁移 081 授权的运行账号 {RUNTIME_DATABASE_ROLE}"
            )),
        }
    }

    /// 严格预检要求显式声明 owner 账号名，否则无法在离线状态下证明两者分离。
    pub fn require_owner_declared(&self) -> Result<(), crate::ConfigError> {
        if self.owner.is_none() {
            return Err(crate::ConfigError::InvalidDatabaseRole(
                "生产严格预检要求在 common.toml 的 [database_roles] 声明 owner（迁移 owner 账号名）",
            ));
        }
        Ok(())
    }
}

/// 数据库分档模式；legacy 完整保留原连接行为。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DatabaseProfileMode {
    Legacy,
    #[default]
    Profiled,
}

/// 公共连接策略；预算仅为部署声明，不读取服务器配置。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct DatabaseProfilesConfig {
    pub mode: DatabaseProfileMode,
    pub maintenance: DatabaseMaintenanceConfig,
    pub budget: DatabaseBudgetConfig,
}

/// 单角色服务池；三个新增超时阈值没有隐式默认值。
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct DatabasePoolConfig {
    pub max_connections: u32,
    pub timeouts_enabled: bool,
    pub acquire_timeout_millis: Option<u64>,
    pub statement_timeout_millis: Option<u64>,
    pub lock_timeout_millis: Option<u64>,
    pub idle_in_transaction_session_timeout_millis: u64,
}

impl Default for DatabasePoolConfig {
    fn default() -> Self {
        Self {
            max_connections: 16,
            timeouts_enabled: false,
            acquire_timeout_millis: None,
            statement_timeout_millis: None,
            lock_timeout_millis: None,
            idle_in_transaction_session_timeout_millis: 0,
        }
    }
}

/// 已显式填写的服务超时；仅在启用且校验通过时投影给装配根。
#[derive(Debug, Clone, Copy)]
pub struct DatabaseServiceTimeouts {
    pub acquire_timeout_millis: u64,
    pub statement_timeout_millis: u64,
    pub lock_timeout_millis: u64,
    pub idle_in_transaction_session_timeout_millis: u64,
}

/// CLI 每次调用的维护池上限；独立 guard、libpq 会话另计预算。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct DatabaseMaintenanceConfig {
    pub max_connections: u32,
    pub acquire_timeout_millis: u64,
}

impl Default for DatabaseMaintenanceConfig {
    fn default() -> Self {
        Self { max_connections: 4, acquire_timeout_millis: 30_000 }
    }
}

/// 连接预算声明；reserve 初值须由用户另行核实线上。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct DatabaseBudgetConfig {
    pub max_connections: u32,
    pub superuser_reserved_connections: u32,
    pub reserved_connections: u32,
    pub max_concurrent_cli: u32,
    pub external_reserved_connections: u32,
}

impl Default for DatabaseBudgetConfig {
    fn default() -> Self {
        Self {
            max_connections: 100,
            superuser_reserved_connections: 3,
            reserved_connections: 0,
            max_concurrent_cli: 2,
            external_reserved_connections: 8,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DatabaseRolesConfig, RUNTIME_DATABASE_ROLE};
    use crate::database_url_username;

    fn roles(owner: Option<&str>) -> DatabaseRolesConfig {
        DatabaseRolesConfig { owner: owner.map(str::to_owned) }
    }

    #[test]
    fn owner_declaration_must_be_a_safe_identifier() {
        assert!(roles(Some("auction_owner")).validate().is_ok());
        assert!(roles(Some("_owner_2")).validate().is_ok());
        assert!(roles(None).validate().is_ok());
        for invalid in ["Auction", "auction-owner", "auction owner", "\"owner\"", "1owner"] {
            assert!(roles(Some(invalid)).validate().is_err(), "{invalid}");
        }
        assert!(roles(Some(&"o".repeat(64))).validate().is_err());
        assert!(roles(Some(RUNTIME_DATABASE_ROLE)).validate().is_err());
    }

    #[test]
    fn runtime_role_is_the_only_accepted_service_account() {
        let declared = roles(Some("auction_owner"));
        assert!(declared.service_account_problem(RUNTIME_DATABASE_ROLE).is_none());
        let owner_problem = declared.service_account_problem("auction_owner").expect("owner");
        assert!(owner_problem.contains("迁移 owner"));
        let other = declared.service_account_problem("auction_local_app").expect("other");
        assert!(other.contains(RUNTIME_DATABASE_ROLE));
        // 未声明 owner 时同样只接受固定运行账号，避免离线放宽为任意账号。
        assert!(roles(None).service_account_problem(RUNTIME_DATABASE_ROLE).is_none());
        assert!(roles(None).service_account_problem("auction_owner").is_some());
    }

    #[test]
    fn strict_check_requires_owner_declaration() {
        assert!(roles(Some("auction_owner")).require_owner_declared().is_ok());
        let error = roles(None).require_owner_declared().expect_err("must require owner");
        assert!(error.to_string().contains("database_roles"));
    }

    #[test]
    fn database_url_username_reads_userinfo_and_query_only() {
        assert_eq!(
            database_url_username("postgresql://auction_runtime:secret@db:5432/auction").as_deref(),
            Some("auction_runtime")
        );
        assert_eq!(
            database_url_username("postgresql://db:5432/auction?user=auction_owner").as_deref(),
            Some("auction_owner")
        );
        assert_eq!(database_url_username("postgresql://db:5432/auction"), None);
        assert_eq!(database_url_username("not a url"), None);
    }
}
