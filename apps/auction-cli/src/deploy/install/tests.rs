use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use super::restore::Swap;
use super::{
    FORMAT, InstallArtifactFailure, RollbackRecord, install_artifact, latest, load, record_dir,
    retain,
};
use crate::deploy::artifact::{Artifact, ArtifactBinary, REQUIRED_BINARIES};
use crate::support::digest::sha256_bytes;
use crate::support::files::effective_uid;

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
    install_artifact(&artifact, &install, effective_uid(), &record, &directory)
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
            crate::support::digest::sha256_path(&install.join(&binary.name)).expect("摘要");
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

fn read_install(install: &Path, name: &str) -> Vec<u8> {
    fs::read(install.join(name)).expect("读取安装程序")
}

/// 第三个程序在 rename 之前失败（制品程序被改写、摘要不符）：已替换的 Worker 与 Admin 从回滚
/// 记录换回，四个程序复核为旧程序后才报告已恢复；原始原因保留在错误链末尾，不留下临时文件。
#[test]
fn install_failure_swaps_back_the_replaced_programs() {
    let root = TempRoot::new();
    let install = install_dir(root.path());
    let state = root.path().join("state");
    let artifact = artifact(root.path());
    let record = retain(&state, uuid::Uuid::new_v4(), &install, 94, 96, None).expect("保留旧程序");
    let directory = record_dir(&state, record.record_id);
    // 等长改写使大小检查通过、摘要复核失败。
    fs::write(root.path().join("artifact/bin/auction-public-api"), b"NEW-auction-public-api")
        .expect("改写制品程序");

    let InstallArtifactFailure { error, restored } =
        install_artifact(&artifact, &install, effective_uid(), &record, &directory)
            .expect_err("第三个程序摘要不符");
    let text = format!("{error:#}");
    assert!(restored, "{text}");
    assert!(
        text.contains("已把本次替换的 auction-worker、auction-admin-api 从回滚记录换回"),
        "{text}"
    );
    assert!(text.ends_with("安装临时文件的摘要与制品不一致：auction-public-api"), "{text}");
    for name in REQUIRED_BINARIES {
        assert_eq!(read_install(&install, name), format!("old-{name}").into_bytes(), "{name}");
    }
    let mut entries = fs::read_dir(&install)
        .expect("安装目录")
        .map(|entry| entry.expect("目录项").file_name().into_string().expect("文件名"))
        .collect::<Vec<_>>();
    entries.sort_unstable();
    let mut expected = REQUIRED_BINARIES.map(str::to_owned).to_vec();
    expected.sort_unstable();
    assert_eq!(entries, expected, "失败与换回都不留下临时文件");
}

/// 换回本身失败（保留的 Worker 程序已丢失）：不声称已恢复，错误点名换回失败的程序并列出
/// 四个程序的当前摘要；能换回的 Admin 仍被换回。
#[test]
fn failed_swap_back_reports_current_digests() {
    let root = TempRoot::new();
    let install = install_dir(root.path());
    let state = root.path().join("state");
    let artifact = artifact(root.path());
    let record = retain(&state, uuid::Uuid::new_v4(), &install, 94, 96, None).expect("保留旧程序");
    let directory = record_dir(&state, record.record_id);
    fs::remove_file(directory.join("bin/auction-worker")).expect("删除保留的 Worker 程序");
    fs::write(root.path().join("artifact/bin/auction-public-api"), b"NEW-auction-public-api")
        .expect("改写制品程序");

    let InstallArtifactFailure { error, restored } =
        install_artifact(&artifact, &install, effective_uid(), &record, &directory)
            .expect_err("第三个程序摘要不符");
    let text = format!("{error:#}");
    assert!(!restored, "{text}");
    assert!(text.contains("自动换回未完成") && text.contains("换回失败：auction-worker"), "{text}");
    assert!(text.contains("与回滚记录不一致：auction-worker"), "{text}");
    let new_worker = sha256_bytes(b"new-auction-worker");
    assert!(text.contains(&format!("auction-worker={new_worker}")), "{text}");
    assert_eq!(read_install(&install, "auction-worker"), b"new-auction-worker");
    assert_eq!(read_install(&install, "auction-admin-api"), b"old-auction-admin-api");
}

