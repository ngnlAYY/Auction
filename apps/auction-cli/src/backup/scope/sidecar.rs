//! 备份范围 sidecar：只记录摘要、规模与相对路径，绝不记录绝对来源目录或文件内容。

use std::path::{Path, PathBuf};

use anyhow::Context;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{
    bundle,
    collect::{MAX_RELATIVE_BYTES, SourceSummary, validate_relative},
};
use crate::support::files;

pub(crate) const FORMAT: &str = "auction-backup-scope.v1";
pub(crate) const SIDECAR_SUFFIX: &str = ".scope.json";
pub(crate) const BUNDLE_SUFFIX: &str = ".scope.bundle.age";
/// sidecar 只保留有界文件列表，完整逐文件摘要以打包文件为准。
pub(crate) const MAX_LISTED_FILES: usize = 256;
const MAX_SIDECAR_BYTES: u64 = 256 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ScopeSidecar {
    pub(crate) format: String,
    pub(crate) created_at: DateTime<Utc>,
    /// 收件人公钥的摘要；不含公钥本身。
    pub(crate) age_recipient_sha256: String,
    pub(crate) bundle_file_name: String,
    pub(crate) bundle_sha256: String,
    pub(crate) bundle_bytes: u64,
    pub(crate) plaintext_sha256: String,
    pub(crate) plaintext_bytes: u64,
    pub(crate) files_count: u64,
    pub(crate) sources: Vec<SourceSummary>,
    pub(crate) files: Vec<bundle::EntryDigest>,
    pub(crate) files_truncated: bool,
}

impl ScopeSidecar {
    /// 名称与字段闭集校验；返回与 archive 同目录的密文打包文件。
    pub(crate) fn validate(
        &self,
        sidecar_path: &Path,
        archive_file_name: &str,
    ) -> anyhow::Result<PathBuf> {
        if self.format != FORMAT
            || self.bundle_file_name != bundle_file_name(archive_file_name)
            || !is_sha256(&self.bundle_sha256)
            || !is_sha256(&self.plaintext_sha256)
            || !is_sha256(&self.age_recipient_sha256)
            || self.bundle_bytes == 0
            || self.plaintext_bytes == 0
            || self.bundle_bytes <= self.plaintext_bytes
            || self.sources.is_empty()
            || self.files.len() > MAX_LISTED_FILES
            || self.files_truncated && self.files.len() != MAX_LISTED_FILES
        {
            anyhow::bail!("备份范围 sidecar 的格式、摘要或规模字段无效");
        }
        if sidecar_path.file_name().and_then(|name| name.to_str())
            != Some(sidecar_file_name(archive_file_name).as_str())
        {
            anyhow::bail!("备份范围 sidecar 文件名必须与 archive 配对");
        }
        let mut kinds = Vec::new();
        let mut total_files = 0_u64;
        for source in &self.sources {
            if kinds.contains(&source.kind) {
                anyhow::bail!("备份范围 sidecar 重复记录来源类型");
            }
            kinds.push(source.kind);
            total_files = total_files.checked_add(source.files).context("备份范围文件数溢出")?;
        }
        if total_files != self.files_count {
            anyhow::bail!("备份范围 sidecar 的文件计数不一致");
        }
        for entry in &self.files {
            validate_relative(&entry.path)?;
            if entry.path.len() > MAX_RELATIVE_BYTES
                || !entry.path.starts_with(entry.kind.slug())
                || !is_sha256(&entry.sha256)
            {
                anyhow::bail!("备份范围 sidecar 的文件条目无效");
            }
        }
        Ok(sidecar_path.with_file_name(self.bundle_file_name.clone()))
    }

    pub(crate) fn to_bytes(&self) -> anyhow::Result<Vec<u8>> {
        serde_json::to_vec_pretty(self).context("序列化备份范围 sidecar 失败")
    }
}

pub(crate) fn read(path: &Path) -> anyhow::Result<(ScopeSidecar, String)> {
    let bytes = files::read_bounded(path, MAX_SIDECAR_BYTES)?;
    let sidecar: ScopeSidecar =
        serde_json::from_slice(&bytes).context("解析备份范围 sidecar 失败")?;
    Ok((sidecar, crate::backup::manifest::sha256_bytes(&bytes)))
}

pub(crate) fn sidecar_file_name(archive_file_name: &str) -> String {
    format!("{archive_file_name}{SIDECAR_SUFFIX}")
}

