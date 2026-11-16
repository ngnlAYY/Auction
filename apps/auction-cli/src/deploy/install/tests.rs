use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};

use uuid::Uuid;

use super::restore::Swap;
use super::trust::PinnedInstallDir;
use super::{
    FORMAT, InstallArtifactFailure, LEGACY_FORMAT, ROLE_BINARIES, RetainRequest, RetainedIdentity,
    RollbackRecord, install_artifact, install_retained, latest, load, record_dir, retain,
};
use crate::deploy::artifact::{Artifact, ArtifactBinary, REQUIRED_BINARIES};
use crate::receipt::{CliIdentity, DeployRecord, RoleIdentity};
use crate::support::digest::{sha256_bytes, sha256_path};
use crate::support::files::effective_uid;

struct TempRoot(PathBuf);

impl TempRoot {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("auction-deploy-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).expect("建立临时目录");
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).expect("设置权限");
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
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).expect("设置权限");
    for name in REQUIRED_BINARIES {
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
    for name in REQUIRED_BINARIES {
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

/// 以未知身份保留 `programs`；deploy 传四个程序，rollback 只传三个角色。
fn retain_programs(
    state: &Path,
    install: &Path,
    programs: &'static [&'static str],
) -> RollbackRecord {
    let request = RetainRequest {
        deploy_receipt_id: Uuid::new_v4(),
        install_dir: install,
        programs,
        observed_database_version: 94,
        installed_role_schema_version: Some(96),
    };
    retain(state, &request, |_| Ok(RetainedIdentity { roles: RoleIdentity::default(), cli: None }))
        .expect("保留旧程序")
}

fn retain_all(state: &Path, install: &Path) -> RollbackRecord {
    retain_programs(state, install, &REQUIRED_BINARIES)
}

/// 安装后的程序必须归当前账号（root 运行时为 root:root）且权限恰为 0755。
fn assert_sealed(install: &Path, name: &str) {
    let metadata = fs::symlink_metadata(install.join(name)).expect("程序元数据");
    assert!(metadata.is_file(), "{name}");
    assert_eq!(metadata.uid(), effective_uid(), "{name}");
    assert_eq!(metadata.mode() & 0o7777, 0o755, "{name}");
}

#[test]
fn retain_install_and_load_round_trip() {
    let root = TempRoot::new();
    let install = install_dir(root.path());
    let state = root.path().join("state");
    let record = retain_all(&state, &install);
    assert_eq!(record.format, FORMAT);
    assert_eq!(record.binaries.len(), 4);
    let (loaded, directory) = load(&state, record.record_id).expect("读取回滚记录");
    assert_eq!(loaded, record);
    assert_eq!(latest(&state).expect("最新记录").0, record);

    let artifact = artifact(root.path());
    install_artifact(&artifact, &install, &record, &directory).expect("安装制品程序");
    for binary in &artifact.binaries {
        let path = install.join(binary.name);
        assert_eq!(
            fs::read(&path).expect("读取安装程序"),
            format!("new-{}", binary.name).into_bytes()
        );
        assert_sealed(&install, binary.name);
    }
    // 回退只换回三个角色：先把当前三个角色保留为反向记录，auction-cli 保持制品版本。
    let reverse = retain_programs(&state, &install, &ROLE_BINARIES);
    assert_eq!(reverse.binaries.len(), 3);
    assert!(reverse.retained_cli.is_none());
    let reverse_directory = record_dir(&state, reverse.record_id);
    install_retained(&record, &directory, &install, &reverse, &reverse_directory)
        .expect("恢复旧角色程序");
    for name in ROLE_BINARIES {
        let (sha256, _) = sha256_path(&install.join(name)).expect("摘要");
        assert_eq!(Some(sha256.as_str()), record.binary(name).map(|binary| binary.sha256.as_str()));
        assert_sealed(&install, name);
    }
    assert_eq!(fs::read(install.join("auction-cli")).expect("CLI"), b"new-auction-cli");
}

#[test]
fn tampered_retained_binary_is_rejected() {
    let root = TempRoot::new();
    let install = install_dir(root.path());
    let state = root.path().join("state");
    let record = retain_all(&state, &install);
    let directory = record_dir(&state, record.record_id);
    fs::write(directory.join("bin/auction-cli"), b"tampered").expect("改写保留程序");
    assert!(load(&state, record.record_id).is_err());
}

/// v2 记录分开保存保留角色身份、保留 CLI 身份与观测版本；CLI 副本与身份摘要不一致、缺少角色
/// 或出现未知程序都失败关闭。
#[test]
fn record_validation_binds_identity_to_the_retained_copies() {
    let root = TempRoot::new();
    let install = install_dir(root.path());
    let state = root.path().join("state");
    let request = RetainRequest {
        deploy_receipt_id: Uuid::new_v4(),
        install_dir: &install,
        programs: &REQUIRED_BINARIES,
        observed_database_version: 94,
        installed_role_schema_version: Some(96),
    };
    let roles = RoleIdentity { schema_version: Some(94), ..RoleIdentity::default() };
    let foreign_cli = CliIdentity {
        sha256: "f".repeat(64),
        package: Some("auction-rust-0.1.0-ffffffffffff".to_owned()),
        source_commit: Some("f".repeat(40)),
        schema_version: Some(94),
    };
    let record = retain(&state, &request, |_| {
        Ok(RetainedIdentity { roles: roles.clone(), cli: Some(foreign_cli.clone()) })
    })
    .expect("保留旧程序");
    let cli = record.retained_cli.clone().expect("deploy 保留 CLI");
    assert_eq!(
        cli,
        CliIdentity::digest_only(&sha256_bytes(b"old-auction-cli")),
        "摘要不符不采用来源"
    );
    assert_eq!(record.retained_roles, roles);
    assert_eq!(record.target_schema(), (94, "recorded"));

    let mut invalid = record.clone();
    invalid.retained_cli = Some(CliIdentity::digest_only(&"e".repeat(64)));
    assert!(invalid.validate().is_err(), "CLI 身份必须等于副本摘要");
    let mut invalid = record.clone();
    invalid.retained_cli = None;
    assert!(invalid.validate().is_err(), "保留了 CLI 副本就必须有 CLI 身份");
    let mut invalid = record.clone();
    invalid.binaries.retain(|binary| binary.name != "auction-worker");
    assert!(invalid.validate().is_err(), "缺少角色程序");
    let mut invalid = record.clone();
    invalid.binaries[0].name = "auction-server".to_owned();
    assert!(invalid.validate().is_err(), "未知程序");
    let mut invalid = record;
    invalid.retained_roles.source_commit = Some("A".repeat(40));
    assert!(invalid.validate().is_err(), "角色身份无效");
}

/// v1 记录原样兼容读取：部署前的数据库版本只作观测值，角色 schema 记为未知（回退门禁按观测
/// 版本假设并标明来源），v1 的来源提交不进入身份；文件本身不改写。
#[test]
fn legacy_records_are_read_without_rewriting() {
    let root = TempRoot::new();
    let install = install_dir(root.path());
    let state = root.path().join("state");
    let record = retain_all(&state, &install);
    let directory = record_dir(&state, record.record_id);
    let legacy = serde_json::json!({
        "format": LEGACY_FORMAT,
        "record_id": record.record_id,
        "deploy_receipt_id": record.record_id,
        "created_at": record.created_at,
        "install_directory": record.install_directory,
        "previous_expected_version": 94,
        "deployed_expected_version": 96,
        "previous_source_commit": "e".repeat(40),
        "binaries": record.binaries,
    });
    let bytes = serde_json::to_vec_pretty(&legacy).expect("序列化");
    fs::remove_file(directory.join("record.json")).expect("移除 v2 记录");
    fs::write(directory.join("record.json"), &bytes).expect("写入 v1 记录");

    let (loaded, _) = load(&state, record.record_id).expect("读取 v1 记录");
    assert_eq!(loaded.format, LEGACY_FORMAT);
    assert_eq!(loaded.observed_database_version, 94);
    assert_eq!(loaded.installed_role_schema_version, Some(96));
    assert_eq!(loaded.retained_roles, RoleIdentity::default());
    assert_eq!(
        loaded.retained_cli,
        Some(CliIdentity::digest_only(&sha256_bytes(b"old-auction-cli")))
    );
    assert_eq!(loaded.target_schema(), (94, "assumed_observed_database_version"));
    assert_eq!(fs::read(directory.join("record.json")).expect("记录文件"), bytes, "不改写 v1 文件");

    let mut three = legacy;
    three["binaries"] = serde_json::to_value(&record.binaries[1..]).expect("三个程序");
    fs::write(directory.join("record.json"), serde_json::to_vec(&three).expect("序列化"))
        .expect("改写");
    assert!(load(&state, record.record_id).is_err(), "v1 记录必须有四个程序");
}

/// `rollback` 按 `RollbackRecord::deploy_receipt_id` 读回部署回执；该标识必须是
/// 保留旧程序时已分配、发布回执时实际使用的同一个标识。
#[test]
fn deploy_receipt_is_readable_from_retained_record_id() {
    use crate::receipt::{Action, Outcome, ReceiptContext, read as read_receipt, record_deploy};

    let root = TempRoot::new();
    let install = install_dir(root.path());
    let state = root.path().join("state");
    let record = retain_all(&state, &install);
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
        complete_deploy_record(&install, record.record_id),
        None,
    )
    .expect("写入部署回执");

    let published = read_receipt(&state, record.deploy_receipt_id).expect("按回滚记录读回部署回执");
    assert_eq!(published.receipt_id, record.record_id);
    assert_eq!(published.action, Action::Deploy);
    assert_eq!(published.outcome, Outcome::Succeeded);
    assert_eq!(published.deploy.expect("部署证据").rollback_record_id(), Some(record.record_id));
}

