//! 进程 TOML 配置与敏感配置加载；凭据必须经显式 getter 读取，不能依赖 `Debug` 输出或环境变量覆盖。
//! 主配置通过 `secrets_file` 引用独立的敏感 TOML。

use std::{
    collections::BTreeSet,
    env, fmt, fs, io,
    net::SocketAddr,
    path::{Path, PathBuf},
};

use chrono::Utc;
use ipnet::IpNet;
use serde::Deserialize;
use thiserror::Error;
use url::Url;

use auction_application::{AdminRuntimeDocument, RecurringSchedule};

use crate::StandardCronSchedule;

const CONFIG_FILE_ENV: &str = "AUCTION_CONFIG_FILE";

mod app;
mod auction;
mod backup;
mod database;
pub use database::*;
mod errors;
mod operations;
mod secrets;
mod security;
mod shutdown;
use shutdown::ShutdownFileConfig;
pub use shutdown::ShutdownSettings;
mod validation;
mod worker;
mod worker_validation;

use self::operations::apply_runtime_operations;
pub(crate) use self::secrets::validate_runtime_startup;
use self::secrets::{AppSecrets, SecretString};
use self::secrets::{
    admin_runtime_listener, load_admin_runtime_file, load_secret_file, runtime_http_security,
    runtime_session, runtime_tracing,
};
use self::validation::*;
use self::worker::{apply_equipment_colors, apply_runtime_document};
use self::worker_validation::*;

pub use app::*;
pub use auction::*;
pub use backup::{BackupConfig, BackupOffsiteConfig};
pub use errors::ConfigError;
pub use secrets::{
    MoogleMailKeyEnv, PostgresConnectionConfig, PostgresMaintenanceSecrets, load_mooglemail_keyenv,
    load_postgres_maintenance_secrets,
};
pub use security::*;
pub use worker::*;
