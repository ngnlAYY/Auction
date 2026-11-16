//! rollback 特有的停机前门禁，dry-run 同样执行：
//!
//! - 安装目录中的三角色程序必须仍是回滚记录对应部署回执的版本；
//! - rollback 不替换 auction-cli：被保留的 CLI 必须就是执行本命令的 CLI，且内嵌 schema 等于
//!   数据库版本，否则回退后的 `service start/restart`（含开机的 auction-stack）会先执行回退要
//!   避开的迁移，或无法管理超前的数据库；
//! - 数据库超前于目标角色程序时，三角色有效配置必须显式为 `compatible`，且目标程序支持该判定。
//!
//! 任一不满足都在停机前拒绝，不自动修改配置。

use std::path::Path;

use anyhow::Context;
use auction_application::operations::MigrationCompatibilityMode;
use auction_config::Role;

use super::super::install::RetainedBinary;
use super::super::role_binary;
use crate::receipt::DeploymentReceipt;
use crate::service::ManagedRole;
use crate::support::digest::sha256_path;

/// 迁移 075 与 `[readiness] migration_compatibility_mode` 同批引入：内嵌 schema 低于它的程序
/// 既不认识该字段，也没有兼容后缀判定。
pub(super) const COMPATIBLE_READINESS_SINCE: i64 = 75;

/// 安装目录中的三角色程序必须仍是部署回执记录的版本，否则回退目标不明确；返回它们的摘要，
/// 停机后保留反向记录时据此复核没有被替换。auction-cli 不参与比较：rollback 不替换它。
pub(super) fn require_installed_matches(
    deployed: &DeploymentReceipt,
    install_dir: &Path,
) -> anyhow::Result<Vec<RetainedBinary>> {
    let mut installed = Vec::with_capacity(ManagedRole::START_ORDER.len());
    for role in ManagedRole::START_ORDER {
        let name = role_binary(role);
        let recorded = deployed
            .artifacts
            .iter()
            .find(|artifact| artifact.role == role.name())
            .and_then(|artifact| artifact.sha256.as_deref())
            .with_context(|| format!("部署回执缺少 {} 的程序摘要", role.name()))?;
        let (sha256, bytes) = sha256_path(&install_dir.join(name))
            .with_context(|| format!("读取安装目录的 {name} 失败"))?;
        if sha256 != recorded {
            anyhow::bail!("安装目录的 {name} 不是该回滚记录对应的部署版本");
        }
        installed.push(RetainedBinary { name: name.to_owned(), sha256, bytes });
    }
    Ok(installed)
}

/// 被保留的 auction-cli 必须就是正在执行本命令的程序：它的内嵌版本由本进程确定，回退后由它
/// 继续执行备份、预检与 service 启停。返回安装目录 CLI 的摘要，停机后据此核对它没有变化。
pub(super) fn require_kept_cli(install_dir: &Path) -> anyhow::Result<String> {
    let installed = install_dir.join("auction-cli");
    let (installed_sha256, _) = sha256_path(&installed)
        .with_context(|| format!("读取安装目录的 auction-cli 失败：{}", installed.display()))?;
    let current = std::env::current_exe().context("读取当前 auction-cli 路径失败")?;
    let (current_sha256, _) = sha256_path(&current).context("计算当前 auction-cli 摘要失败")?;
    if installed_sha256 != current_sha256 {
        anyhow::bail!(
            "rollback 只换回三个运行角色并保留安装目录中的 auction-cli，必须由它执行：请改用 {}",
            installed.display()
        );
    }
    Ok(installed_sha256)
}

/// 保留的 CLI 内嵌版本必须等于数据库版本：较高时之后的 `service start` 会先执行回退要避开的
/// 迁移（例如部署在迁移前失败），较低时它无法预检、迁移或备份超前的数据库。
pub(super) fn require_cli_matches_database(
    cli_schema: i64,
    database_version: i64,
) -> anyhow::Result<()> {
    if cli_schema > database_version {
        anyhow::bail!(
            "保留的 auction-cli 期望 schema {cli_schema} 高于数据库 {database_version}（部署在迁移\
             完成前失败）：回退后任何 service start/restart（含开机的 auction-stack）都会先执行这些\
             迁移。修复原因后用安装目录的 CLI 执行 service start 完成升级，或按 \
             ops/deploy/RECOVERY.md 的“安装中途失败的手工换回”用回滚记录中的程序（含 CLI）整体换回"
        );
    }
    if cli_schema < database_version {
        anyhow::bail!(
            "保留的 auction-cli 期望 schema {cli_schema} 低于数据库 {database_version}，回退后无法\
             管理数据库；先用与数据库同版本的 auction-cli 重建安装目录"
        );
    }
    Ok(())
}

/// 数据库超前于目标角色程序时，旧程序只有在显式 `compatible` 模式下才会就绪（默认 shadow 与
/// strict 都要求迁移历史与内嵌集合完全一致）。逐角色核对有效配置，并要求目标程序已包含兼容
/// 判定；同版本回退不要求任何模式。返回写入报告的判定。
pub(super) fn require_compatible_readiness(
    modes: &[(Role, MigrationCompatibilityMode); 3],
    target_schema: i64,
    database_version: i64,
) -> anyhow::Result<serde_json::Value> {
    let projection = modes
        .iter()
        .map(|(role, mode)| (role.name().to_owned(), serde_json::json!(mode_name(*mode))))
        .collect::<serde_json::Map<_, _>>();
    if database_version <= target_schema {
        return Ok(serde_json::json!({ "compatible_required": false, "modes": projection }));
    }
    if target_schema < COMPATIBLE_READINESS_SINCE {
        anyhow::bail!(
            "数据库 {database_version} 高于目标角色程序的 schema {target_schema}，而该程序早于迁移 \
             {COMPATIBLE_READINESS_SINCE}，不支持兼容就绪判定：不能换回，请按 \
             ops/deploy/RECOVERY.md 恢复升级前备份"
        );
    }
    let lacking = modes
        .iter()
        .filter(|(_, mode)| *mode != MigrationCompatibilityMode::Compatible)
        .map(|(role, mode)| format!("{}={}", role.name(), mode_name(*mode)))
        .collect::<Vec<_>>();
    if !lacking.is_empty() {
        anyhow::bail!(
            "数据库 {database_version} 高于目标角色程序的 schema {target_schema}：旧程序只有在 \
             [readiness] migration_compatibility_mode = \"compatible\" 时才会就绪，当前有效配置为 \
             {}。请在三角色共用的 common 配置中显式设为 compatible 后重试（rollback 不修改配置），\
             回到同版程序后再恢复 shadow",
            lacking.join("、")
        );
    }
    Ok(serde_json::json!({ "compatible_required": true, "modes": projection }))
}

const fn mode_name(mode: MigrationCompatibilityMode) -> &'static str {
    match mode {
        MigrationCompatibilityMode::Strict => "strict",
        MigrationCompatibilityMode::Shadow => "shadow",
        MigrationCompatibilityMode::Compatible => "compatible",
    }
}
