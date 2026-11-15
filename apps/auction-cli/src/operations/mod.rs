//! 业务运维命令；只装配既有应用端口与存储实现，不绕过领域规则、幂等账本或追加式审计。

pub(crate) mod alerts;
pub(crate) mod credentials;
pub(crate) mod forum;
pub(crate) mod funds;
pub(crate) mod keys;
pub(crate) mod mail;
pub(crate) mod operator_identity;
pub(crate) mod recovery;
pub(crate) mod settlement;
pub(crate) mod switches;
pub(crate) mod watch_pm;

use std::sync::Arc;

use auction_config::{AppConfig, DataKeyDomain};
use auction_integrations::{Keyring, build_keyring};
use auction_storage::DatabaseStore;

use crate::config_dir::ConfigDir;

/// 加载 Worker 配置、连接其数据库并执行 `operation`；共享存储以 `Arc` 交给应用服务，
/// 无论成败都先关闭连接池再返回结果。
async fn with_worker_store<T>(
    config_dir: &ConfigDir,
    purpose: auction_storage::DatabaseConnectionPurpose,
    operation: impl AsyncFnOnce(&AppConfig, &Arc<DatabaseStore>) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let config = config_dir.load_worker()?;
    with_config_store(&config, purpose, async |store| operation(&config, store).await).await
}

/// 用已加载的 Worker 配置连接数据库并执行 `operation`，无论成败都先关闭连接池；需要在连接
/// 之前完成配置门禁的命令先自行加载配置再调用。
async fn with_config_store<T>(
    config: &AppConfig,
    purpose: auction_storage::DatabaseConnectionPurpose,
    operation: impl AsyncFnOnce(&Arc<DatabaseStore>) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let store = Arc::new(crate::database::connect(config, purpose).await?);
    let result = operation(&store).await;
    store.postgres_pool().close().await;
    result
}

/// 按 Worker 配置装配单个数据密钥域的密钥环：未配置当前密钥时为 None；密钥格式错误直接
/// 失败，错误只含固定分类，不回显密钥材料。
fn keyring(config: &AppConfig, domain: DataKeyDomain) -> anyhow::Result<Option<Arc<Keyring>>> {
    build_keyring(
        domain,
        config.data_key_current(domain),
        config.data_key_previous(domain).iter().map(|key| (key.id(), key.material())),
    )
    .map_err(|error| anyhow::anyhow!("{} 域密钥环无效：{error}", domain.as_str()))
}
