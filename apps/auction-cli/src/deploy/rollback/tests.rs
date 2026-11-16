use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use auction_application::operations::MigrationCompatibilityMode;
use auction_config::Role;
use chrono::Utc;
use uuid::Uuid;

use super::guidance::{install_failed, stopped_before_install};
use super::identity::{kept_cli_identity, replaced_roles_identity, rollback_deploy_record};
use super::installed_retained_artifacts;
use super::preconditions::{
    COMPATIBLE_READINESS_SINCE, require_cli_matches_database, require_compatible_readiness,
    require_installed_matches,
};
use crate::deploy::artifact::{Artifact, ArtifactBinary, REQUIRED_BINARIES};
use crate::deploy::install::{
    FORMAT, InstallArtifactFailure, ROLE_BINARIES, RetainRequest, RetainedBinary, RetainedIdentity,
    RollbackRecord, install_artifact, install_retained, record_dir, retain,
};
use crate::deploy::receipts::{installed_identity, roles_match};
use crate::receipt::{
    Action, Artifact as ReceiptArtifact, CliIdentity, DeployRecord, DeploySection,
    DeploymentReceipt, FORMAT as RECEIPT_FORMAT, LEGACY_FORMAT as LEGACY_RECEIPT_FORMAT,
    LegacyDeployRecord, Outcome, ReceiptContext, RoleIdentity, read as read_receipt, record_deploy,
};
use crate::support::digest::{sha256_bytes, sha256_path};

fn role_identity(tag: char, schema: i64) -> RoleIdentity {
    RoleIdentity {
        artifact_directory: Some(format!("/opt/auction-run/{tag}")),
        package: Some(format!("auction-rust-0.1.{schema}-{}", tag.to_string().repeat(12))),
        source_commit: Some(tag.to_string().repeat(40)),
        source_fingerprint: Some(tag.to_string().repeat(64)),
        public_payload_sha256: Some(tag.to_string().repeat(64)),
        admin_payload_sha256: Some(tag.to_string().repeat(64)),
        schema_version: Some(schema),
    }
}

fn record(retained_roles: RoleIdentity) -> RollbackRecord {
    let mut binaries =
        vec![RetainedBinary { name: "auction-cli".to_owned(), sha256: "b".repeat(64), bytes: 1 }];
    for name in ROLE_BINARIES {
        binaries.push(RetainedBinary { name: name.to_owned(), sha256: "c".repeat(64), bytes: 2 });
    }
    let id = Uuid::new_v4();
    RollbackRecord {
        format: FORMAT.to_owned(),
        record_id: id,
        deploy_receipt_id: id,
        created_at: Utc::now(),
        install_directory: "/opt/auction/bin".to_owned(),
        observed_database_version: 1,
        installed_role_schema_version: Some(2),
        retained_roles,
        retained_cli: Some(CliIdentity::digest_only(&"b".repeat(64))),
        binaries,
    }
}

fn receipt(action: Action, deploy: DeploySection) -> DeploymentReceipt {
    DeploymentReceipt {
        format: RECEIPT_FORMAT.to_owned(),
        receipt_id: Uuid::new_v4(),
        action,
        outcome: Outcome::Succeeded,
        started_at: Utc::now(),
        finished_at: Utc::now(),
        actor: "operator".to_owned(),
        actor_uid: 0,
        backup_id: None,
        migration: None,
        artifacts: Vec::new(),
        failure: None,
        deploy: Some(deploy),
    }
}

fn deploy_record(roles: RoleIdentity, cli: CliIdentity) -> DeployRecord {
    DeployRecord {
        install_directory: "/opt/auction/bin".to_owned(),
        roles,
        cli,
        database_version: 1,
        migration_drill_id: Some("drill-1".to_owned()),
        backup_archive: Some("/var/backups/auction/a.dump".to_owned()),
        backup_manifest: Some("/var/backups/auction/a.dump.manifest".to_owned()),
        rollback_record_id: Some(Uuid::new_v4()),
    }
}

