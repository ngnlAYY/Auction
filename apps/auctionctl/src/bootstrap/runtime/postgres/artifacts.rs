//! PostgreSQL 运维 artifact 辅助模块装配。
//!
//! 外部命令环境、备份 manifest、文件系统安全、命令执行和证据输入分别由子模块拥有；本文件
//! 只保留原有 `runtime::postgres` 的稳定导出，避免 restore、rebuild 与 legacy v28 形成跨层
//! 依赖。

#[cfg(feature = "postgres")]
mod environment;
#[cfg(feature = "postgres")]
mod filesystem;
#[cfg(feature = "postgres")]
mod input;
#[cfg(feature = "postgres")]
mod manifest;
#[cfg(feature = "postgres")]
mod process;
#[cfg(feature = "postgres")]
mod source;

#[cfg(feature = "postgres")]
pub(crate) use environment::PostgresCommandEnvironment;
#[cfg(feature = "postgres")]
pub(crate) use environment::{decode_uri_component, require_pg_value};
#[cfg(feature = "postgres")]
pub(crate) use filesystem::{
    LegacyV28AdoptionReceiptSpec, PrivateReceiptReservation, RebuildReceiptSpec,
    RestoreReceiptSpec, backup_manifest_path, create_private_temp_path, publish_private_artifact,
    remove_owned_temp, require_absent_artifact, require_dump_extension, sync_private_artifact,
    validate_private_evidence_path, verify_private_regular_file, write_private_evidence,
};
#[cfg(feature = "postgres")]
pub(crate) use input::read_bootstrap_cookie;
#[cfg(feature = "postgres")]
pub(crate) use manifest::{
    is_sha256_hex, manifest_archive_path, read_backup_manifest, sha256_file, sha256_text,
    validate_backup_manifest,
};
#[cfg(feature = "postgres")]
pub(crate) use process::{run_external_command, run_pg_dump, verify_archive};
#[cfg(feature = "postgres")]
pub(crate) use source::require_backup_source;
