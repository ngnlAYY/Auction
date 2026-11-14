//! 进程 TOML 配置与敏感配置加载；凭据必须经显式 getter 读取，不能依赖 `Debug` 输出或环境变量覆盖。
//! 主配置通过 `secrets_file` 引用独立的敏感 TOML。

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use auction_application::scheduling::RecurringSchedule;
use chrono::Utc;
use serde::Deserialize;
use thiserror::Error;
use url::Url;

use crate::StandardCronSchedule;

const CONFIG_FILE_ENV: &str = "AUCTION_CONFIG_FILE";

mod app;
mod auction;
mod backup;
mod database;
pub use auction_domain::DataKeyDomain;
pub use database::*;
mod errors;
mod operations;
mod secrets;
mod security;
mod shutdown;
pub use shutdown::ShutdownSettings;
mod validation;
mod worker;
mod worker_validation;

pub(crate) use self::secrets::validate_runtime_startup;

pub use app::*;
pub use auction::*;
pub use backup::{BackupConfig, BackupOffsiteConfig};
pub use errors::ConfigError;
pub use secrets::{
    DATA_KEY_RETENTION_MONTHS, MoogleMailKeyEnv, PostgresConnectionConfig,
    PostgresMaintenanceSecrets, PostgresOwnerSecrets, RetiredDataKey, database_url_username,
    load_mooglemail_keyenv, load_postgres_maintenance_secrets, load_postgres_owner_secrets,
};
pub use security::*;
pub use worker::*;
