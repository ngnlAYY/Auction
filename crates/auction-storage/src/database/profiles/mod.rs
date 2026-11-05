//! 显式连接用途与会话初始化；不读取配置，保留目标解析、TLS 与驱动 UTC 启动参数。

use std::time::Duration;

use sqlx::{Connection, PgConnection};
use thiserror::Error;

mod purpose;
pub use purpose::DatabaseConnectionPurpose;

/// 启用前由配置 owner 校验且显式填写的服务限界。
#[derive(Debug, Clone, Copy)]
pub struct DatabaseConnectionTimeouts {
    pub acquire_millis: u64,
    pub statement_millis: u64,
    pub lock_millis: u64,
    pub idle_transaction_millis: u64,
}

/// 装配根选择的连接档位；legacy 不设置名称、hook 或任何会话参数。
#[derive(Debug, Clone, Copy)]
pub struct DatabaseConnectionProfile {
    purpose: Option<DatabaseConnectionPurpose>,
    max_connections: u32,
    acquire_millis: u64,
    timeouts: Option<DatabaseConnectionTimeouts>,
    maintenance: bool,
}

impl DatabaseConnectionProfile {
    /// 完整保留旧池上限、获取时限和会话继承语义。
    pub const fn legacy() -> Self {
        Self {
            purpose: None,
            max_connections: 16,
            acquire_millis: 30_000,
            timeouts: None,
            maintenance: false,
        }
    }

    /// 服务物理上限含长期 owner；关闭限界时仍沿用 30 秒 acquire。
    pub fn service(
        purpose: DatabaseConnectionPurpose,
        max_connections: u32,
        timeouts: Option<DatabaseConnectionTimeouts>,
    ) -> Self {
        Self {
            purpose: Some(purpose),
            max_connections,
            acquire_millis: timeouts.map_or(30_000, |value| value.acquire_millis),
            timeouts,
            maintenance: false,
        }
    }

    /// 维护档基础 statement_timeout 为零，既有 owner 的局部限界保持有效。
    pub const fn maintenance(
        purpose: DatabaseConnectionPurpose,
        max_connections: u32,
        acquire_millis: u64,
    ) -> Self {
        Self {
            purpose: Some(purpose),
            max_connections,
            acquire_millis,
            timeouts: None,
            maintenance: true,
        }
    }

    /// 普通业务可同时持有的连接上限；长期 owner 与 readiness 各保留一个物理连接。
    /// 与配置校验中的 Public 1 / Admin 0 / Worker 3 个长期预留保持同一口径；
    /// CLI/维护档没有 readiness 探针，不额外扣减容量。
    pub(crate) const fn ordinary_capacity(self) -> u32 {
        let long_lived = match self.purpose {
            Some(DatabaseConnectionPurpose::Public) => 1,
            Some(DatabaseConnectionPurpose::Worker) => 3,
            // legacy 档不区分角色；按 Worker 的最坏长期 owner 预留，避免探针无连接。
            None => 3,
            Some(_) => 0,
        };
        let readiness = match self.purpose {
            Some(
                DatabaseConnectionPurpose::Public
                | DatabaseConnectionPurpose::Admin
                | DatabaseConnectionPurpose::Worker,
            )
            | None => 1,
            Some(_) => 0,
        };
        let capacity = self.max_connections.saturating_sub(long_lived).saturating_sub(readiness);
        if capacity == 0 { 1 } else { capacity }
    }

    pub(crate) fn pool_options(self) -> sqlx::postgres::PgPoolOptions {
        let options = sqlx::postgres::PgPoolOptions::new()
            .max_connections(self.max_connections)
            .acquire_timeout(Duration::from_millis(self.acquire_millis));
        if self.purpose.is_none() {
            return options;
        }
        options.after_connect(move |connection, _| Box::pin(self.initialize(connection)))
    }