/// rename 之后才失败的程序（目标已是新程序）必须列入换回：列入时 `Swap::back` 把它装回旧程序并
/// 复核通过；漏列时四个程序复核不一致，不报告已恢复。
#[test]
fn swap_back_restores_a_program_replaced_before_its_failure() {
    let root = TempRoot::new();
    let install = install_dir(root.path());
    let state = root.path().join("state");
    let record = retain(&state, uuid::Uuid::new_v4(), &install, 94, 96, None).expect("保留旧程序");
    let directory = record_dir(&state, record.record_id);
    // 模拟 Worker 已 rename 为新程序、随后的同步或复核失败。
    fs::write(install.join("auction-worker"), b"new-auction-worker").expect("写入新程序");
    let swap = Swap {
        retained: &record,
        record_directory: &directory,
        install_dir: &install,
        owner_uid: effective_uid(),
    };

    let omitted = swap.back(anyhow::anyhow!("安装后复核失败"), &[]);
    let text = format!("{:#}", omitted.error);
    assert!(!omitted.restored, "{text}");
    assert!(text.contains("与回滚记录不一致：auction-worker"), "{text}");

    let InstallArtifactFailure { error, restored } =
        swap.back(anyhow::anyhow!("安装后复核失败"), &["auction-worker"]);
    let text = format!("{error:#}");
    assert!(restored, "{text}");
    assert!(text.contains("已把本次替换的 auction-worker 从回滚记录换回"), "{text}");
    assert!(text.ends_with("安装后复核失败"), "{text}");
    assert_eq!(read_install(&install, "auction-worker"), b"old-auction-worker");
}

/// 安装中途失败（第三个目标被目录占住，rename 失败）：已替换的程序换回，但占位目录使安装目录
/// 与回滚记录不一致，因此不报告已恢复。失败回执以回滚记录的标识发布、记录实际文件摘要，回滚
/// 记录仍能读回这条回执，且只有这一条。
#[test]
fn install_failure_receipt_records_the_actual_install_dir() {
    use crate::deploy::receipts::FailedReceipt;
    use crate::deploy::verification::install_dir_artifacts;
    use crate::receipt::{Action, DeployRecord, Outcome, ReceiptContext, read as read_receipt};

    let root = TempRoot::new();
    let install = install_dir(root.path());
    let state = root.path().join("state");
    let artifact = artifact(root.path());
    let record = retain(&state, uuid::Uuid::new_v4(), &install, 94, 96, None).expect("保留旧程序");
    let blocked = install.join("auction-public-api");
    fs::remove_file(&blocked).expect("移除旧 Public 程序");
    fs::create_dir(&blocked).expect("以目录占住 Public 目标");
    fs::write(blocked.join("keep"), b"x").expect("目录非空");
    let InstallArtifactFailure { error, restored } = install_artifact(
        &artifact,
        &install,
        effective_uid(),
        &record,
        &record_dir(&state, record.record_id),
    )
    .expect_err("第三个程序无法替换");
    let text = format!("{error:#}");
    assert!(!restored, "{text}");
    assert!(text.contains("原子替换程序失败"), "{text}");
    assert!(text.contains("与回滚记录不一致：auction-public-api"), "{text}");
    assert!(text.contains("auction-public-api=不可读"), "{text}");

    let context = ReceiptContext::resolve(None, Some("operator")).expect("上下文");
    let deploy = DeployRecord {
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
    };
    let failed = FailedReceipt {
        state_dir: &state,
        receipt_id: record.deploy_receipt_id,
        action: Action::Deploy,
        started_at: chrono::Utc::now(),
        context: &context,
        deploy: &deploy,
    };
    let failure = failed.publish(error, None, Ok(install_dir_artifacts(&install)));
    assert!(failure.recorded);

    let published = read_receipt(&state, record.deploy_receipt_id).expect("按回滚记录读回失败回执");
    assert_eq!(published.outcome, Outcome::Failed);
    let digest = |role: &str| {
        published
            .artifacts
            .iter()
            .find(|artifact| artifact.role == role)
            .and_then(|artifact| artifact.sha256.clone())
    };
    assert_eq!(digest("worker"), Some(sha256_bytes(b"old-auction-worker")), "Worker 已换回");
    assert_eq!(digest("admin"), Some(sha256_bytes(b"old-auction-admin-api")), "Admin 已换回");
    assert_eq!(digest("public"), None, "无法读取的程序不伪造摘要");
    assert_eq!(fs::read_dir(state.join("receipts")).expect("回执目录").count(), 1);
    assert_eq!(latest(&state).expect("最新回滚记录").0.record_id, record.record_id);
}