/// 回退回执的身份只取自目标组件：角色身份来自回滚记录（未知项保持 null），CLI 是被保留的
/// 实际程序，摘要相同时才沿用部署回执的来源；不带入原部署的演练与恢复点。
#[test]
fn rollback_receipt_takes_identity_from_the_target_components() {
    let mut target = role_identity('a', 94);
    target.package = None;
    let retained = record(target.clone());
    let deployed_cli = CliIdentity {
        sha256: "e".repeat(64),
        package: Some("auction-rust-0.1.96-eeeeeeeeeeee".to_owned()),
        source_commit: Some("e".repeat(40)),
        schema_version: Some(96),
    };
    let deployed = receipt(
        Action::Deploy,
        DeploySection::Current(deploy_record(role_identity('e', 96), deployed_cli.clone())),
    );

    let cli = kept_cli_identity(&deployed, &"e".repeat(64), 96);
    assert_eq!(cli, deployed_cli, "摘要相同时沿用来源，schema 取自实际程序");
    let reverse_id = Uuid::new_v4();
    let rollback =
        rollback_deploy_record(&retained, Path::new("/opt/auction/bin"), 96, cli, reverse_id);
    assert_eq!(rollback.roles, target, "角色身份来自回滚记录，未知项不从新包补齐");
    assert_eq!(rollback.cli.package.as_deref(), Some("auction-rust-0.1.96-eeeeeeeeeeee"));
    assert_eq!(rollback.database_version, 96);
    assert_eq!(rollback.rollback_record_id, Some(reverse_id));
    assert!(rollback.migration_drill_id.is_none());
    assert!(rollback.backup_archive.is_none() && rollback.backup_manifest.is_none());
    receipt(Action::Rollback, DeploySection::Current(rollback)).validate().expect("回退回执有效");

    let other = kept_cli_identity(&deployed, &"f".repeat(64), 96);
    assert_eq!(
        other,
        CliIdentity { schema_version: Some(96), ..CliIdentity::digest_only(&"f".repeat(64)) }
    );
}

/// 被替换的角色身份来自部署回执描述的已安装程序；v1 rollback 回执的身份混有被回退版本，
/// 记为未知，schema 用回滚记录登记的同一次操作所安装的版本。
#[test]
fn replaced_roles_identity_comes_from_the_installed_programs() {
    let retained = record(RoleIdentity::default());
    let deployed = receipt(
        Action::Deploy,
        DeploySection::Current(deploy_record(
            role_identity('e', 96),
            CliIdentity::digest_only(&"e".repeat(64)),
        )),
    );
    assert_eq!(replaced_roles_identity(&deployed, &retained), role_identity('e', 96));

    let legacy = DeploySection::Legacy(LegacyDeployRecord {
        artifact_directory: "/srv/artifact".to_owned(),
        package: "auction-rust-0.1.0-ffffffffffff".to_owned(),
        source_commit: "f".repeat(40),
        source_fingerprint: "f".repeat(64),
        cli_sha256: "b".repeat(64),
        install_directory: "/opt/auction/bin".to_owned(),
        public_payload_sha256: "f".repeat(64),
        admin_payload_sha256: "f".repeat(64),
        migration_drill_id: None,
        backup_archive: None,
        backup_manifest: None,
        rollback_record_id: Some(Uuid::new_v4()),
    });
    let legacy_rollback = DeploymentReceipt {
        format: LEGACY_RECEIPT_FORMAT.to_owned(),
        ..receipt(Action::Rollback, legacy)
    };
    legacy_rollback.validate().expect("v1 回退回执可读");
    let replaced = replaced_roles_identity(&legacy_rollback, &retained);
    assert_eq!(replaced, RoleIdentity { schema_version: Some(2), ..RoleIdentity::default() });
}