    pub(crate) fn connect_options(
        self,
        url: &str,
    ) -> Result<sqlx::postgres::PgConnectOptions, sqlx::Error> {
        if self.max_connections == 0 || self.acquire_millis == 0 {
            return Err(sqlx::Error::Configuration(Box::new(InvalidProfile)));
        }
        let options = crate::deployment_identity::postgres_connect_options(url)?;
        Ok(match self.purpose {
            Some(purpose) => options.application_name(purpose.application_name()),
            None => options,
        })
    }

    /// 直连与池共用初始化；失败会丢弃该连接，不把未完成设置的会话交给 owner。
    pub(crate) async fn connect(self, url: &str) -> Result<PgConnection, sqlx::Error> {
        let options = self.connect_options(url)?;
        let mut connection = PgConnection::connect_with(&options).await?;
        self.initialize(&mut connection).await?;
        Ok(connection)
    }

    async fn initialize(self, connection: &mut PgConnection) -> Result<(), sqlx::Error> {
        let Some(purpose) = self.purpose else {
            return Ok(());
        };
        // 使用固定 GUC 名与绑定值，一次往返；false 为会话作用域，事务 LOCAL 可临时覆盖。
        // 关闭服务超时时只设置名称，不以零覆盖服务器原有超时，保护旧继承边界。
        if let Some(timeouts) = self.timeouts {
            sqlx::query(
                "SELECT set_config('application_name', $1, false),
                        set_config('statement_timeout', $2, false),
                        set_config('lock_timeout', $3, false),
                        set_config('idle_in_transaction_session_timeout', $4, false)",
            )
            .bind(purpose.application_name())
            .bind(format!("{}ms", timeouts.statement_millis))
            .bind(format!("{}ms", timeouts.lock_millis))
            .bind(format!("{}ms", timeouts.idle_transaction_millis))
            .execute(connection)
            .await?;
        } else if self.maintenance {
            sqlx::query(
                "SELECT set_config('application_name', $1, false),
                        set_config('statement_timeout', '0', false)",
            )
            .bind(purpose.application_name())
            .execute(connection)
            .await?;
        } else {
            sqlx::query("SELECT set_config('application_name', $1, false)")
                .bind(purpose.application_name())
                .execute(connection)
                .await?;
        }
        Ok(())
    }
}

#[derive(Debug, Error)]
#[error("数据库连接档位无效")]
struct InvalidProfile;

#[cfg(test)]
mod tests {
    use self::parent::*;
    use super as parent;

    #[test]
    fn ordinary_capacity_reserves_long_lived_and_readiness_slots() {
        let public =
            DatabaseConnectionProfile::service(DatabaseConnectionPurpose::Public, 16, None);
        let admin = DatabaseConnectionProfile::service(DatabaseConnectionPurpose::Admin, 16, None);
        let worker =
            DatabaseConnectionProfile::service(DatabaseConnectionPurpose::Worker, 16, None);
        let legacy = DatabaseConnectionProfile::legacy();
        assert_eq!(public.ordinary_capacity(), 14, "Public 1 长期 owner + 1 readiness");
        assert_eq!(admin.ordinary_capacity(), 15, "Admin 仅扣 1 readiness");
        assert_eq!(worker.ordinary_capacity(), 12, "Worker 3 长期预留 + 1 readiness");
        assert_eq!(legacy.ordinary_capacity(), 12, "legacy 按 Worker 最坏 owner 与探针预留");
    }

    #[test]
    fn tiny_pools_keep_at_least_one_ordinary_slot() {
        let admin = DatabaseConnectionProfile::service(DatabaseConnectionPurpose::Admin, 1, None);
        let maintenance = DatabaseConnectionProfile::maintenance(
            DatabaseConnectionPurpose::CliBackupCheck,
            1,
            30_000,
        );
        assert_eq!(admin.ordinary_capacity(), 1);
        assert_eq!(maintenance.ordinary_capacity(), 1, "维护档只有一个槽位时不额外扣减");
    }
}
