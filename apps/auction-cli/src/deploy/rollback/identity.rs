//! 回退回执与反向回滚记录的组件身份：每一项只取自它所描述的组件自己的事实，未知项显式为
//! null，不继承被回退版本的制品证据（v1 回执曾把它们混在一起）。失败回执的 deploy 段只是尝试
//! 安装的目标，从不作为身份来源：安装中途失败并换回后，实际安装的仍是旧程序。
//!
//! - 回退后的角色身份：目标回滚记录保留的角色身份（含内嵌 schema，未知为 null）。
//! - 回退后的 CLI 身份：安装目录中保留的 CLI，摘要取自实际文件，内嵌 schema 取自执行本命令的
//!   同一程序；来源（包名、提交）只在成功部署回执的 CLI 摘要与之相同时沿用。
//! - 被替换的角色身份（写入反向记录）：按保留副本的摘要从成功回执重新绑定，无法证明时为 null；
//!   schema 缺失且部署回执成功时，用回滚记录登记的同一次操作所安装的角色 schema。

use std::path::Path;

use uuid::Uuid;

use super::super::install::{RetainedBinary, RetainedIdentity, RollbackRecord};
use super::super::receipts::{installed_identity, roles_match};
use crate::receipt::{CliIdentity, DeployRecord, DeploymentReceipt, Outcome};
use crate::support::files::PrivateDirectory;

/// 回退回执的 v2 deploy 段；停机前构造，停机后的任何失败都以同一份发布。它描述本次回退要安装
/// 的目标，失败回执的实际程序以回执 `artifacts` 的摘要为准。
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

/// 保留 CLI 的身份：摘要与内嵌 schema 来自实际程序；成功部署回执的 CLI 摘要相同时沿用其来源。
pub(super) fn kept_cli_identity(
    deployed: &DeploymentReceipt,
    sha256: &str,
    schema_version: i64,
) -> CliIdentity {
    let provenance =
        deployed.proven_identity().map(|(_, cli)| cli).filter(|cli| cli.sha256 == sha256);
    CliIdentity {
        sha256: sha256.to_owned(),
        package: provenance.as_ref().and_then(|cli| cli.package.clone()),
        source_commit: provenance.and_then(|cli| cli.source_commit),
        schema_version: Some(schema_version),
    }
}

/// 反向记录保留的（回退前的）三个角色程序的身份。停机前已核对安装目录是部署回执记录的版本，
/// 保留副本必须仍是同一组程序。身份按保留副本的摘要从成功回执重新绑定，与 deploy 保留旧程序同
/// 一口径；部署回执失败时，它的 deploy 段是尝试安装的目标而不是这组程序，不作为来源。内嵌
/// schema 缺失时，只有部署回执成功（当前程序就是它的操作装上的）才用回滚记录登记的同一次操作
/// 所安装的角色 schema。
pub(super) fn reverse_identity(
    state: &PrivateDirectory,
    deployed: &DeploymentReceipt,
    record: &RollbackRecord,
    binaries: &[RetainedBinary],
) -> anyhow::Result<RetainedIdentity> {
    if !roles_match(deployed, binaries) {
        anyhow::bail!("安装目录的角色程序在停机前后发生变化，与部署回执记录的版本不符，拒绝保留");
    }
    let mut roles = installed_identity(state, binaries).roles;
    if roles.schema_version.is_none() && deployed.outcome == Outcome::Succeeded {
        roles.schema_version = record.installed_role_schema_version;
    }
    Ok(RetainedIdentity { roles, cli: None })
}
