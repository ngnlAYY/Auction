use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use auction_application::operations::MigrationCompatibilityMode;
use auction_config::Role;
use auction_time::Timestamp;
use uuid::Uuid;

use super::guidance::{install_failed, stopped_before_install};
use super::identity::{kept_cli_identity, reverse_identity, rollback_deploy_record};
use super::installed_retained_artifacts;
use super::preconditions::{
    COMPATIBLE_READINESS_SINCE, require_cli_matches_database, require_compatible_readiness,
    require_installed_matches, require_known_schema_across_migrations,
    require_rollbackable_receipt, require_target_differs,
};
use crate::deploy::artifact::{Artifact, ArtifactBinary, REQUIRED_BINARIES};
use crate::deploy::install::{
    FORMAT, InstallArtifactFailure, InstallRecoveryOutcome, ROLE_BINARIES, RetainRequest,
    RetainedBinary, RollbackRecord, SchemaSource, install_artifact, install_retained, latest, load,
    retain,
};
use crate::deploy::receipts::{FailedReceipt, installed_identity};
use crate::deploy::role_binary;
use crate::deploy::verification::installed_artifacts;
use crate::receipt::{
    Action, Artifact as ReceiptArtifact, CliIdentity, DeployRecord, DeploySection,
    DeploymentReceipt, FORMAT as RECEIPT_FORMAT, LEGACY_FORMAT as LEGACY_RECEIPT_FORMAT,
    LegacyDeployRecord, MigrationRecord, Outcome, ReceiptContext, RoleIdentity, append,
    list as list_receipts, read as read_receipt, record_deploy,
};
use crate::service::ManagedRole;
use crate::support::digest::{sha256_bytes, sha256_path};
use crate::support::files::PrivateDirectory;

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
        created_at: Timestamp::now(),
        install_directory: "/opt/auction/bin".to_owned(),
        observed_database_version: 1,
        installed_role_schema_version: Some(2),
        retained_roles,
        retained_cli: Some(CliIdentity::digest_only(&"b".repeat(64))),
        binaries,
    }
}

