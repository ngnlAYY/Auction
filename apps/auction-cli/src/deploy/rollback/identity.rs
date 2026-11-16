//! 回退回执与反向回滚记录的组件身份：每一项只取自它所描述的组件自己的事实，未知项显式为
//! null，不继承被回退版本的制品证据（v1 回执曾把它们混在一起）。
//!
//! - 回退后的角色身份：目标回滚记录保留的角色身份（含内嵌 schema，未知为 null）。
//! - 回退后的 CLI 身份：安装目录中保留的 CLI，摘要取自实际文件，内嵌 schema 取自执行本命令的
//!   同一程序；来源（包名、提交）只在部署回执的 CLI 摘要与之相同时沿用。
//! - 被替换的角色身份（写入反向记录）：部署回执描述的已安装程序身份；schema 缺失时用回滚记录
//!   登记的同一次操作所安装的角色 schema。

use std::path::Path;

use uuid::Uuid;

use super::super::install::RollbackRecord;
use crate::receipt::{CliIdentity, DeployRecord, DeploymentReceipt, RoleIdentity};

/// 回退回执的 v2 deploy 段；停机前构造，停机后的任何失败都以同一份发布。
pub(super) fn rollback_deploy_record(
    record: &RollbackRecord,
    install_dir: &Path,
    database_version: i64,
    cli: CliIdentity,
    reverse_receipt_id: Uuid,
) -> DeployRecord {
    DeployRecord {
        install_directory: install_dir.display().to_string(),
        roles: record.retained_roles.clone(),
        cli,
        database_version,
        migration_drill_id: None,
        backup_archive: None,
        backup_manifest: None,
        rollback_record_id: Some(reverse_receipt_id),
    }
}

/// 保留 CLI 的身份：摘要与内嵌 schema 来自实际程序；部署回执的 CLI 摘要相同时沿用其来源。
pub(super) fn kept_cli_identity(
    deployed: &DeploymentReceipt,
    sha256: &str,
    schema_version: i64,
) -> CliIdentity {
    let provenance = deployed
        .deploy
        .as_ref()
        .and_then(|section| {
            section.installed_identity(deployed.action, deployed.migration.as_ref())
        })
        .map(|(_, cli)| cli)
        .filter(|cli| cli.sha256 == sha256);
    CliIdentity {
        sha256: sha256.to_owned(),
        package: provenance.as_ref().and_then(|cli| cli.package.clone()),
        source_commit: provenance.and_then(|cli| cli.source_commit),
        schema_version: Some(schema_version),
    }
}

/// 被替换的（回退前的）角色身份：部署回执可信时用它描述的已安装身份，否则全部未知；内嵌
/// schema 缺失时用回滚记录登记的同一次操作所安装的角色 schema。
pub(super) fn replaced_roles_identity(
    deployed: &DeploymentReceipt,
    record: &RollbackRecord,
) -> RoleIdentity {
    let mut roles = deployed
        .deploy
        .as_ref()
        .and_then(|section| {
            section.installed_identity(deployed.action, deployed.migration.as_ref())
        })
        .map(|(roles, _)| roles)
        .unwrap_or_default();
    if roles.schema_version.is_none() {
        roles.schema_version = record.installed_role_schema_version;
    }
    roles
}
