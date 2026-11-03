//! 服务池、维护池与部署连接预算的闭集 TOML 模型。

use serde::{Deserialize, Serialize};

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