/// 同版本回退不要求任何就绪模式；数据库超前时三角色都必须显式 compatible，且目标程序须在
/// 迁移 075 之后，否则在停机前拒绝并点名不满足的角色。
#[test]
fn compatible_readiness_is_required_only_when_the_database_is_ahead() {
    let modes = |public, admin, worker| {
        [(Role::Public, public), (Role::Admin, admin), (Role::Worker, worker)]
    };
    use MigrationCompatibilityMode::{Compatible, Shadow, Strict};
    let same = require_compatible_readiness(&modes(Shadow, Strict, Shadow), 96, 96)
        .expect("同版本允许 strict/shadow");
    assert_eq!(same["compatible_required"], false);

    let error = require_compatible_readiness(&modes(Compatible, Shadow, Compatible), 96, 98)
        .expect_err("任一角色非 compatible 拒绝");
    let text = format!("{error:#}");
    assert!(text.contains("admin=shadow") && !text.contains("public="), "{text}");
    assert!(text.contains("不修改配置"), "{text}");

    let passed = require_compatible_readiness(&modes(Compatible, Compatible, Compatible), 96, 98)
        .expect("三角色 compatible 允许");
    assert_eq!(passed["compatible_required"], true);
    assert_eq!(passed["modes"]["worker"], "compatible");

    let old = COMPATIBLE_READINESS_SINCE - 1;
    let error = require_compatible_readiness(&modes(Compatible, Compatible, Compatible), old, 98)
        .expect_err("目标程序不支持兼容判定");
    assert!(format!("{error:#}").contains("恢复升级前备份"));
}

/// 保留的 CLI 必须与数据库同版本：较高（部署在迁移前失败）时回退后的 service start 会执行
/// 被回退的迁移，较低时无法管理数据库，两者都在停机前拒绝。
#[test]
fn kept_cli_must_match_the_database_version() {
    require_cli_matches_database(98, 98).expect("同版本允许");
    let ahead = format!("{:#}", require_cli_matches_database(98, 96).expect_err("CLI 超前拒绝"));
    assert!(ahead.contains("service start") && ahead.contains("手工换回"), "{ahead}");
    assert!(require_cli_matches_database(96, 98).is_err(), "CLI 落后拒绝");
}

/// 回退停机后、安装前的失败交给入口写通用回执；错误说明安装目录仍是回退前的程序、数据库未变，
/// 给出带 --owner-secrets 与 --state-dir 的恢复命令（保留的 CLI 与数据库同版本，迁移为空操作），
/// 并点名可能残缺的反向记录目录；原始原因保留在错误链末尾。
#[test]
fn stopped_before_install_describes_the_pre_rollback_state() {
    let retained = record(role_identity('a', 1));
    let failure = stopped_before_install(
        anyhow::anyhow!("磁盘已满").context("保留当前程序失败"),
        Path::new("/opt/auction/bin"),
        2,
        &retained,
        Path::new("/var/lib/auction/rollback/reverse"),
    );
    assert!(!failure.recorded);
    let text = format!("{:#}", failure.error);
    assert!(text.contains("/opt/auction/bin/auction-cli service start --owner-secrets"), "{text}");
    assert!(text.contains("--state-dir") && text.contains("数据库未改变（版本 2）"), "{text}");
    assert!(text.contains("迁移为空操作"), "{text}");
    assert!(text.contains(&format!("rollback --receipt-id {}", retained.record_id)), "{text}");
    assert!(text.contains("/var/lib/auction/rollback/reverse"), "{text}");
    assert!(text.ends_with("保留当前程序失败: 磁盘已满"), "{text}");
}