/// deploy 回执的完整 v2 deploy 段。
fn complete_deploy_record(install: &Path, rollback_record_id: Uuid) -> DeployRecord {
    let package = Some("auction-rust-0.1.0-aaaaaaaaaaaa".to_owned());
    let source_commit = Some("a".repeat(40));
    DeployRecord {
        install_directory: install.display().to_string(),
        roles: RoleIdentity {
            artifact_directory: Some("/srv/artifact".to_owned()),
            package: package.clone(),
            source_commit: source_commit.clone(),
            source_fingerprint: Some("b".repeat(64)),
            public_payload_sha256: Some("d".repeat(64)),
            admin_payload_sha256: Some("e".repeat(64)),
            schema_version: Some(96),
        },
        cli: CliIdentity {
            sha256: "c".repeat(64),
            package,
            source_commit,
            schema_version: Some(96),
        },
        database_version: 94,
        migration_drill_id: None,
        backup_archive: None,
        backup_manifest: None,
        rollback_record_id: Some(rollback_record_id),
    }
}

fn read_install(install: &Path, name: &str) -> Vec<u8> {
    fs::read(install.join(name)).expect("读取安装程序")
}

/// 第三个程序在 rename 之前失败（制品程序被改写、摘要不符）：已替换的 Worker 与 Admin 从回滚
/// 记录换回，四个程序复核为旧程序后才报告已恢复；换回的程序恢复为可信属主与 0755，原始原因
/// 保留在错误链末尾，不留下临时文件。
#[test]
fn install_failure_swaps_back_the_replaced_programs() {
    let root = TempRoot::new();
    let install = install_dir(root.path());
    let state = root.path().join("state");
    let artifact = artifact(root.path());
    let record = retain_all(&state, &install);
    let directory = record_dir(&state, record.record_id);
    // 等长改写使大小检查通过、摘要复核失败。
    fs::write(root.path().join("artifact/bin/auction-public-api"), b"NEW-auction-public-api")
        .expect("改写制品程序");

    let InstallArtifactFailure { error, restored } =
        install_artifact(&artifact, &install, &record, &directory).expect_err("第三个程序摘要不符");
    let text = format!("{error:#}");
    assert!(restored, "{text}");
    assert!(
        text.contains("已把本次替换的 auction-worker、auction-admin-api 从回滚记录换回"),
        "{text}"
    );
    assert!(text.ends_with("安装临时文件的摘要与期望不一致：auction-public-api"), "{text}");
    for name in REQUIRED_BINARIES {
        assert_eq!(read_install(&install, name), format!("old-{name}").into_bytes(), "{name}");
    }
    assert_sealed(&install, "auction-worker");
    assert_sealed(&install, "auction-admin-api");
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
/// 各程序的当前摘要；能换回的 Admin 仍被换回。
#[test]
fn failed_swap_back_reports_current_digests() {
    let root = TempRoot::new();
    let install = install_dir(root.path());
    let state = root.path().join("state");
    let artifact = artifact(root.path());
    let record = retain_all(&state, &install);
    let directory = record_dir(&state, record.record_id);
    fs::remove_file(directory.join("bin/auction-worker")).expect("删除保留的 Worker 程序");
    fs::write(root.path().join("artifact/bin/auction-public-api"), b"NEW-auction-public-api")
        .expect("改写制品程序");

    let InstallArtifactFailure { error, restored } =
        install_artifact(&artifact, &install, &record, &directory).expect_err("第三个程序摘要不符");
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
/// 复核通过；漏列时复核不一致，不报告已恢复。
#[test]
fn swap_back_restores_a_program_replaced_before_its_failure() {
    let root = TempRoot::new();
    let install = install_dir(root.path());
    let state = root.path().join("state");
    let record = retain_all(&state, &install);
    let directory = record_dir(&state, record.record_id);
    // 模拟 Worker 已 rename 为新程序、随后的同步或复核失败。
    fs::write(install.join("auction-worker"), b"new-auction-worker").expect("写入新程序");
    let pinned = PinnedInstallDir::open(&install).expect("固定安装目录");
    let swap = Swap { retained: &record, record_directory: &directory, directory: &pinned };

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
    assert_sealed(&install, "auction-worker");
}

/// rollback 的安装现场：安装目录已部署为新程序，`record` 保留部署前的四个旧程序，`reverse` 是
/// rollback 停机后刚保留的回退前三个角色程序。
struct RollbackSite {
    _root: TempRoot,
    install: PathBuf,
    record: RollbackRecord,
    record_directory: PathBuf,
    reverse: RollbackRecord,
    reverse_directory: PathBuf,
}

fn rollback_site() -> RollbackSite {
    let root = TempRoot::new();
    let install = install_dir(root.path());
    let state = root.path().join("state");
    let record = retain_all(&state, &install);
    let record_directory = record_dir(&state, record.record_id);
    install_artifact(&artifact(root.path()), &install, &record, &record_directory)
        .expect("部署新程序");
    let reverse = retain_programs(&state, &install, &ROLE_BINARIES);
    let reverse_directory = record_dir(&state, reverse.record_id);
    RollbackSite { _root: root, install, record, record_directory, reverse, reverse_directory }
}

impl RollbackSite {
    fn install_retained(&self) -> Result<(), InstallArtifactFailure> {
        install_retained(
            &self.record,
            &self.record_directory,
            &self.install,
            &self.reverse,
            &self.reverse_directory,
        )
    }
}

/// rollback 安装旧角色程序时第三个程序在 rename 之前失败（保留的旧程序被等长改写、摘要不符）：
/// 已替换的 Worker 与 Admin 从反向记录换回，三个角色复核为回退前的程序后才报告已恢复；
/// auction-cli 从未被触碰，原始原因保留在错误链末尾，不留下临时文件。
#[test]
fn retained_install_failure_swaps_back_from_the_reverse_record() {
    let site = rollback_site();
    fs::write(site.record_directory.join("bin/auction-public-api"), b"OLD-auction-public-api")
        .expect("改写保留的旧程序");

    let InstallArtifactFailure { error, restored } =
        site.install_retained().expect_err("第三个程序摘要不符");
    let text = format!("{error:#}");
    assert!(restored, "{text}");
    assert!(
        text.contains("已把本次替换的 auction-worker、auction-admin-api 从回滚记录换回"),
        "{text}"
    );
    assert!(text.contains("覆盖的 3 个程序摘要一致"), "{text}");
    assert!(text.ends_with("安装临时文件的摘要与期望不一致：auction-public-api"), "{text}");
    for name in REQUIRED_BINARIES {
        assert_eq!(read_install(&site.install, name), format!("new-{name}").into_bytes(), "{name}");
    }
    let entries = fs::read_dir(&site.install).expect("安装目录").count();
    assert_eq!(entries, REQUIRED_BINARIES.len(), "失败与换回都不留下临时文件");
}

/// rollback 换回本身失败（反向记录保留的 Worker 程序已丢失）：不声称已恢复，错误点名换回失败的
/// 程序并列出三个角色的当前摘要；能换回的 Admin 仍被换回为回退前的程序，CLI 保持不变。
#[test]
fn failed_retained_swap_back_reports_current_digests() {
    let site = rollback_site();
    fs::remove_file(site.reverse_directory.join("bin/auction-worker"))
        .expect("删除反向记录的 Worker 程序");
    fs::write(site.record_directory.join("bin/auction-public-api"), b"OLD-auction-public-api")
        .expect("改写保留的旧程序");

    let InstallArtifactFailure { error, restored } =
        site.install_retained().expect_err("第三个程序摘要不符");
    let text = format!("{error:#}");
    assert!(!restored, "{text}");
    assert!(text.contains("自动换回未完成") && text.contains("换回失败：auction-worker"), "{text}");
    assert!(text.contains("与回滚记录不一致：auction-worker"), "{text}");
    assert!(!text.contains("auction-cli="), "rollback 的复核不含 CLI：{text}");
    let old_worker = sha256_bytes(b"old-auction-worker");
    assert!(text.contains(&format!("auction-worker={old_worker}")), "{text}");
    assert_eq!(read_install(&site.install, "auction-worker"), b"old-auction-worker");
    assert_eq!(read_install(&site.install, "auction-admin-api"), b"new-auction-admin-api");
    assert_eq!(read_install(&site.install, "auction-cli"), b"new-auction-cli");
}

/// 安装目录不可信（group 可写）时在替换任何程序之前失败，安装目录保持原状。
#[test]
fn untrusted_install_dir_is_rejected_before_any_replacement() {
    let root = TempRoot::new();
    let install = install_dir(root.path());
    let state = root.path().join("state");
    let record = retain_all(&state, &install);
    fs::set_permissions(&install, fs::Permissions::from_mode(0o775)).expect("放宽安装目录");
    let InstallArtifactFailure { error, restored } = install_artifact(
        &artifact(root.path()),
        &install,
        &record,
        &record_dir(&state, record.record_id),
    )
    .expect_err("不可信安装目录");
    let text = format!("{error:#}");
    assert!(restored && text.contains("没有替换任何程序"), "{text}");
    for name in REQUIRED_BINARIES {
        assert_eq!(read_install(&install, name), format!("old-{name}").into_bytes(), "{name}");
    }
}

/// 安装中途失败（第三个目标被目录占住，rename 失败）：已替换的程序换回，但占位目录使安装目录
/// 与回滚记录不一致，因此不报告已恢复。失败回执以回滚记录的标识发布、记录实际文件摘要，回滚
/// 记录仍能读回这条回执，且只有这一条。
#[test]
fn install_failure_receipt_records_the_actual_install_dir() {
    use crate::deploy::receipts::FailedReceipt;
    use crate::deploy::verification::install_dir_artifacts;
    use crate::receipt::{Action, Outcome, ReceiptContext, read as read_receipt};

    let root = TempRoot::new();
    let install = install_dir(root.path());
    let state = root.path().join("state");
    let artifact = artifact(root.path());
    let record = retain_all(&state, &install);
    let blocked = install.join("auction-public-api");
    fs::remove_file(&blocked).expect("移除旧 Public 程序");
    fs::create_dir(&blocked).expect("以目录占住 Public 目标");
    fs::write(blocked.join("keep"), b"x").expect("目录非空");
    let InstallArtifactFailure { error, restored } =
        install_artifact(&artifact, &install, &record, &record_dir(&state, record.record_id))
            .expect_err("第三个程序无法替换");
    let text = format!("{error:#}");
    assert!(!restored, "{text}");
    assert!(text.contains("原子替换程序失败"), "{text}");
    assert!(text.contains("与回滚记录不一致：auction-public-api"), "{text}");
    assert!(text.contains("auction-public-api=不可读"), "{text}");

    let context = ReceiptContext::resolve(None, Some("operator")).expect("上下文");
    let deploy = complete_deploy_record(&install, record.record_id);
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
        let record = retain_all(&state, &install);
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
    let partial = record_dir(&state, Uuid::new_v4());
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
    let tied = retain_all(&state, &install);
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
fn record_rejects_inconsistent_identifiers() {
    let root = TempRoot::new();
    let install = install_dir(root.path());
    let state = root.path().join("state");
    let mut record = retain_all(&state, &install);
    record.deploy_receipt_id = Uuid::new_v4();
    assert!(record.validate().is_err());
    let mut record = retain_all(&state, &install);
    record.observed_database_version = -1;
    assert!(record.validate().is_err());
}
