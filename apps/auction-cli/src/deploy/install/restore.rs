//! 安装中途失败后的自动换回：只把本次已替换的程序从刚保留的回滚记录（deploy 是部署前的四个
//! 程序，rollback 是回退前三个角色的反向记录）装回，再按该记录复核它覆盖的全部程序；rollback
//! 从不替换 auction-cli，也不把它列入复核。换回经同一个固定的安装目录描述符完成，保留程序经
//! 回滚记录目录的固定描述符读取，发生在迁移与启动服务之前；换回失败时不重试，错误列出各程序的
//! 当前摘要，交由人工按 RECOVERY 核对。

use anyhow::Context;

use super::trust::PinnedInstallDir;
use super::{InstallArtifactFailure, RecordDirectory, RollbackRecord, Source, install_one};

/// 换回所需的回滚记录与固定的安装目录。
pub(super) struct Swap<'a> {
    pub(super) retained: &'a RollbackRecord,
    pub(super) record_directory: &'a RecordDirectory,
    pub(super) directory: &'a PinnedInstallDir,
}

impl Swap<'_> {
    /// 逆序换回 `replaced` 中的程序，再复核记录覆盖的全部程序（摘要，以及换回过的程序的属主与
    /// 权限）；原始安装错误保留在错误链末尾。只有这些程序都与回滚记录一致时才报告已恢复。
    pub(super) fn back(&self, error: anyhow::Error, replaced: &[&str]) -> InstallArtifactFailure {
        let mut failures = Vec::new();
        for name in replaced.iter().rev() {
            if let Err(restore_error) = self.restore(name) {
                failures.push(format!("{name}：{restore_error:#}"));
            }
        }
        let mut mismatched = Vec::new();
        let mut digests = Vec::with_capacity(self.retained.binaries.len());
        for binary in &self.retained.binaries {
            let current = self.directory.inspect(&binary.name).ok();
            let matches = current.as_ref().is_some_and(|current| {
                current.sha256 == binary.sha256 && current.bytes == binary.bytes
            });
            // 换回过的程序还必须恢复为可信属主与 0755；未替换的程序保持原样。
            let sealed = !replaced.contains(&binary.name.as_str())
                || current.as_ref().is_some_and(|current| current.sealed);
            if !matches || !sealed {
                mismatched.push(binary.name.as_str());
            }
            digests.push(format!(
                "{}={}",
                binary.name,
                current.map_or_else(|| "不可读".to_owned(), |current| current.sha256)
            ));
        }
        if failures.is_empty() && mismatched.is_empty() {
            let swapped = if replaced.is_empty() {
                "本次没有程序被替换".to_owned()
            } else {
                format!("已把本次替换的 {} 从回滚记录换回", replaced.join("、"))
            };
            return InstallArtifactFailure {
                error: error.context(format!(
                    "{swapped}，安装目录中回滚记录覆盖的 {} 个程序摘要一致",
                    self.retained.binaries.len()
                )),
                restored: true,
            };
        }
        let mut details = Vec::new();
        if !failures.is_empty() {
            details.push(format!("换回失败：{}", failures.join("；")));
        }
        if !mismatched.is_empty() {
            details.push(format!("与回滚记录不一致：{}", mismatched.join("、")));
        }
        InstallArtifactFailure {
            error: error.context(format!(
                "自动换回未完成（{}）；安装目录各程序当前摘要：{}",
                details.join("；"),
                digests.join("、")
            )),
            restored: false,
        }
    }

    fn restore(&self, name: &str) -> anyhow::Result<()> {
        let binary = self.retained.binary(name).with_context(|| format!("回滚记录缺少 {name}"))?;
        install_one(
            self.directory,
            &Source::Retained(self.record_directory),
            name,
            &binary.sha256,
            binary.bytes,
        )
        .map_err(|failure| failure.error)
    }
}