fn receipt(action: Action, deploy: DeploySection) -> DeploymentReceipt {
    let migration = match (&action, &deploy) {
        (Action::Deploy, DeploySection::Current(record)) => {
            let before_version = record.database_version;
            let expected_version = record.roles.schema_version.unwrap_or(before_version);
            Some(MigrationRecord {
                before_version,
                after_version: expected_version,
                expected_version,
                applied: expected_version - before_version,
            })
        }
        _ => None,
    };
    DeploymentReceipt {
        format: RECEIPT_FORMAT.to_owned(),
        receipt_id: Uuid::new_v4(),
        action,
        outcome: Outcome::Succeeded,
        started_at: Timestamp::now(),
        finished_at: Timestamp::now(),
        actor: "operator".to_owned(),
        actor_uid: 0,
        backup_id: None,
        migration,
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

/// 三个角色程序内容为 `<tag>-<程序名>` 的保留副本与对应的回执程序摘要。
fn role_copies(tag: char) -> (Vec<RetainedBinary>, Vec<ReceiptArtifact>) {
    ManagedRole::START_ORDER
        .into_iter()
        .map(|role| {
            let name = role_binary(role);
            let content = format!("{tag}-{name}");
            let sha256 = sha256_bytes(content.as_bytes());
            let bytes = content.len() as u64;
            (
                RetainedBinary { name: name.to_owned(), sha256: sha256.clone(), bytes },
                ReceiptArtifact {
                    role: role.name().to_owned(),
                    path: Some(format!("/opt/auction/bin/{name}")),
                    sha256: Some(sha256),
                    bytes: Some(bytes),
                },
            )
        })
        .unzip()
}

/// 被替换的角色身份按保留副本的摘要从成功回执重新绑定：成功部署回执的身份被采用；同一组程序
/// 的失败回执只描述尝试安装的另一个目标，不被采用；只有失败回执描述过的程序身份未知，也不从
/// 回滚记录补 schema（旧实现直接取部署回执的 deploy 段）；v1 rollback 回执的身份混有被回退
/// 版本，记为未知，schema 用回滚记录登记的同一次操作所安装的版本。
#[test]
fn reverse_identity_binds_by_digest_from_successful_receipts_only() {
    let root = std::env::temp_dir().join(format!("auction-reverse-identity-{}", Uuid::new_v4()));
    fs::create_dir(&root).expect("建立目录");
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).expect("设置权限");
    let state = PrivateDirectory::open_trusted(&root, false).expect("固定").expect("存在");
    let retained = record(RoleIdentity::default());
    let (binaries, artifacts) = role_copies('e');

    let deployed_cli = CliIdentity {
        sha256: "e".repeat(64),
        package: Some("auction-rust-0.1.96-eeeeeeeeeeee".to_owned()),
        source_commit: Some("e".repeat(40)),
        schema_version: Some(96),
    };
    let deployed = DeploymentReceipt {
        artifacts: artifacts.clone(),
        ..receipt(
            Action::Deploy,
            DeploySection::Current(deploy_record(role_identity('e', 96), deployed_cli)),
        )
    };
    append(&state, &deployed).expect("成功部署回执");
    let identity = reverse_identity(&state, &deployed, &retained, &binaries).expect("身份");
    assert_eq!(identity.roles, role_identity('e', 96));
    assert!(identity.cli.is_none(), "反向记录不保留 CLI");

    let attempted = |artifacts: Vec<ReceiptArtifact>| DeploymentReceipt {
        outcome: Outcome::Failed,
        failure: Some("安装第三个程序失败".to_owned()),
        artifacts,
        ..receipt(
            Action::Rollback,
            DeploySection::Current(deploy_record(
                role_identity('f', 94),
                CliIdentity::digest_only(&"e".repeat(64)),
            )),
        )
    };
    let failed = attempted(artifacts);
    append(&state, &failed).expect("失败回执");
    let identity = reverse_identity(&state, &failed, &retained, &binaries).expect("身份");
    assert_eq!(identity.roles, role_identity('e', 96), "仍绑定到成功回执，不取失败回执的 f");

    let (other_binaries, other_artifacts) = role_copies('g');
    let failed_only = attempted(other_artifacts);
    append(&state, &failed_only).expect("失败回执");
    let identity =
        reverse_identity(&state, &failed_only, &retained, &other_binaries).expect("身份");
    assert_eq!(identity.roles, RoleIdentity::default(), "无法证明时为 null");
    assert!(
        reverse_identity(&state, &deployed, &retained, &other_binaries).is_err(),
        "保留副本与部署回执记录的程序不符时拒绝"
    );

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
    let (legacy_binaries, legacy_artifacts) = role_copies('h');
    let legacy_rollback = DeploymentReceipt {
        format: LEGACY_RECEIPT_FORMAT.to_owned(),
        artifacts: legacy_artifacts,
        ..receipt(Action::Rollback, legacy)
    };
    legacy_rollback.validate().expect("v1 回退回执可读");
    append(&state, &legacy_rollback).expect("v1 回退回执");
    let replaced = reverse_identity(&state, &legacy_rollback, &retained, &legacy_binaries)
        .expect("身份")
        .roles;
    assert_eq!(replaced, RoleIdentity { schema_version: Some(2), ..RoleIdentity::default() });
    fs::remove_dir_all(&root).expect("清理");
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

/// 目标 schema 未知（按创建记录时观测到的数据库版本假设）时只允许 `D = E`：数据库已迁移到更高
/// 版本时，即使区间都登记 compatible 也在停机前拒绝（旧 Worker 进入服务后立即领取任务，就绪
/// 失败不能证明没有副作用）；数据库落后同样拒绝；已按摘要绑定的 schema 交给区间与就绪门禁。
#[test]
fn assumed_schema_allows_only_the_same_database_version() {
    let unknown = record(RoleIdentity::default());
    let (target, source) = unknown.target_schema();
    assert_eq!((target, source), (1, SchemaSource::AssumedObservedDatabaseVersion));
    require_known_schema_across_migrations(target, source, 1).expect("同版本允许");
    let error = require_known_schema_across_migrations(target, source, 3).expect_err("跨迁移");
    let text = format!("{error:#}");
    assert!(text.contains("schema 未知") && text.contains("恢复升级前备份"), "{text}");
    assert!(require_known_schema_across_migrations(target, source, 0).is_err(), "数据库落后");

    let known = record(role_identity('a', 1));
    let (target, source) = known.target_schema();
    assert_eq!(source, SchemaSource::Recorded);
    require_known_schema_across_migrations(target, source, 3)
        .expect("已记录的 schema 不受这道门影响");
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
    let failure = |outcome| InstallArtifactFailure {
        error: anyhow::anyhow!("原子替换程序失败"),
        outcome,
    };

    let swapped = install_failed(
        failure(InstallRecoveryOutcome::Restored),
        install_dir,
        2,
        &retained,
        reverse_directory,
    );
    let text = format!("{swapped:#}");
    assert!(text.contains("auction-cli 未被替换"), "{text}");
    assert!(text.contains("/opt/auction/bin/auction-cli service start --owner-secrets"), "{text}");
    assert!(text.contains(&format!("rollback --receipt-id {}", retained.record_id)), "{text}");
    assert!(text.contains("/var/lib/auction/deployment/rollback/reverse"), "{text}");
    assert!(text.contains("因目标与当前程序相同而拒绝"), "{text}");
    assert!(!text.contains("安装目录核对处拒绝"), "安装目录核对会通过，不能这样指引：{text}");
    assert!(text.ends_with("原子替换程序失败"), "{text}");

    let mixed = install_failed(
        failure(InstallRecoveryOutcome::Mixed),
        install_dir,
        2,
        &retained,
        reverse_directory,
    );
    let text = format!("{mixed:#}");
    assert!(text.contains("自动换回未完成") && text.contains("可能新旧混合"), "{text}");
    assert!(text.contains("安装中途失败的手工换回"), "{text}");
    assert!(text.contains("/var/lib/auction/deployment/rollback/reverse/bin"), "{text}");
    assert!(text.contains(&retained.record_id.to_string()), "{text}");
    assert!(!text.contains("已复核为回退前的程序"), "{text}");
    assert!(!text.contains("service start"), "{text}");
    assert!(text.ends_with("原子替换程序失败"), "{text}");

    let unverified = install_failed(
        failure(InstallRecoveryOutcome::UntouchedUnverified),
        install_dir,
        2,
        &retained,
        reverse_directory,
    );
    let text = format!("{unverified:#}");
    assert!(text.contains("没有替换任何程序") && text.contains("身份未能复核"), "{text}");
    assert!(!text.contains("service start"), "{text}");
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

/// 一次 deploy/rollback 生命周期的本地现场：安装目录、按生产口径固定的状态目录与按版本命名的
/// 制品。
struct Site {
    root: PathBuf,
    install: PathBuf,
    state: PrivateDirectory,
}

impl Site {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("auction-rollback-{}", Uuid::new_v4()));
        let install = root.join("bin");
        for directory in [&root, &install] {
            fs::create_dir(directory).expect("建立目录");
            fs::set_permissions(directory, fs::Permissions::from_mode(0o755)).expect("设置权限");
        }
        for name in REQUIRED_BINARIES {
            write_program(&install.join(name), &format!("a-{name}"));
        }
        let state = PrivateDirectory::open_trusted(&root.join("state"), true)
            .expect("固定状态目录")
            .expect("状态目录存在");
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
                    file: fs::File::open(bin.join(name)).expect("固定真实制品文件句柄"),
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
            web_stack: None,
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
        self.deploy_with(artifact, schema, database_version, DeployFault::None)
    }

    /// 按 `fault` 模拟 `deploy_locked` 的结局。`InstallThird` 在安装前把制品的第三个程序等长改写
    /// （安装后复原），安装第三个程序失败后与 `deploy_locked` 一样自动换回，并以安装目录的实际
    /// 摘要发布失败回执；`Startup` 让四个程序都安装成功，再走 `deploy_locked` 启动失败分支的同一
    /// 发布路径（`FailedReceipt` 与已安装制品的摘要）。
    fn deploy_with(
        &self,
        artifact: &Artifact,
        schema: i64,
        database_version: i64,
        fault: DeployFault,
    ) -> RollbackRecord {
        let id = Uuid::new_v4();
        let request = RetainRequest {
            deploy_receipt_id: id,
            install_dir: &self.install,
            programs: &REQUIRED_BINARIES,
            observed_database_version: database_version,
            installed_role_schema_version: Some(schema),
        };
        let (retained, directory) =
            retain(&self.state, &request, |binaries| Ok(installed_identity(&self.state, binaries)))
                .expect("保留旧程序");
        let third = artifact.binary("auction-public-api").expect("制品程序").path.clone();
        let restore = corrupt_if(&third, fault == DeployFault::InstallThird);
        let installed = install_artifact(artifact, &self.install, &retained, &directory);
        restore();
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
        match (installed, fault) {
            (Ok(()), DeployFault::None) => {
                self.publish(id, Action::Deploy, Outcome::Succeeded, deploy);
            }
            (Ok(()), DeployFault::Startup) => {
                let context = ReceiptContext::resolve(None, Some("operator")).expect("上下文");
                let failed = FailedReceipt {
                    state: &self.state,
                    receipt_id: id,
                    action: Action::Deploy,
                    started_at: Timestamp::now(),
                    context: &context,
                    deploy: &deploy,
                };
                let failure = failed.publish(
                    anyhow::anyhow!("新程序已安装但启动失败"),
                    None,
                    installed_artifacts(artifact),
                );
                let text = format!("{:#}", failure.error);
                assert!(!text.contains("部署回执也未写入"), "{text}");
            }
            (Err(failure), DeployFault::InstallThird) => {
                assert_eq!(
                    failure.outcome,
                    InstallRecoveryOutcome::Restored,
                    "{:#}",
                    failure.error
                );
                self.publish(id, Action::Deploy, Outcome::Failed, deploy);
            }
            (Ok(()), DeployFault::InstallThird) => panic!("第三个程序必须安装失败"),
            (Err(failure), _) => panic!("安装不应失败：{:#}", failure.error),
        }
        retained
    }

    /// 模拟 `rollback`（不含 systemd 与数据库），停机前门禁与停机后的保留、安装都使用 `run` 与
    /// `apply` 的同一组判定：省略 `record_id` 时取最新记录；核对部署回执可回滚、安装目录与目标；
    /// 反向记录身份按摘要重新绑定；安装失败时自动换回，并像 `apply` 一样以安装目录的实际摘要
    /// 发布失败回执。`fail_third` 为真时在安装前把目标记录的第三个程序等长改写（安装后复原），
    /// 模拟安装第三个程序失败。
    fn attempt(
        &self,
        record_id: Option<Uuid>,
        database_version: i64,
        fail_third: bool,
    ) -> anyhow::Result<Attempt> {
        let (record, record_directory) = match record_id {
            Some(id) => load(&self.state, id)?,
            None => latest(&self.state)?,
        };
        let deployed = read_receipt(&self.state, record.deploy_receipt_id)?;
        require_rollbackable_receipt(&deployed)?;
        let installed = require_installed_matches(&deployed, &self.install)?;
        require_target_differs(&record, &installed)?;
        let (cli_before, _) = sha256_path(&self.install.join("auction-cli"))?;
        let reverse_id = Uuid::new_v4();
        let cli = kept_cli_identity(&deployed, &cli_before, database_version);
        let deploy =
            rollback_deploy_record(&record, &self.install, database_version, cli, reverse_id);
        let request = RetainRequest {
            deploy_receipt_id: reverse_id,
            install_dir: &self.install,
            programs: &ROLE_BINARIES,
            observed_database_version: database_version,
            installed_role_schema_version: record.retained_roles.schema_version,
        };
        let (reverse, reverse_directory) = retain(&self.state, &request, |binaries| {
            reverse_identity(&self.state, &deployed, &record, binaries)
        })?;
        let restore =
            corrupt_if(&record_directory.path().join("bin/auction-public-api"), fail_third);
        let installed = install_retained(
            &record,
            &record_directory,
            &self.install,
            &reverse,
            &reverse_directory,
        );
        restore();
        let (cli_after, _) = sha256_path(&self.install.join("auction-cli"))?;
        assert_eq!(cli_after, cli_before, "rollback 不替换 auction-cli");
        match installed {
            Ok(()) => {
                self.publish(reverse_id, Action::Rollback, Outcome::Succeeded, deploy);
                Ok(Attempt::Succeeded(reverse))
            }
            Err(failure) => {
                assert!(
                    fail_third && failure.outcome == InstallRecoveryOutcome::Restored,
                    "{:#}",
                    failure.error
                );
                self.publish(reverse_id, Action::Rollback, Outcome::Failed, deploy);
                Ok(Attempt::Failed(reverse))
            }
        }
    }

    /// 显式指定记录且必须成功的回退。
    fn rollback(&self, record: &RollbackRecord, database_version: i64) -> RollbackRecord {
        match self.attempt(Some(record.record_id), database_version, false).expect("回退") {
            Attempt::Succeeded(reverse) => reverse,
            Attempt::Failed(_) => panic!("回退不应失败"),
        }
    }

    /// 回执的程序摘要取安装目录的实际文件，与 `deploy`/`rollback` 的成功与失败回执同一口径。
    fn publish(
        &self,
        id: Uuid,
        action: Action,
        outcome: Outcome,
        deploy: DeployRecord,
    ) -> DeploymentReceipt {
        let context = ReceiptContext::resolve(None, Some("operator")).expect("上下文");
        let failure = (outcome == Outcome::Failed).then(|| "安装第三个程序失败".to_owned());
        let migration = (action == Action::Deploy && outcome == Outcome::Succeeded).then(|| {
            let before_version = deploy.database_version;
            let expected_version = deploy.roles.schema_version.expect("制品 schema 版本");
            MigrationRecord {
                before_version,
                after_version: expected_version,
                expected_version,
                applied: expected_version - before_version,
            }
        });
        record_deploy(
            &self.state,
            id,
            action,
            outcome,
            Timestamp::now(),
            &context,
            migration,
            self.role_artifacts(),
            deploy,
            failure,
        )
        .expect("发布回执")
    }

    /// 安装目录当前三个角色程序的保留副本形态（名称、摘要、字节数）。
    fn installed_roles(&self) -> Vec<RetainedBinary> {
        ROLE_BINARIES
            .into_iter()
            .map(|name| {
                let (sha256, bytes) = sha256_path(&self.install.join(name)).expect("摘要");
                RetainedBinary { name: name.to_owned(), sha256, bytes }
            })
            .collect()
    }

    /// 每条成功回执证明的角色身份都与它记录的程序摘要属于同一个制品；每条回滚记录登记的保留
    /// 身份都与保留副本属于同一个制品（身份未知的不比较）。制品 `<tag>` 的程序内容是
    /// `<tag>-<程序名>`，包名以 `<tag>` 重复 12 次结尾。
    fn assert_identities_match_bytes(&self) {
        fn tag(package: Option<&str>) -> Option<&str> {
            package.and_then(|package| package.rsplit('-').next()).and_then(|tail| tail.get(..1))
        }
        for receipt in list_receipts(&self.state, usize::MAX).expect("列出回执") {
            let Some((roles, _)) = receipt.proven_identity() else { continue };
            let tag = tag(roles.package.as_deref()).expect("成功回执的身份已知");
            for artifact in &receipt.artifacts {
                let name = ManagedRole::START_ORDER
                    .into_iter()
                    .find(|role| role.name() == artifact.role)
                    .map(role_binary)
                    .expect("角色");
                let expected = sha256_bytes(format!("{tag}-{name}").as_bytes());
                assert_eq!(artifact.sha256.as_deref(), Some(expected.as_str()), "回执 {name}");
            }
        }
        let root = self.state.path.join("rollback");
        for entry in fs::read_dir(&root).expect("回滚记录目录") {
            let name = entry.expect("目录项").file_name().into_string().expect("名称");
            let (record, _) =
                load(&self.state, Uuid::parse_str(&name).expect("标识")).expect("记录");
            let Some(tag) = tag(record.retained_roles.package.as_deref()) else { continue };
            for name in ROLE_BINARIES {
                let expected = sha256_bytes(format!("{tag}-{name}").as_bytes());
                let binary = record.binary(name).expect("角色程序");
                assert_eq!(binary.sha256, expected, "回滚记录 {} 的 {name}", record.record_id);
            }
        }
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

/// `Site::attempt` 的结果：成功或安装失败（已自动换回），都带本次保留的反向记录。
enum Attempt {
    Succeeded(RollbackRecord),
    Failed(RollbackRecord),
}

/// `Site::deploy_with` 模拟的 `deploy_locked` 结局。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeployFault {
    /// 安装与启动都成功，发布成功回执。
    None,
    /// 安装第三个程序失败并自动换回，失败回执的 `artifacts` 是换回后的实际程序。
    InstallThird,
    /// 四个程序都已安装、启动失败，失败回执的 `artifacts` 是已安装的制品程序。
    Startup,
}

/// `corrupt` 为真时就地等长改写 `path`（同一 inode，大小不变、摘要不同），返回复原函数。
fn corrupt_if(path: &Path, corrupt: bool) -> impl FnOnce() + use<> {
    let original = fs::read(path).expect("读取程序");
    if corrupt {
        let mut changed = original.clone();
        changed[0] ^= 0x20;
        fs::write(path, &changed).expect("改写程序");
    }
    let path = path.to_path_buf();
    move || {
        if corrupt {
            fs::write(&path, &original).expect("复原程序");
        }
    }
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
    site.assert_identities_match_bytes();
}

/// A、B 同 schema：B→A 回退装第三个程序时失败并自动换回（实际仍是 B），失败回执的 deploy 段是
/// 尝试安装的 A、`artifacts` 是实际的 B。再次省略 `--receipt-id` 的回退选中这条带失败回执的
/// 反向记录：目标就是当前程序，在停机前拒绝；即使绕过这道门，反向身份也只按摘要从成功回执绑定
/// 为 B，不会把 B 的副本记成 A。显式重试原记录才真正回到 A，之后每条成功回执与回滚记录的身份
/// 都与字节一致（旧实现会从失败回执取得身份，产生“摘要 B、身份 A”的记录与回执）。
#[test]
fn failed_rollback_receipt_is_never_an_identity_source() {
    let site = Site::new();
    let a = site.artifact("a");
    let b = site.artifact("b");
    site.deploy(&a, 96, 96);
    let to_b = site.deploy(&b, 96, 96);

    let Attempt::Failed(back) = site.attempt(Some(to_b.record_id), 96, true).expect("回退尝试")
    else {
        panic!("第三个程序必须失败");
    };
    for name in ROLE_BINARIES {
        assert_eq!(site.installed(name), format!("b-{name}"), "自动换回后仍是 B");
    }
    assert_eq!(back.retained_roles.package.as_deref(), Some(b.package.as_str()));
    let failed = read_receipt(&site.state, back.record_id).expect("失败回执");
    assert_eq!(failed.outcome, Outcome::Failed);
    assert_eq!(
        site.current(back.record_id).roles.package.as_deref(),
        Some(a.package.as_str()),
        "deploy 段只是尝试安装的 A"
    );
    assert!(failed.proven_identity().is_none(), "失败回执不证明任何组件的身份");

    let error = site.attempt(None, 96, false).err().expect("目标就是当前程序时必须拒绝");
    assert!(format!("{error:#}").contains("就是安装目录当前的程序"), "{error:#}");
    let rebound =
        reverse_identity(&site.state, &failed, &back, &site.installed_roles()).expect("反向身份");
    assert_eq!(rebound.roles.package.as_deref(), Some(b.package.as_str()), "按摘要绑定为 B");

    // 显式指定原记录重试：真正回到 A，回执声称 A、实际字节也是 A。
    let forward = site.rollback(&to_b, 96);
    for name in ROLE_BINARIES {
        assert_eq!(site.installed(name), format!("a-{name}"));
    }
    assert_eq!(site.current(forward.record_id).roles.package.as_deref(), Some(a.package.as_str()));
    assert_eq!(forward.retained_roles.package.as_deref(), Some(b.package.as_str()));

    // 再次省略 --receipt-id：选中刚保留 B 的反向记录，回到 B，身份仍与字节一致。
    let Attempt::Succeeded(again) = site.attempt(None, 96, false).expect("再次回退") else {
        panic!("再次回退应当成功");
    };
    for name in ROLE_BINARIES {
        assert_eq!(site.installed(name), format!("b-{name}"));
    }
    assert_eq!(site.current(again.record_id).roles.package.as_deref(), Some(b.package.as_str()));
    assert_eq!(again.retained_roles.package.as_deref(), Some(a.package.as_str()));
    site.assert_identities_match_bytes();
}

/// deploy 安装第三个程序失败并换回：失败回执的 deploy 段是尝试安装的 B、`artifacts` 是实际的
/// A；本次回滚记录保留的正是当前的 A，按它回退会在停机前因目标与当前相同而拒绝。之后再部署 B，
/// 被保留的程序按摘要绑定为 A，不采用失败回执里的 B。
#[test]
fn restored_deploy_failure_is_not_a_rollback_target() {
    let site = Site::new();
    let a = site.artifact("a");
    let b = site.artifact("b");
    site.deploy(&a, 96, 96);
    let attempted = site.deploy_with(&b, 96, 96, DeployFault::InstallThird);
    for name in ROLE_BINARIES {
        assert_eq!(site.installed(name), format!("a-{name}"), "自动换回后仍是 A");
    }
    let failed = read_receipt(&site.state, attempted.record_id).expect("失败回执");
    assert_eq!(failed.outcome, Outcome::Failed);
    assert!(failed.proven_identity().is_none());

    let error = site.attempt(Some(attempted.record_id), 96, false).err().expect("必须拒绝");
    assert!(format!("{error:#}").contains("就是安装目录当前的程序"), "{error:#}");

    let redeploy = site.deploy(&b, 96, 96);
    assert_eq!(redeploy.retained_roles.package.as_deref(), Some(a.package.as_str()));
    site.assert_identities_match_bytes();
}

/// 安装完成、启动失败之后回退到不同的目标仍被放行（A、B 同 schema，迁移已完成或没有迁移）：
/// B 的四个程序都已装上，部署回执按 `deploy_locked` 启动失败分支发布为失败回执（deploy 段与
/// `artifacts` 都是 B），本次回滚记录保留的是 A。入口接受失败回执，“目标就是当前程序”只比较
/// 程序摘要，因此省略 `--receipt-id` 与显式指定这条记录都回到 A，CLI 仍是 B。失败回执不证明
/// B 的身份，反向记录的角色身份为 null；每条成功回执与回滚记录的身份仍与字节一致。
#[test]
fn startup_failure_after_install_still_rolls_back_to_a_different_target() {
    for explicit in [false, true] {
        let site = Site::new();
        let a = site.artifact("a");
        let b = site.artifact("b");
        site.deploy(&a, 96, 96);
        let to_b = site.deploy_with(&b, 96, 96, DeployFault::Startup);
        for name in REQUIRED_BINARIES {
            assert_eq!(site.installed(name), format!("b-{name}"), "B 已完整安装");
        }
        assert_eq!(to_b.retained_roles.package.as_deref(), Some(a.package.as_str()));
        let failed = read_receipt(&site.state, to_b.record_id).expect("失败回执");
        assert_eq!(failed.outcome, Outcome::Failed);
        assert!(failed.proven_identity().is_none(), "失败回执不证明任何组件的身份");
        assert_eq!(site.current(to_b.record_id).roles.package.as_deref(), Some(b.package.as_str()));

        let selected = explicit.then_some(to_b.record_id);
        let attempt = site.attempt(selected, 96, false);
        let Attempt::Succeeded(back) = attempt.expect("启动失败后回退到不同目标必须放行")
        else {
            panic!("回退不应失败");
        };
        for name in ROLE_BINARIES {
            assert_eq!(site.installed(name), format!("a-{name}"), "三个角色回到 A");
        }
        assert_eq!(site.installed("auction-cli"), "b-auction-cli", "rollback 保留 B 的 CLI");
        let receipt = site.current(back.record_id);
        assert_eq!(receipt.roles.package.as_deref(), Some(a.package.as_str()));
        assert_eq!(receipt.cli.package.as_deref(), None, "B 的 CLI 只有失败回执，来源未知");
        assert_eq!(back.retained_roles, RoleIdentity::default(), "B 的角色身份未经证明");
        site.assert_identities_match_bytes();
    }
}