/// 按指定创建时间改写 record.json，使“最新”不依赖测试执行时的时钟分辨率。
fn set_created_at(state: &Path, record: &RollbackRecord, minutes: i64) -> RollbackRecord {
    let mut updated = record.clone();
    updated.created_at = chrono::DateTime::from_timestamp(1_790_000_000, 0).expect("固定时间")
        + chrono::Duration::minutes(minutes);
    let path = record_dir(state, record.record_id).join("record.json");
    fs::write(&path, serde_json::to_vec_pretty(&updated).expect("序列化")).expect("改写记录");
    updated
}

/// latest 先只读各候选的 record.json 选出最新，再只散列选中的记录：旧记录缺少保留程序、
/// 保留中途失败留下的无 record.json 目录都不再阻断默认回退；record.json 损坏、创建时间并列或
/// 选中记录被篡改时点名失败。
#[test]
fn latest_reads_records_first_and_hashes_only_the_selection() {
    let root = TempRoot::new();
    let install = install_dir(root.path());
    let state = root.path().join("state");
    let mut records = Vec::new();
    for minutes in 0..3 {
        let record =
            retain(&state, uuid::Uuid::new_v4(), &install, 94, 96, None).expect("保留旧程序");
        records.push(set_created_at(&state, &record, minutes));
    }
    // 最旧记录的保留程序被人工删除：不影响选择最新记录。
    fs::remove_file(record_dir(&state, records[0].record_id).join("bin/auction-cli"))
        .expect("删除旧记录的程序");
    let (selected, directory) = latest(&state).expect("最新记录");
    assert_eq!(selected, records[2]);
    assert_eq!(directory, record_dir(&state, records[2].record_id));
    assert!(load(&state, records[0].record_id).is_err(), "显式读取残缺记录仍失败关闭");

    // 保留中途失败留下的目录没有 record.json：从未成为回退目标，跳过后仍选出最新记录。
    let partial = record_dir(&state, uuid::Uuid::new_v4());
    fs::create_dir_all(partial.join("bin")).expect("残缺目录");
    fs::write(partial.join("bin/auction-worker"), b"partial").expect("残缺程序");
    assert_eq!(latest(&state).expect("跳过残缺目录").0, records[2]);

    // record.json 存在但损坏：不能确定它是否更新，点名该目录并提示改用 --receipt-id。
    fs::write(partial.join("record.json"), b"{").expect("损坏的记录");
    let text = format!("{:#}", latest(&state).expect_err("损坏记录必须失败关闭"));
    assert!(
        text.contains(&partial.display().to_string()) && text.contains("--receipt-id"),
        "{text}"
    );
    fs::remove_dir_all(&partial).expect("移除残缺目录");

    // 两条记录的创建时间并列时无法确定最新一条。
    let tied = retain(&state, uuid::Uuid::new_v4(), &install, 94, 96, None).expect("保留旧程序");
    set_created_at(&state, &tied, 2);
    let text = format!("{:#}", latest(&state).expect_err("并列必须拒绝"));
    assert!(text.contains("创建时间相同"), "{text}");
    fs::remove_dir_all(record_dir(&state, tied.record_id)).expect("移除并列记录");

    // 选中的最新记录被篡改时仍失败关闭。
    fs::write(record_dir(&state, records[2].record_id).join("bin/auction-cli"), b"tampered")
        .expect("篡改最新记录");
    assert!(latest(&state).is_err());
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
