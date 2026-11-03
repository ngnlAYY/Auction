//! 分档参数与最终 HTTP 时限校验；不猜测运行时 readiness 开关或数据库现状。

use super::{
    DatabasePoolConfig, DatabaseProfileMode, DatabaseProfilesConfig, DatabaseServiceTimeouts,
};
use crate::{ConfigError, Role};

fn invalid(reason: &'static str) -> ConfigError {
    ConfigError::InvalidDatabaseProfile(reason)
}

impl DatabasePoolConfig {
    /// 解析已显式填写的服务阈值；关闭时不写会话参数或缩短 acquire。
    pub fn timeouts(&self) -> Result<Option<DatabaseServiceTimeouts>, ConfigError> {
        if !self.timeouts_enabled {
            return Ok(None);
        }
        let (Some(acquire), Some(statement), Some(lock)) =
            (self.acquire_timeout_millis, self.statement_timeout_millis, self.lock_timeout_millis)
        else {
            return Err(invalid("启用超时须显式填写 acquire、statement 与 lock 三个毫秒阈值"));
        };
        Ok(Some(DatabaseServiceTimeouts {
            acquire_timeout_millis: acquire,
            statement_timeout_millis: statement,
            lock_timeout_millis: lock,
            idle_in_transaction_session_timeout_millis: self
                .idle_in_transaction_session_timeout_millis,
        }))
    }

    /// 保留不变量 2、5、6：连接上限含所有权预留，限界不能绕过最终 HTTP 配置。
    pub(crate) fn validate(
        &self,
        profiles: DatabaseProfilesConfig,
        role: Role,
        request_timeout_seconds: u64,
    ) -> Result<(), ConfigError> {
        let reserved = match role {
            Role::Public => 1,
            Role::Admin => 0,
            Role::Worker => 3,
        };
        if self.max_connections <= reserved {
            return Err(invalid(
                "服务池扣除 Public 1、Admin 0、Worker 3 个长期连接后须至少剩 1 个",
            ));
        }
        if self.timeouts_enabled && profiles.mode == DatabaseProfileMode::Legacy {
            return Err(invalid("legacy 模式不能启用服务超时"));
        }
        if self.acquire_timeout_millis.is_some_and(|value| !(1..=120_000).contains(&value))
            || self.statement_timeout_millis.is_some_and(|value| !(1..=120_000).contains(&value))
            || self.lock_timeout_millis.is_some_and(|value| !(1..=120_000).contains(&value))
            || self.idle_in_transaction_session_timeout_millis > 3_600_000
        {
            return Err(invalid("acquire、statement、lock 须为 1-120000ms，idle 须为 0-3600000ms"));
        }
        if let Some(timeouts) = self.timeouts()? {
            if role != Role::Worker
                && timeouts.acquire_timeout_millis >= request_timeout_seconds.saturating_mul(1000)
            {
                return Err(invalid("获取连接超时必须短于最终生效的 HTTP request_timeout"));
            }
            if timeouts.lock_timeout_millis >= timeouts.statement_timeout_millis {
                return Err(invalid("lock_timeout 必须短于 statement_timeout"));
            }
        }
        profiles.validate()
    }
}

impl DatabaseProfilesConfig {
    fn validate(self) -> Result<(), ConfigError> {
        let budget = self.budget;
        if self.maintenance.max_connections == 0
            || !(1..=120_000).contains(&self.maintenance.acquire_timeout_millis)
        {
            return Err(invalid("维护池连接数须非零，获取时限须为 1-120000ms"));
        }
        let reserved = u64::from(budget.superuser_reserved_connections)
            + u64::from(budget.reserved_connections);
        if budget.max_connections == 0
            || budget.max_concurrent_cli == 0
            || reserved >= u64::from(budget.max_connections)
        {
            return Err(invalid("连接预算须非零，保留连接合计须小于 max_connections"));
        }
        Ok(())
    }
}