/// 安装旧角色程序中途失败的指引取决于自动换回：已换回并复核时说明三个角色仍是回退前的程序、
/// CLI 未被替换，并提醒重试须显式带原记录；未完成时只指向人工换回与反向记录，不给任何启动命令。
#[test]
fn install_failure_guidance_follows_the_swap_back_result() {
    let retained = record(role_identity('a', 1));
    let install_dir = Path::new("/opt/auction/bin");
    let reverse_directory = Path::new("/var/lib/auction/deployment/rollback/reverse");
    let failure = |restored| InstallArtifactFailure {
        error: anyhow::anyhow!("原子替换程序失败"),
        restored,
    };

    let swapped = install_failed(failure(true), install_dir, 2, &retained, reverse_directory);
    let text = format!("{swapped:#}");
    assert!(
        text.contains("已复核为回退前的程序") && text.contains("auction-cli 未被替换"),
        "{text}"
    );
    assert!(text.contains("/opt/auction/bin/auction-cli service start --owner-secrets"), "{text}");
    assert!(text.contains(&format!("rollback --receipt-id {}", retained.record_id)), "{text}");
    assert!(text.contains("/var/lib/auction/deployment/rollback/reverse"), "{text}");
    assert!(text.ends_with("原子替换程序失败"), "{text}");

    let mixed = install_failed(failure(false), install_dir, 2, &retained, reverse_directory);
    let text = format!("{mixed:#}");
    assert!(text.contains("自动换回未完成") && text.contains("可能新旧混合"), "{text}");
    assert!(text.contains("安装中途失败的手工换回"), "{text}");
    assert!(text.contains("/var/lib/auction/deployment/rollback/reverse/bin"), "{text}");
    assert!(text.contains(&retained.record_id.to_string()), "{text}");
    assert!(!text.contains("已复核为回退前的程序"), "{text}");
    assert!(!text.contains("service start"), "{text}");
    assert!(text.ends_with("原子替换程序失败"), "{text}");
}

#[test]
fn failed_rollback_receipt_uses_installed_retained_artifacts() {
    let artifacts =
        installed_retained_artifacts(&record(RoleIdentity::default()), Path::new("/opt/bin"))
            .expect("静态回执");
    assert_eq!(artifacts.len(), 3);
    assert!(artifacts.iter().all(|artifact| artifact.sha256.is_some()));
    assert!(artifacts.iter().all(|artifact| artifact.path.is_some()));
}

/// 一次 deploy/rollback 生命周期的本地现场：安装目录、状态目录与按版本命名的制品。
struct Site {
    root: PathBuf,
    install: PathBuf,
    state: PathBuf,
}

