//! 数据库之外的恢复范围：运行配置、加密邮件诊断证据与部署回执。
//!
//! 内容一律先打包成单一明文临时文件，再用 age 公钥加密为 `<archive>.scope.bundle.age`；
//! 明文在封存后立即删除。同目录的 `<archive>.scope.json` 只记录摘要、规模与相对路径，
//! 本身不含秘密，可随异机副本一起上传。

use std::{
    fs::{self, OpenOptions},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

use anyhow::Context;
use auction_config::AppConfig;
use chrono::Utc;
use serde::{Deserialize, Serialize};

use super::manifest::{sha256_bytes, sha256_file};
use crate::{backup::scheduled::filesystem::FileIdentity, support::files};

mod bundle;
mod collect;
mod crypto;
pub(crate) mod extract;
mod sidecar;

pub(crate) use sidecar::ScopeSidecar;

/// 写入备份回执的范围投影；只含摘要与规模，可在后续异机阶段核对原文件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ScopeSummary {
    pub(crate) sidecar_file_name: String,
    pub(crate) sidecar_sha256: String,
    pub(crate) sidecar_identity: FileIdentity,
    pub(crate) bundle_file_name: String,
    pub(crate) bundle_sha256: String,
    pub(crate) bundle_identity: FileIdentity,
    pub(crate) plaintext_sha256: String,
    pub(crate) plaintext_bytes: u64,
    pub(crate) files_count: u64,
}

pub(crate) struct VerifiedScope {
    pub(crate) sidecar: ScopeSidecar,
    pub(crate) sidecar_path: PathBuf,
    pub(crate) sidecar_sha256: String,
    pub(crate) bundle_path: PathBuf,
}

impl VerifiedScope {
    /// verify 输出的公开投影；不含来源绝对路径或文件内容。
    pub(crate) fn projection(&self) -> serde_json::Value {
        serde_json::json!({
            "sidecar": self.sidecar_path,
            "bundle_sha256": self.sidecar.bundle_sha256,
            "bundle_bytes": self.sidecar.bundle_bytes,
            "files_count": self.sidecar.files_count,
            "sources": sidecar::kinds(&self.sidecar.sources),
        })
    }
}

/// 按配置收集并加密范围证据；范围关闭或没有来源时返回 None。
pub(crate) async fn create(
    config: &AppConfig,
    archive: &Path,
) -> anyhow::Result<Option<ScopeSummary>> {
    let Some(plan) = collect::plan(config)? else {
        return Ok(None);
    };
    let archive_name = archive
        .file_name()
        .and_then(|name| name.to_str())
        .context("备份范围要求 archive 使用 UTF-8 文件名")?;
    let output_dir = archive.parent().context("备份 archive 缺少父目录")?;
    let recipient = config.backup.offsite.age_recipient.as_str();
    if recipient.is_empty() {
        anyhow::bail!("启用备份范围时必须配置 [backup.offsite].age_recipient");
    }
    crate::support::process::age_version().await?;
    let collected = collect::collect(&plan)?;
    let plaintext = output_dir.join(format!(".{archive_name}.scope.partial"));
    let mut guard = PlaintextGuard::new(plaintext.clone());
    let digest = {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&plaintext)
            .with_context(|| format!("创建范围暂存文件失败：{}", plaintext.display()))?;
        let digest = {
            let mut writer = std::io::BufWriter::new(&mut file);
            let digest = bundle::write(&mut writer, &collected.artifacts)?;
            std::io::Write::flush(&mut writer).context("同步范围暂存文件失败")?;
            digest
        };
        file.sync_all().context("同步范围暂存文件失败")?;
        digest
    };
    let bundle_path = output_dir.join(sidecar::bundle_file_name(archive_name));
    crypto::seal_file(&plaintext, &bundle_path, recipient).await?;
    guard.remove()?;
    let bundle_sha256 = sha256_file(&bundle_path).await?;
    let bundle_metadata = files::open_regular(&bundle_path)?.metadata()?;
    let listed = digest.entries.len().min(sidecar::MAX_LISTED_FILES);
    let sidecar = sidecar::ScopeSidecar {
        format: sidecar::FORMAT.to_owned(),
        created_at: Utc::now(),
        age_recipient_sha256: sha256_bytes(recipient.as_bytes()),
        bundle_file_name: sidecar::bundle_file_name(archive_name),
        bundle_sha256: bundle_sha256.clone(),
        bundle_bytes: bundle_metadata.len(),
        plaintext_sha256: digest.sha256.clone(),
        plaintext_bytes: digest.bytes,
        files_count: digest.entries.len() as u64,
        sources: collected.summaries,
        files: digest.entries[..listed].to_vec(),
        files_truncated: listed < digest.entries.len(),
    };
    let sidecar_path = output_dir.join(sidecar::sidecar_file_name(archive_name));
    let sidecar_bytes = sidecar.to_bytes()?;
    files::write_private(&sidecar_path, &sidecar_bytes)?;
    let sidecar_metadata = files::open_regular(&sidecar_path)?.metadata()?;
    Ok(Some(ScopeSummary {
        sidecar_file_name: sidecar::sidecar_file_name(archive_name),
        sidecar_sha256: sha256_bytes(&sidecar_bytes),
        sidecar_identity: FileIdentity::of(&sidecar_metadata),
        bundle_file_name: sidecar.bundle_file_name,
        bundle_sha256,
        bundle_identity: FileIdentity::of(&bundle_metadata),
        plaintext_sha256: sidecar.plaintext_sha256,
        plaintext_bytes: sidecar.plaintext_bytes,
        files_count: sidecar.files_count,
    }))
}

