//! 进程 TOML 配置与敏感配置加载。
//!
//! 普通配置负责监听地址、缓存模式等非敏感设置；主配置可通过 `secrets_file` 引用独立的
//! 敏感 TOML。任何凭据都必须通过显式 getter 使用，不能依赖 `Debug` 输出或环境变量覆盖。

use std::{
    collections::BTreeSet,
    env, fmt, fs, io,
    io::Read,
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    str::FromStr,
};

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

use chrono::{Duration, Utc};
use ipnet::IpNet;
use serde::Deserialize;
use thiserror::Error;
use url::Url;

use auction_application::{AdminRuntimeDocument, AuctionPolicyTemplate, RecurringSchedule};
use auction_domain::{
    AdjustmentAnchor, RecoveryMode, RecoveryPolicy, SettlementMode, SettlementPolicy,
};

use crate::StandardCronSchedule;

const CONFIG_FILE_ENV: &str = "AUCTION_CONFIG_FILE";

mod app;
mod auction;
mod errors;
mod secrets;
mod security;
mod validation;
mod worker;
mod worker_validation;

use self::secrets::{AppSecrets, SecretString};
use self::secrets::{
    load_admin_runtime_file, load_secret_file, runtime_http_security, runtime_session,
};
use self::validation::*;
use self::worker_validation::*;

pub use app::*;
pub use auction::*;
pub use errors::ConfigError;
pub use secrets::{
    MoogleMailKeyEnv, PostgresMaintenanceSecrets, load_mooglemail_keyenv,
    load_postgres_maintenance_secrets,
};
pub use security::*;
pub use worker::*;