impl Site {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("auction-rollback-{}", Uuid::new_v4()));
        let install = root.join("bin");
        let state = root.join("state");
        for directory in [&root, &install] {
            fs::create_dir(directory).expect("建立目录");
            fs::set_permissions(directory, fs::Permissions::from_mode(0o755)).expect("设置权限");
        }
        for name in REQUIRED_BINARIES {
            write_program(&install.join(name), &format!("a-{name}"));
        }
        Self { root, install, state }
    }

    /// 版本 `tag` 的制品：四个程序内容为 `<tag>-<程序名>`。
    fn artifact(&self, tag: &str) -> Artifact {
        let bin = self.root.join(format!("artifact-{tag}/bin"));
        fs::create_dir_all(&bin).expect("建立制品目录");
        let binaries = REQUIRED_BINARIES
            .into_iter()
            .map(|name| {
                let content = format!("{tag}-{name}");
                write_program(&bin.join(name), &content);
                ArtifactBinary {
                    name,
                    path: bin.join(name),
                    sha256: sha256_bytes(content.as_bytes()),
                    bytes: content.len() as u64,
                }
            })
            .collect();
        Artifact {
            root: self.root.join(format!("artifact-{tag}")),
            package: format!("auction-rust-0.1.0-{}", tag.repeat(12)),
            source_commit: tag.repeat(40),
            source_fingerprint: tag.repeat(64),
            binaries,
            public_payload_sha256: tag.repeat(64),
            admin_payload_sha256: tag.repeat(64),
        }
    }

    fn installed(&self, name: &str) -> String {
        fs::read_to_string(self.install.join(name)).expect("读取安装程序")
    }

    /// 安装目录当前三个角色程序的运行回执证据（等价于 `/proc/<pid>/exe` 的摘要）。
    fn role_artifacts(&self) -> Vec<ReceiptArtifact> {
        crate::deploy::verification::install_dir_artifacts(&self.install)
    }

    /// 模拟 `deploy`：保留四个程序（身份按摘要从回执绑定）、安装制品并发布 v2 deploy 回执。
    fn deploy(&self, artifact: &Artifact, schema: i64, database_version: i64) -> RollbackRecord {
        let id = Uuid::new_v4();
        let request = RetainRequest {
            deploy_receipt_id: id,
            install_dir: &self.install,
            programs: &REQUIRED_BINARIES,
            observed_database_version: database_version,
            installed_role_schema_version: Some(schema),
        };
        let retained =
            retain(&self.state, &request, |binaries| Ok(installed_identity(&self.state, binaries)))
                .expect("保留旧程序");
        install_artifact(artifact, &self.install, &retained, &record_dir(&self.state, id))
            .expect("安装制品");
        let roles = RoleIdentity {
            artifact_directory: Some(artifact.root.display().to_string()),
            package: Some(artifact.package.clone()),
            source_commit: Some(artifact.source_commit.clone()),
            source_fingerprint: Some(artifact.source_fingerprint.clone()),
            public_payload_sha256: Some(artifact.public_payload_sha256.clone()),
            admin_payload_sha256: Some(artifact.admin_payload_sha256.clone()),
            schema_version: Some(schema),
        };
        let cli = CliIdentity {
            sha256: artifact.cli_sha256().to_owned(),
            package: Some(artifact.package.clone()),
            source_commit: Some(artifact.source_commit.clone()),
            schema_version: Some(schema),
        };
        let deploy = DeployRecord {
            install_directory: self.install.display().to_string(),
            roles,
            cli,
            database_version,
            migration_drill_id: None,
            backup_archive: None,
            backup_manifest: None,
            rollback_record_id: Some(id),
        };
        self.publish(id, Action::Deploy, deploy);
        retained
    }

    /// 模拟 `rollback`（不含 systemd 与数据库）：核对安装目录、保留三个角色为反向记录、换回
    /// 三个角色、核对 CLI 未变并发布 v2 回退回执。
    fn rollback(&self, record: &RollbackRecord, database_version: i64) -> RollbackRecord {
        let deployed = read_receipt(&self.state, record.deploy_receipt_id).expect("部署回执");
        require_installed_matches(&deployed, &self.install).expect("安装目录是部署版本");
        let (cli_before, _) = sha256_path(&self.install.join("auction-cli")).expect("CLI 摘要");
        let reverse_id = Uuid::new_v4();
        let cli = kept_cli_identity(&deployed, &cli_before, database_version);
        let deploy =
            rollback_deploy_record(record, &self.install, database_version, cli, reverse_id);
        let request = RetainRequest {
            deploy_receipt_id: reverse_id,
            install_dir: &self.install,
            programs: &ROLE_BINARIES,
            observed_database_version: database_version,
            installed_role_schema_version: record.retained_roles.schema_version,
        };
        let reverse = retain(&self.state, &request, |binaries| {
            assert!(roles_match(&deployed, binaries));
            Ok(RetainedIdentity { roles: replaced_roles_identity(&deployed, record), cli: None })
        })
        .expect("反向记录");
        install_retained(
            record,
            &record_dir(&self.state, record.record_id),
            &self.install,
            &reverse,
            &record_dir(&self.state, reverse_id),
        )
        .expect("换回三个角色");
        let (cli_after, _) = sha256_path(&self.install.join("auction-cli")).expect("CLI 摘要");
        assert_eq!(cli_after, cli_before, "rollback 不替换 auction-cli");
        self.publish(reverse_id, Action::Rollback, deploy);
        reverse
    }

    fn publish(&self, id: Uuid, action: Action, deploy: DeployRecord) -> DeploymentReceipt {
        let context = ReceiptContext::resolve(None, Some("operator")).expect("上下文");
        record_deploy(
            &self.state,
            id,
            action,
            Outcome::Succeeded,
            Utc::now(),
            &context,
            None,
            self.role_artifacts(),
            deploy,
            None,
        )
        .expect("发布回执")
    }

    fn current(&self, id: Uuid) -> DeployRecord {
        match read_receipt(&self.state, id).expect("回执").deploy.expect("deploy 段") {
            DeploySection::Current(record) => record,
            DeploySection::Legacy(_) => panic!("新回执必须是 v2"),
        }
    }
}

