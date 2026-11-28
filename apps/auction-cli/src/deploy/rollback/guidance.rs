//! 回退在停机后失败时的处置说明：错误文本写明当时安装目录与数据库的状态，并只给出在该状态下
//! 安全的命令。rollback 从不替换 auction-cli，停机前又已核对它与数据库同版本，因此安装目录
//! 仍是回退前的程序时，用它执行 `service start` 不会迁移，只会重新启动回退前的角色程序。

use std::path::Path;

use super::super::install::{self, InstallRecoveryOutcome, RollbackRecord};
use super::super::receipts::Failure;

/// 回退停机后、安装前的失败：安装目录仍是回退前的程序，数据库未改变，处置见
/// [`pre_rollback_recovery`]。反向记录目录可能已部分写出：没有 `record.json` 时省略
/// `--receipt-id` 的回退会跳过它，已写出时会选中它并因没有对应的部署回执而拒绝，因此一并点名
/// 并指引显式重试。
pub(super) fn stopped_before_install(
    error: anyhow::Error,
    install_dir: &Path,
    database_version: i64,
    record: &RollbackRecord,
    reverse_directory: &Path,
) -> Failure {
    Failure::unrecorded(error.context(format!(
        "回退在停机后、安装前中止：三角色已停止，{} 中仍是回退前的程序，数据库未改变（版本 \
         {database_version}）；{}。本次的反向回滚记录目录 {} 若已部分写出，它没有对应的\
         部署回执，不能作为回退目标：重试时显式带 --receipt-id（省略时只会跳过缺少 record.json \
         的残留目录），人工核对后整目录删除它",
        install_dir.display(),
        pre_rollback_recovery(install_dir, record),
        reverse_directory.display()
    )))
}

/// 安装旧角色程序中途失败：数据库未改变，本次已替换的程序已尝试从反向回滚记录换回。换回并
/// 复核成功时安装目录仍是回退前的程序，恢复指引与停机后、安装前的失败相同。反向记录已带失败
/// 回执（实际摘要就是当前程序，安装目录核对会通过），保留的也正是当前程序：省略 `--receipt-id`
/// 会选中它并因目标与当前程序相同而拒绝，因此重试必须显式指定原记录。换回未完成时安装目录可能
/// 新旧混合，只给出人工换回的入口，不给启动或重试命令。
pub(super) fn install_failed(
    failure: install::InstallArtifactFailure,
    install_dir: &Path,
    database_version: i64,
    record: &RollbackRecord,
    reverse_directory: &Path,
) -> anyhow::Error {
    match failure.outcome {
        InstallRecoveryOutcome::Restored | InstallRecoveryOutcome::UntouchedVerified => {
            failure.error.context(format!(
                "安装旧角色程序失败：三角色已停止，{} 中的三个角色程序已复核为回退前的可信程序，\
                 auction-cli 未被替换，数据库未改变（版本 {database_version}）；{}。本次的反向回滚记录 {}\
                 带失败回执、保留的就是当前程序，省略 --receipt-id 的 rollback 会选中它并因目标与当前程序相同而拒绝",
                install_dir.display(),
                pre_rollback_recovery(install_dir, record),
                reverse_directory.display()
            ))
        }
        InstallRecoveryOutcome::UntouchedUnverified => failure.error.context(format!(
            "安装旧角色程序失败：三角色已停止，{} 中没有替换任何程序，但安装目录及程序身份未能复核；\
             数据库未改变（版本 {database_version}）。不要启动服务；按 ops/deploy/RECOVERY.md 的\
             “重建安装目录信任”核对并修复后再处置",
            install_dir.display()
        )),
        InstallRecoveryOutcome::Mixed => failure.error.context(format!(
            "安装旧角色程序失败且自动换回未完成：三角色已停止，数据库未改变（版本 {database_version}），\
             auction-cli 未被替换；{} 中的角色程序可能新旧混合，回退前的角色程序完整保留在 {}（摘要见 \
             {}）。按 ops/deploy/RECOVERY.md 的“安装中途失败的手工换回”核对摘要并换回后再处置（原\
             回滚记录 {}）；换回前不要启动服务",
            install_dir.display(),
            reverse_directory.join("bin").display(),
            reverse_directory.join(install::RECORD_FILE).display(),
            record.record_id
        )),
    }
}

/// 安装目录仍是回退前的程序、数据库未改变时的处置：保留的 CLI 已在停机前核对为与数据库同版本，
/// `service start` 的迁移是空操作，只重新启动回退前的角色程序；或修复原因后以原记录重试回退。
fn pre_rollback_recovery(install_dir: &Path, record: &RollbackRecord) -> String {
    let cli = install_dir.join("auction-cli");
    format!(
        "恢复原状请用 {} service start --owner-secrets <迁移 owner 凭据>（沿用本次的 --config-dir、\
         --state-dir 与 unit 参数；保留的 CLI 与数据库同版本，迁移为空操作），或修复原因后用 {} \
         rollback --receipt-id {} 重试回退（rollback 只接受安装目录的 CLI，沿用本次的 \
         --config-dir、--state-dir 与 unit 参数）",
        cli.display(),
        cli.display(),
        record.record_id
    )
}