pub(crate) fn bundle_file_name(archive_file_name: &str) -> String {
    format!("{archive_file_name}{BUNDLE_SUFFIX}")
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// 只投影保留的类型名，供 CLI JSON 输出；不包含路径或摘要。
pub(crate) fn kinds(sources: &[SourceSummary]) -> Vec<&'static str> {
    sources.iter().map(|source| source.kind.slug()).collect()
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};

    use chrono::Utc;

    use super::{
        MAX_LISTED_FILES, ScopeSidecar, bundle_file_name, read, sidecar_file_name,
        validate_relative,
    };
    use crate::backup::scope::{
        bundle::EntryDigest,
        collect::{SourceKind, SourceSummary},
    };

    fn sidecar() -> ScopeSidecar {
        ScopeSidecar {
            format: super::FORMAT.to_owned(),
            created_at: Utc::now(),
            age_recipient_sha256: "a".repeat(64),
            bundle_file_name: bundle_file_name("scheduled.dump"),
            bundle_sha256: "b".repeat(64),
            bundle_bytes: 4096,
            plaintext_sha256: "c".repeat(64),
            plaintext_bytes: 2048,
            files_count: 1,
            sources: vec![SourceSummary {
                kind: SourceKind::RuntimeConfig,
                present: true,
                files: 1,
                bytes: 2048,
                root_sha256: None,
            }],
            files: vec![EntryDigest {
                kind: SourceKind::RuntimeConfig,
                path: "runtime_config/worker.toml".to_owned(),
                bytes: 2048,
                sha256: "d".repeat(64),
            }],
            files_truncated: false,
        }
    }

    #[test]
    fn valid_sidecar_binds_archive_names() {
        let sidecar = sidecar();
        assert!(
            sidecar
                .validate(std::path::Path::new("/b/scheduled.dump.scope.json"), "scheduled.dump")
                .is_ok()
        );
        assert!(
            sidecar
                .validate(std::path::Path::new("/b/other.dump.scope.json"), "scheduled.dump")
                .is_err()
        );
        assert_eq!(sidecar_file_name("scheduled.dump"), "scheduled.dump.scope.json");
    }

    #[test]
    fn sidecar_never_contains_source_paths_or_contents() {
        let bytes = sidecar().to_bytes().expect("序列化");
        let text = String::from_utf8(bytes).expect("UTF-8");
        assert!(!text.contains("/etc/auction"));
        assert!(!text.contains("password"));
        assert!(text.contains("runtime_config/worker.toml"));
    }

    #[test]
    fn invalid_counts_and_limits_are_rejected() {
        let mut broken = sidecar();
        broken.files_count = 2;
        assert!(
            broken
                .validate(std::path::Path::new("/b/scheduled.dump.scope.json"), "scheduled.dump")
                .is_err()
        );
        let mut truncated = sidecar();
        truncated.files_truncated = true;
        assert!(
            truncated
                .validate(std::path::Path::new("/b/scheduled.dump.scope.json"), "scheduled.dump")
                .is_err()
        );
        let mut too_many = sidecar();
        too_many.files = vec![too_many.files[0].clone(); MAX_LISTED_FILES + 1];
        assert!(
            too_many
                .validate(std::path::Path::new("/b/scheduled.dump.scope.json"), "scheduled.dump")
                .is_err()
        );
    }

    #[test]
    fn path_traversal_is_rejected() {
        for path in [
            "../escape",
            "/absolute",
            "runtime_config/../x",
            "runtime_config//x",
            "mail_diagnostics/x",
        ] {
            let mut broken = sidecar();
            broken.files[0].path = path.to_owned();
            broken.files[0].kind = SourceKind::RuntimeConfig;
            assert!(
                broken
                    .validate(
                        std::path::Path::new("/b/scheduled.dump.scope.json"),
                        "scheduled.dump"
                    )
                    .is_err(),
                "{path} 应被拒绝"
            );
        }
    }

    #[test]
    fn bounded_reader_rejects_oversized_files() {
        let directory =
            std::env::temp_dir().join(format!("auction-sidecar-{}", uuid::Uuid::new_v4()));
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&directory).expect("目录");
        let path = directory.join("big.json");
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .expect("文件");
        std::io::Write::write_all(&mut file, &vec![b'{'; 256 * 1024 + 1]).expect("写入");
        assert!(read(&path).is_err());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn relative_path_validation_is_strict() {
        for ok in ["runtime_config/worker.toml", "mail_diagnostics/a/b.json"] {
            assert!(validate_relative(ok).is_ok(), "{ok}");
        }
        for bad in ["", ".", "..", "a//b", "a/./b", "a/../b", "/a", "a\\b", "a/\u{7}"] {
            assert!(validate_relative(bad).is_err(), "{bad:?}");
        }
    }
}