/// 离线校验：sidecar 字段闭集、配对名称、密文字节数与 SHA-256；不解密，不读取内容。
pub(crate) async fn verify(
    archive: &Path,
    archive_file_name: &str,
) -> anyhow::Result<Option<VerifiedScope>> {
    let sidecar_path = archive.with_file_name(sidecar::sidecar_file_name(archive_file_name));
    let (sidecar, sidecar_sha256) = match sidecar::read(&sidecar_path) {
        Ok(pair) => pair,
        Err(error) if is_not_found(&error) => return Ok(None),
        Err(error) => return Err(error),
    };
    let bundle_path = sidecar.validate(&sidecar_path, archive_file_name)?;
    let metadata = crypto::open_plain(&bundle_path)?.metadata()?;
    if metadata.len() != sidecar.bundle_bytes {
        anyhow::bail!("备份范围密文大小与 sidecar 不一致：{}", bundle_path.display());
    }
    if sha256_file(&bundle_path).await? != sidecar.bundle_sha256 {
        anyhow::bail!("备份范围密文 SHA-256 与 sidecar 不一致：{}", bundle_path.display());
    }
    Ok(Some(VerifiedScope { sidecar, sidecar_path, sidecar_sha256, bundle_path }))
}

/// sidecar 是否存在只由文件缺失区分；其他读取错误必须失败关闭。
pub(crate) fn is_not_found(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound)
    })
}

/// 明文暂存在任何退出路径上都被删除；封存成功后立即主动删除。
struct PlaintextGuard {
    path: PathBuf,
    armed: bool,
}

impl PlaintextGuard {
    fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }

    fn remove(&mut self) -> anyhow::Result<()> {
        if !self.armed {
            return Ok(());
        }
        match fs::remove_file(&self.path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("删除范围明文暂存失败：{}", self.path.display()));
            }
        }
        self.armed = false;
        files::sync_parent(&self.path)
    }
}

