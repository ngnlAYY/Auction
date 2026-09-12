//! PostgreSQL rebuild 的授权、参数和回执上下文模型。

use std::path::Path;

use auction_storage::{PostgresRebuildPreflight, PostgresRebuildResult};
use chrono::{DateTime, Utc};
use serde::Serialize;

#[derive(Debug, Clone, Copy)]
pub enum Mode {
    EmptyForRestore,
    FreshSchema,
}

impl Mode {
    pub(super) const fn name(self) -> &'static str {
        match self {
            Self::EmptyForRestore => "empty_for_restore",
            Self::FreshSchema => "fresh_schema",
        }
    }
}

pub struct Options<'a> {
    pub(in crate::bootstrap) confirmed_database_name: &'a str,
    pub(in crate::bootstrap) mode: Mode,
    pub(in crate::bootstrap) backup_manifest: Option<&'a Path>,
    pub(in crate::bootstrap) allow_unrecoverable_ephemeral: bool,
    pub(in crate::bootstrap) confirm_ephemeral: Option<&'a str>,
    pub(in crate::bootstrap) maintenance_secrets_file: Option<&'a Path>,
    pub(in crate::bootstrap) execute: bool,
    pub(in crate::bootstrap) confirm_rebuild: Option<&'a str>,
    pub(in crate::bootstrap) receipt: Option<&'a Path>,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct AuthorizationReceipt {
    pub(super) kind: &'static str,
    pub(super) manifest_sha256: Option<String>,
    pub(super) archive_file_name: Option<String>,
    pub(super) archive_sha256: Option<String>,
    pub(super) backup_schema_version: Option<i64>,
    pub(super) backup_created_at: Option<DateTime<Utc>>,
    pub(super) unrecoverable_ephemeral: bool,
}

pub(super) enum Authorization<'a> {
    Backup {
        manifest_path: &'a Path,
        verified: super::super::postgres_restore::VerifiedBackup,
        receipt: AuthorizationReceipt,
    },
    Ephemeral {
        receipt: AuthorizationReceipt,
    },
}

impl Authorization<'_> {
    pub(super) fn receipt(&self) -> &AuthorizationReceipt {
        match self {
            Self::Backup { receipt, .. } | Self::Ephemeral { receipt } => receipt,
        }
    }

    pub(super) fn reverify(&self, confirmed_database_name: &str) -> anyhow::Result<()> {
        let Self::Backup { manifest_path, verified, .. } = self else {
            return Ok(());
        };
        let current = super::super::postgres_restore::verify_backup_pair(
            manifest_path,
            confirmed_database_name,
        )?;
        if current.manifest_sha256 != verified.manifest_sha256
            || current.archive_path != verified.archive_path
            || current.manifest.archive_sha256 != verified.manifest.archive_sha256
            || current.manifest.schema_version != verified.manifest.schema_version
            || current.manifest.created_at != verified.manifest.created_at
        {
            anyhow::bail!("backup archive or manifest changed during PostgreSQL rebuild preflight");
        }
        // 用户提供的既有备份也必须同步，不能只依赖当前页缓存内可读的摘要。
        super::super::sync_private_artifact(&current.archive_path)?;
        super::super::sync_private_artifact(manifest_path)?;
        Ok(())
    }
}

pub(super) struct ReceiptContext<'a> {
    pub(super) options: &'a Options<'a>,
    pub(super) authorization: &'a AuthorizationReceipt,
    pub(super) source_schema_version: i64,
    pub(super) preflight: &'a PostgresRebuildPreflight,
    pub(super) rebuild: Option<&'a PostgresRebuildResult>,
    pub(super) status: &'static str,
    pub(super) failure_stage: Option<&'static str>,
    pub(super) database_dropped: Option<bool>,
    pub(super) database_recreated: Option<bool>,
    pub(super) postcondition: Option<&'static str>,
    pub(super) migration_version: Option<i64>,
}

impl<'a> ReceiptContext<'a> {
    pub(super) fn completed(
        options: &'a Options<'a>,
        authorization: &'a AuthorizationReceipt,
        source_schema_version: i64,
        preflight: &'a PostgresRebuildPreflight,
        rebuild: &'a PostgresRebuildResult,
        postcondition: &'static str,
        migration_version: Option<i64>,
    ) -> Self {
        Self {
            options,
            authorization,
            source_schema_version,
            preflight,
            rebuild: Some(rebuild),
            status: "completed",
            failure_stage: None,
            database_dropped: Some(true),
            database_recreated: Some(true),
            postcondition: Some(postcondition),
            migration_version,
        }
    }
}
