//! 业务运维命令；只装配既有应用端口与存储实现，不绕过领域规则、幂等账本或追加式审计。

pub(crate) mod alerts;
pub(crate) mod credentials;
pub(crate) mod forum;
pub(crate) mod funds;
#[cfg(feature = "postgres")]
pub(crate) mod keys;
pub(crate) mod mail;
mod operator_identity;
#[cfg(feature = "postgres")]
pub(crate) mod recovery;
pub(crate) mod rules;
pub(crate) mod settlement;
pub(crate) mod switches;
pub(crate) mod watch_pm;

use std::sync::Arc;

use auction_config::AppConfig;
use auction_storage::DatabaseStore;

use crate::deployment::ConfigDir;

/// 连接 Worker 配置的数据库，返回配置与共享存储。
async fn worker_store(
    config_dir: &ConfigDir,
    purpose: auction_storage::DatabaseConnectionPurpose,
) -> anyhow::Result<(AppConfig, Arc<DatabaseStore>)> {
    let config = config_dir.load_worker()?;
    let store = Arc::new(crate::database::connect(&config, purpose).await?);
    Ok((config, store))
}

/// 应用端口使用的系统时钟。
#[derive(Clone, Copy)]
struct SystemClock;

impl auction_application::core::Clock for SystemClock {
    fn now(&self) -> chrono::DateTime<chrono::Utc> {
        chrono::Utc::now()
    }
}