impl Drop for PlaintextGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        os::unix::fs::{DirBuilderExt, OpenOptionsExt},
        path::{Path, PathBuf},
    };

    use chrono::Utc;

    use super::{
        collect::{self, Plan, SourceKind, SourceSpec},
        sidecar::{self, ScopeSidecar},
        verify,
    };
    use crate::backup::manifest::{sha256_bytes, sha256_file};
    use crate::support::files;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("auction-scope-mod-{label}-{}", uuid::Uuid::new_v4()));
            fs::DirBuilder::new().recursive(true).mode(0o700).create(&path).expect("目录");
            Self(path)
        }

        fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
            let path = self.0.join(name);
            if let Some(parent) = path.parent() {
                fs::DirBuilder::new().recursive(true).mode(0o700).create(parent).expect("子目录");
            }
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
                .expect("文件");
            std::io::Write::write_all(&mut file, bytes).expect("写入");
            path
        }

        fn write_private(&self, name: &str, bytes: &[u8]) -> PathBuf {
            let path = self.0.join(name);
            files::write_private(&path, bytes).expect("私有文件");
            path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn summary(kind: SourceKind, files: u64, bytes: u64) -> collect::SourceSummary {
        collect::SourceSummary {
            kind,
            present: true,
            files,
            bytes,
            root_sha256: Some("e".repeat(64)),
        }
    }

    fn sidecar(bundle_path: &Path, bundle_bytes: u64) -> ScopeSidecar {
        let sha256 = file_sha256(bundle_path);
        ScopeSidecar {
            format: sidecar::FORMAT.to_owned(),
            created_at: Utc::now(),
            age_recipient_sha256: "a".repeat(64),
            bundle_file_name: sidecar::bundle_file_name("scheduled.dump"),
            bundle_sha256: sha256,
            bundle_bytes,
            plaintext_sha256: "b".repeat(64),
            plaintext_bytes: bundle_bytes - 1,
            files_count: 0,
            sources: vec![summary(SourceKind::RuntimeConfig, 0, 0)],
            files: Vec::new(),
            files_truncated: false,
        }
    }

    fn file_sha256(path: &Path) -> String {
        sha256_bytes(&fs::read(path).expect("读取"))
    }

    #[test]
    fn inclusion_covers_all_three_sources_without_absolute_paths() {
        let tree = TempDir::new("include");
        let role = tree.file("config/worker.toml", b"db = 1");
        let common = tree.file("config/common.toml", b"log = 2");
        tree.file("mail/a.json", b"header\nciphertext");
        tree.file("mail/nested/b.json", b"header\nciphertext");
        tree.file("receipts/deploy.json", b"{\"version\":1}");
        let plan = Plan {
            sources: vec![
                SourceSpec::Files { kind: SourceKind::RuntimeConfig, paths: vec![common, role] },
                SourceSpec::Directory {
                    kind: SourceKind::MailDiagnostics,
                    root: tree.0.join("mail"),
                    missing_ok: false,
                },
                SourceSpec::Directory {
                    kind: SourceKind::DeploymentReceipts,
                    root: tree.0.join("receipts"),
                    missing_ok: false,
                },
            ],
            max_files: 16,
            maximum_total_bytes: 1024 * 1024,
        };
        let collected = collect::collect(&plan).expect("收集");
        let paths: Vec<_> =
            collected.artifacts.iter().map(|artifact| artifact.path.as_str()).collect();
        assert_eq!(
            paths,
            vec![
                "runtime_config/common.toml",
                "runtime_config/worker.toml",
                "mail_diagnostics/a.json",
                "mail_diagnostics/nested/b.json",
                "deployment_receipts/deploy.json",
            ]
        );
        assert_eq!(collected.summaries.len(), 3);
        assert!(collected.summaries.iter().all(|source| source.present));
        assert!(
            collected
                .summaries
                .iter()
                .all(|source| source.root_sha256.is_none()
                    || source.kind != SourceKind::RuntimeConfig)
        );
        let bytes = serde_json::to_string(&collected.summaries).expect("序列化");
        assert!(!bytes.contains(&tree.0.to_string_lossy().to_string()));
        assert!(!bytes.contains("db = 1"));
    }

    fn tool_available(name: &str) -> bool {
        std::process::Command::new(name)
            .arg("--version")
            .output()
            .is_ok_and(|output| output.status.success())
    }

    #[tokio::test]
    async fn age_roundtrip_keeps_plaintext_off_disk_and_rejects_wrong_identity() {
        if !tool_available("age") || !tool_available("age-keygen") {
            eprintln!("跳过：本机没有 age/age-keygen");
            return;
        }
        let tree = TempDir::new("age");
        let identity = tree.0.join("identity.txt");
        let generated = std::process::Command::new("age-keygen")
            .arg("--output")
            .arg(&identity)
            .output()
            .expect("age-keygen");
        assert!(generated.status.success(), "age-keygen 失败");
        std::fs::set_permissions(&identity, std::os::unix::fs::PermissionsExt::from_mode(0o600))
            .expect("权限");
        let public = String::from_utf8_lossy(&generated.stderr);
        let recipient = public
            .split_whitespace()
            .find(|token| token.starts_with("age1"))
            .expect("公钥")
            .to_owned();

        let plaintext = tree.file("bundle.plain", b"{\"secret\":\"do-not-leak\"}");
        let sealed = tree.0.join("bundle.age");
        super::crypto::seal_file(&plaintext, &sealed, &recipient).await.expect("加密");
        let ciphertext = fs::read(&sealed).expect("读取密文");
        assert!(!ciphertext.windows(10).any(|window| window == b"do-not-leak"));
        // 输出目录里不能留下明文暂存。
        assert!(!tree.0.join(".bundle.age.partial").exists());

        let decrypted = tree.0.join("bundle.decrypted");
        super::crypto::decrypt_file(&identity, &sealed, &decrypted).await.expect("解密");
        assert_eq!(fs::read(&decrypted).expect("读取"), b"{\"secret\":\"do-not-leak\"}");
        assert!(!tree.0.join(".bundle.decrypted.partial").exists());

        // 用另一把私钥不能解开，且不能留下部分输出。
        let other = tree.0.join("other.txt");
        std::process::Command::new("age-keygen")
            .arg("--output")
            .arg(&other)
            .output()
            .expect("age-keygen");
        std::fs::set_permissions(&other, std::os::unix::fs::PermissionsExt::from_mode(0o600))
            .expect("权限");
        let wrong = tree.0.join("wrong.decrypted");
        assert!(super::crypto::decrypt_file(&other, &sealed, &wrong).await.is_err());
        assert!(!wrong.exists());
    }

    #[tokio::test]
    async fn verify_accepts_matching_pair_and_rejects_tampering() {
        let tree = TempDir::new("verify");
        let archive = tree.file("scheduled.dump", b"dump");
        let bundle = tree.file("scheduled.dump.scope.bundle.age", b"age-ciphertext-bytes");
        let bundle_bytes = fs::metadata(&bundle).expect("元数据").len();
        let record = sidecar(&bundle, bundle_bytes);
        let sidecar_path =
            tree.write_private("scheduled.dump.scope.json", &record.to_bytes().expect("序列化"));
        let verified = verify(&archive, "scheduled.dump").await.expect("校验").expect("存在");
        assert_eq!(verified.sidecar_sha256, sha256_file(&sidecar_path).await.expect("摘要"));
        assert_eq!(verified.bundle_path, bundle);

        // 篡改 sidecar 摘要后必须失败，不能只因为文件名配对就通过。
        let mut tampered = record.clone();
        tampered.bundle_sha256 = "f".repeat(64);
        fs::remove_file(&sidecar_path).expect("删除");
        tree.write_private("scheduled.dump.scope.json", &tampered.to_bytes().expect("序列化"));
        assert!(verify(&archive, "scheduled.dump").await.is_err());

        // 恢复正确 sidecar、改写密文后同样必须失败。
        fs::remove_file(&sidecar_path).expect("删除");
        tree.write_private("scheduled.dump.scope.json", &record.to_bytes().expect("序列化"));
        let mut file =
            fs::OpenOptions::new().write(true).truncate(true).open(&bundle).expect("打开");
        std::io::Write::write_all(&mut file, b"age-ciphertext-bytez").expect("改写");
        assert!(verify(&archive, "scheduled.dump").await.is_err());

        // 没有 sidecar 的旧恢复点按无范围处理。
        fs::remove_file(&sidecar_path).expect("删除");
        assert!(verify(&archive, "scheduled.dump").await.expect("校验").is_none());
    }
}
