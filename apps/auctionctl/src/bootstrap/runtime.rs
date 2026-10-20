//! auctionctl 装配入口的兼容层；CLI 类型、分派、部署检查和 PostgreSQL 运维实现位于
//! `runtime/` 子目录。

use anyhow::Context;
#[cfg(feature = "postgres")]
use auction_application::{
    AdminCredentialImportCommand, AdminCredentialKind, AdminCredentialSecret,
    AdminCredentialService, AdminCredentialWorld, ForumSyncRecoveryPort,
    ForumWriteRetryAuthorizationCommand, RequestHash,
};
use auction_config::{AppConfig, HttpListenMode, Role};
#[cfg(feature = "postgres")]
use auction_integrations::{CredentialSealer, DataKey};
use auction_storage::DatabaseBackend;
#[cfg(feature = "postgres")]
use auction_storage::DatabaseStore;
#[cfg(feature = "postgres")]
use chrono::{DateTime, Utc};
use clap::{Args, Parser, Subcommand, ValueEnum};
#[cfg(feature = "postgres")]
use serde::{Deserialize, Serialize};
#[cfg(feature = "postgres")]
use sha2::{Digest, Sha256};
#[cfg(feature = "postgres")]
use std::collections::HashSet;
#[cfg(feature = "postgres")]
use std::io::{Read, Write};
#[cfg(all(feature = "postgres", unix))]
use std::os::unix::fs::OpenOptionsExt;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(feature = "postgres")]
use std::process::{Command as ProcessCommand, Stdio};
#[cfg(feature = "postgres")]
use std::sync::Arc;
use std::{
    fs,
    net::SocketAddr,
    path::{Path, PathBuf},
};
#[cfg(feature = "postgres")]
use url::Url;
use uuid::Uuid;

mod deployment_control;
#[cfg(feature = "postgres")]
mod postgres_legacy_v28;
#[cfg(feature = "postgres")]
mod postgres_rebuild;
#[cfg(feature = "postgres")]
mod postgres_restore;

#[path = "runtime/bot_funds/mod.rs"]
mod bot_funds;
#[path = "runtime/bot_funds_record_only/mod.rs"]
mod bot_funds_record_only;
#[path = "runtime/cli.rs"]
mod cli;
#[path = "runtime/deployment.rs"]
mod deployment;
#[path = "runtime/deployment_identity/mod.rs"]
mod deployment_identity;
#[path = "runtime/dispatch.rs"]
mod dispatch;
#[path = "runtime/legacy_settlement/mod.rs"]
mod legacy_settlement;
#[path = "runtime/mail_diagnostics/mod.rs"]
mod mail_diagnostics;
#[cfg(unix)]
#[path = "runtime/managed.rs"]
mod managed;
#[path = "runtime/postgres/mod.rs"]
mod postgres;

use cli::{
    Cli, Command, DeploymentAction, LotWatchPmAction, MailEvidenceAction, PostgresRebuildModeArg,
    SystemdControllerArgs,
};
use deployment::{
    DeploymentListener, admin_deployment_listener, deployment_check, public_deployment_listener,
};
use dispatch::migrate_config;
pub use dispatch::run;
use postgres::*;
