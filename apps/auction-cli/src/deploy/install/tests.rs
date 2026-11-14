use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use super::{FORMAT, RollbackRecord, install_artifact, latest, load, retain};
use crate::deploy::artifact::{Artifact, ArtifactBinary, sha256_bytes};

struct TempRoot(PathBuf);

impl TempRoot {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("auction-deploy-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).expect("建立临时目录");
        Self(root)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn install_dir(root: &Path) -> PathBuf {
    let directory = root.join("install");
    fs::create_dir_all(&directory).expect("建立安装目录");
    for name in crate::deploy::artifact::REQUIRED_BINARIES {
        let path = directory.join(name);
        fs::write(&path, format!("old-{name}")).expect("写入旧程序");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("设置权限");
    }
    directory
}

fn artifact(root: &Path) -> Artifact {
    let binaries_dir = root.join("artifact/bin");
    fs::create_dir_all(&binaries_dir).expect("建立制品目录");
    let mut binaries = Vec::new();
    for name in crate::deploy::artifact::REQUIRED_BINARIES {
        let path = binaries_dir.join(name);
        fs::write(&path, format!("new-{name}")).expect("写入新程序");
        binaries.push(ArtifactBinary {
            name,
            path,
            sha256: sha256_bytes(format!("new-{name}").as_bytes()),
            bytes: format!("new-{name}").len() as u64,
        });
    }
    Artifact {
        root: root.join("artifact"),
        package: "auction-rust-0.1.0-aaaaaaaaaaaa".to_owned(),
        source_commit: "a".repeat(40),
        source_fingerprint: "b".repeat(64),
        binaries,
        public_payload_sha256: "c".repeat(64),
        admin_payload_sha256: "d".repeat(64),
    }
}

#[test]
fn retain_install_and_load_round_trip() {
    let root = TempRoot::new();
    let install = install_dir(root.path());
    let state = root.path().join("state");
    let receipt_id = uuid::Uuid::new_v4();
    let record =
        retain(&state, receipt_id, &install, 94, 96, Some(&"e".repeat(40))).expect("保留旧程序");
    assert_eq!(record.format, FORMAT);
    let (loaded, directory) = load(&state, receipt_id).expect("读取回滚记录");
    assert_eq!(loaded, record);
    assert_eq!(latest(&state).expect("最新记录").0, record);

    let artifact = artifact(root.path());
    install_artifact(&artifact, &install, crate::support::files::effective_uid())
        .expect("安装制品程序");
    for binary in &artifact.binaries {
        let path = install.join(binary.name);
        assert_eq!(
            fs::read(&path).expect("读取安装程序"),
            format!("new-{}", binary.name).into_bytes()
        );
    }
    // 回退复制保留的旧程序后必须与记录摘要一致。
    super::install_retained(&record, &directory, &install, crate::support::files::effective_uid())
        .expect("恢复旧程序");
    for binary in &record.binaries {
        let (sha256, _) =
            crate::deploy::artifact::sha256_path(&install.join(&binary.name)).expect("摘要");
        assert_eq!(sha256, binary.sha256);
    }
}

#[test]
fn tampered_retained_binary_is_rejected() {
    let root = TempRoot::new();
    let install = install_dir(root.path());
    let state = root.path().join("state");
    let receipt_id = uuid::Uuid::new_v4();
    retain(&state, receipt_id, &install, 94, 96, None).expect("保留旧程序");
    let directory = state.join("rollback").join(receipt_id.to_string());
    fs::write(directory.join("bin/auction-cli"), b"tampered").expect("改写保留程序");
    assert!(load(&state, receipt_id).is_err());
}

/// `rollback` 按 `RollbackRecord::deploy_receipt_id` 读回部署回执；该标识必须是
/// 保留旧程序时已分配、发布回执时实际使用的同一个标识。
#[test]
fn deploy_receipt_is_readable_from_retained_record_id() {
    use crate::receipt::{
        Action, DeployRecord, Outcome, ReceiptContext, read as read_receipt, record_deploy,
    };

    let root = TempRoot::new();
    let install = install_dir(root.path());
    let state = root.path().join("state");
    let receipt_id = uuid::Uuid::new_v4();
    let record = retain(&state, receipt_id, &install, 94, 96, None).expect("保留旧程序");
    let context = ReceiptContext::resolve(Some("backup-1"), Some("operator")).expect("上下文");
    record_deploy(
        &state,
        record.deploy_receipt_id,
        Action::Deploy,
        Outcome::Succeeded,
        chrono::Utc::now(),
        &context,
        None,
        Vec::new(),
        DeployRecord {
            artifact_directory: "/srv/artifact".to_owned(),
            package: "auction-rust-0.1.0-aaaaaaaaaaaa".to_owned(),
            source_commit: "a".repeat(40),
            source_fingerprint: "b".repeat(64),
            cli_sha256: "c".repeat(64),
            install_directory: install.display().to_string(),
            public_payload_sha256: "d".repeat(64),
            admin_payload_sha256: "e".repeat(64),
            migration_drill_id: None,
            backup_archive: None,
            backup_manifest: None,
            rollback_record_id: Some(record.record_id),
        },
        None,
    )
    .expect("写入部署回执");

    let published = read_receipt(&state, record.deploy_receipt_id).expect("按回滚记录读回部署回执");
    assert_eq!(published.receipt_id, record.record_id);
    assert_eq!(published.action, Action::Deploy);
    assert_eq!(published.outcome, Outcome::Succeeded);
    assert_eq!(published.deploy.expect("部署证据").rollback_record_id, Some(record.record_id));
}

#[test]
fn failed_deploy_receipt_is_readable_with_installed_artifacts() {
    use crate::receipt::{
        Action, DeployRecord, Outcome, ReceiptContext, read as read_receipt, record_deploy,
    };

    let root = TempRoot::new();
    let install = install_dir(root.path());
    let state = root.path().join("state");
    let artifact = artifact(root.path());
    let record_id = uuid::Uuid::new_v4();
    let record = retain(&state, record_id, &install, 94, 96, None).expect("保留旧程序");
    let artifacts =
        crate::deploy::verification::installed_artifacts(&artifact).expect("读取静态制品回执");
    let context = ReceiptContext::resolve(None, Some("operator")).expect("上下文");
    record_deploy(
        &state,
        record.deploy_receipt_id,
        Action::Deploy,
        Outcome::Failed,
        chrono::Utc::now(),
        &context,
        None,
        artifacts,
        DeployRecord {
            artifact_directory: artifact.root.display().to_string(),
            package: artifact.package.clone(),
            source_commit: artifact.source_commit.clone(),
            source_fingerprint: artifact.source_fingerprint.clone(),
            cli_sha256: artifact.cli_sha256().to_owned(),
            install_directory: install.display().to_string(),
            public_payload_sha256: artifact.public_payload_sha256.clone(),
            admin_payload_sha256: artifact.admin_payload_sha256.clone(),
            migration_drill_id: None,
            backup_archive: None,
            backup_manifest: None,
            rollback_record_id: Some(record.record_id),
        },
        Some("startup failed".to_owned()),
    )
    .expect("写入失败部署回执");

    let published =
        read_receipt(&state, record.deploy_receipt_id).expect("回滚记录仍能读回失败回执");
    assert_eq!(published.outcome, Outcome::Failed);
    assert_eq!(published.artifacts.len(), 3);
    assert_eq!(published.deploy.expect("部署证据").cli_sha256, artifact.cli_sha256());
}

#[test]
fn record_rejects_inconsistent_versions() {
    let root = TempRoot::new();
    let install = install_dir(root.path());
    let mut record = RollbackRecord {
        format: FORMAT.to_owned(),
        record_id: uuid::Uuid::new_v4(),
        deploy_receipt_id: uuid::Uuid::nil(),
        created_at: chrono::Utc::now(),
        install_directory: install.display().to_string(),
        previous_expected_version: 94,
        deployed_expected_version: 96,
        previous_source_commit: None,
        binaries: Vec::new(),
    };
    record.deploy_receipt_id = record.record_id;
    assert!(record.validate().is_err());
}
