//! 分档参数与最终 HTTP 时限校验；不猜测运行时 readiness 开关或数据库现状。

use super::{
    DatabasePoolConfig, DatabaseProfileMode, DatabaseProfilesConfig, DatabaseServiceTimeouts,
};
use crate::{ConfigError, Role};

fn invalid(reason: &'static str) -> ConfigError {
    ConfigError::InvalidDatabaseProfile(reason)
}

/// W12.5：拍卖关键循环（生命周期、慢速代理、独立结算、结算履约）保留的连接数。
/// 关键循环从不获取批量配额，因此这些连接不会被邮件、论坛、投递或维护轮次占用。
pub const WORKER_CRITICAL_CONNECTION_RESERVE: u32 = 4;

/// Worker 服务池的长期预留（运行时开关刷新、热配置读取与心跳等常驻任务）。
const WORKER_LONG_LIVED_CONNECTION_RESERVE: u32 = 3;

/// readiness/heartbeat 依赖探针的保留连接；普通批量轮次不得占用。
const READINESS_CONNECTION_RESERVE: u32 = 1;

impl DatabasePoolConfig {
    /// W12.5：Worker 批量与维护循环的并发连接配额。
    ///
    /// 服务池上限扣除长期预留（与校验中的 Worker 3 个一致）、关键循环保留与 readiness 探针
    /// 保留后，其余连接作为批量轮次的并发上限，至少 1 个；配额只限制批量轮次同时开始的数量，
    /// 不改变租约、重试或任何持久化事实（不变量 5）。
    pub fn worker_bulk_connection_budget(&self) -> u32 {
        self.max_connections
            .saturating_sub(
                WORKER_CRITICAL_CONNECTION_RESERVE
                    + WORKER_LONG_LIVED_CONNECTION_RESERVE
                    + READINESS_CONNECTION_RESERVE,
            )
            .max(1)
    }

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
            Role::Worker => WORKER_LONG_LIVED_CONNECTION_RESERVE,
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