impl Drop for Site {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn write_program(path: &Path, content: &str) {
    fs::write(path, content).expect("写入程序");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("设置权限");
}

/// A→B→A→B：每条回滚记录与回执的角色身份、CLI 身份与观测版本都属于它描述的组件。回退只换
/// 三个角色、保留 B 的 CLI；再前进从反向记录换回 B 的角色；之后的 deploy 按程序摘要把 A 的
/// 身份（而不是 B 的 CLI 或数据库版本）记给被保留的角色。
#[test]
fn a_b_a_b_keeps_every_identity_with_its_own_component() {
    let site = Site::new();
    let a = site.artifact("a");
    let b = site.artifact("b");
    // 把初始程序登记为 A 的一次部署，使后续身份可以按摘要绑定。
    let initial = site.deploy(&a, 94, 94);
    assert_eq!(initial.retained_roles, RoleIdentity::default(), "没有任何回执时身份未知");
    assert_eq!(initial.retained_cli.as_ref().map(|cli| cli.package.clone()), Some(None));

    // A → B：保留 A 的四个程序，身份按摘要绑定到 A 的回执。
    let to_b = site.deploy(&b, 96, 94);
    assert_eq!(to_b.retained_roles.package.as_deref(), Some(a.package.as_str()));
    assert_eq!(to_b.retained_roles.schema_version, Some(94));
    assert_eq!(to_b.observed_database_version, 94);
    assert_eq!(to_b.installed_role_schema_version, Some(96));
    assert_eq!(
        to_b.retained_cli.as_ref().and_then(|cli| cli.package.as_deref()),
        Some(a.package.as_str())
    );
    assert_eq!(site.installed("auction-cli"), "b-auction-cli");

    // B → A：只换回三个角色，CLI 仍是 B；回执的角色身份属于 A、CLI 身份属于 B。
    let back = site.rollback(&to_b, 96);
    for name in ROLE_BINARIES {
        assert_eq!(site.installed(name), format!("a-{name}"));
    }
    assert_eq!(site.installed("auction-cli"), "b-auction-cli");
    assert_eq!(back.binaries.len(), 3, "反向记录只保留三个角色");
    assert!(back.retained_cli.is_none());
    assert_eq!(back.retained_roles.package.as_deref(), Some(b.package.as_str()));
    assert_eq!(back.installed_role_schema_version, Some(94));
    let receipt = site.current(back.record_id);
    assert_eq!(receipt.roles.package.as_deref(), Some(a.package.as_str()));
    assert_eq!(receipt.roles.schema_version, Some(94));
    assert_eq!(receipt.cli.sha256, b.cli_sha256());
    assert_eq!(receipt.cli.package.as_deref(), Some(b.package.as_str()));
    assert_eq!(receipt.database_version, 96);

    // A → B：再前进从反向记录换回 B 的角色，CLI 不变。
    let forward = site.rollback(&back, 96);
    for name in ROLE_BINARIES {
        assert_eq!(site.installed(name), format!("b-{name}"));
    }
    let receipt = site.current(forward.record_id);
    assert_eq!(receipt.roles.package.as_deref(), Some(b.package.as_str()));
    assert_eq!(receipt.cli.package.as_deref(), Some(b.package.as_str()));
    assert_eq!(forward.retained_roles.package.as_deref(), Some(a.package.as_str()));

    // 回到 A 后再部署 B：被保留的角色按摘要绑定到 A，CLI 按摘要绑定到 B，观测版本另记。
    let back_again = site.rollback(&forward, 96);
    let redeploy = site.deploy(&b, 96, 96);
    assert_eq!(redeploy.retained_roles.package.as_deref(), Some(a.package.as_str()));
    assert_eq!(redeploy.retained_roles.schema_version, Some(94));
    assert_eq!(
        redeploy.retained_cli.as_ref().and_then(|cli| cli.package.as_deref()),
        Some(b.package.as_str())
    );
    assert_eq!(redeploy.observed_database_version, 96);
    assert_ne!(back_again.record_id, redeploy.record_id);
}
