//! 制品安装中途失败后的自动换回：只把本次已替换的程序从刚保留的回滚记录装回，再按记录复核
//! 安装目录中的全部四个程序。换回发生在迁移之前，不启动服务；换回失败时不重试，错误列出各
//! 程序的当前摘要，交由人工按 RECOVERY 核对。

use std::{fs, path::Path};

use anyhow::Context;

use super::{InstallArtifactFailure, RollbackRecord, install_one};
use crate::support::digest::sha256_path;

/// 换回所需的回滚记录与安装目标。
pub(super) struct Swap<'a> {
    pub(super) retained: &'a RollbackRecord,
    pub(super) record_directory: &'a Path,
    pub(super) install_dir: &'a Path,
    pub(super) owner_uid: u32,
}

impl Swap<'_> {
    /// 逆序换回 `replaced` 中的程序，再复核四个程序；原始安装错误保留在错误链末尾。只有四个
    /// 程序都与回滚记录一致时才报告已恢复，否则不声称安装目录一致。
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
            let current = current_digest(&self.install_dir.join(&binary.name));
            let matches = current
                .as_ref()
                .is_some_and(|(sha256, bytes)| *sha256 == binary.sha256 && *bytes == binary.bytes);
            if !matches {
                mismatched.push(binary.name.as_str());
            }
            digests.push(format!(
                "{}={}",
                binary.name,
                current.map_or_else(|| "不可读".to_owned(), |(sha256, _)| sha256)
            ));
        }
        if failures.is_empty() && mismatched.is_empty() {
            let swapped = if replaced.is_empty() {
                "本次没有程序被替换".to_owned()
            } else {
                format!("已把本次替换的 {} 从回滚记录换回", replaced.join("、"))
            };
            return InstallArtifactFailure {
                error: error.context(format!("{swapped}，安装目录四个程序与回滚记录的摘要一致")),
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
            &self.record_directory.join("bin").join(name),
            &self.install_dir.join(name),
            &binary.sha256,
            binary.bytes,
            self.owner_uid,
        )
        .map_err(|failure| failure.error)
    }
}

/// 安装目录中程序的当前摘要；不是非符号链接普通文件或读不到时为 `None`，不伪造身份。
fn current_digest(path: &Path) -> Option<(String, u64)> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    sha256_path(path).ok()
}
