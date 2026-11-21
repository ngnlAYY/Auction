//! 部署回执、失败记录与已安装程序的身份查询。
//!
//! 不变量：每次 `deploy`/`rollback` 调用最多追加一条回执。保留旧程序（写出回滚记录）之后的
//! 失败，以与回滚记录相同的标识发布带 deploy 段的失败回执，`rollback --receipt-id` 因此总能
//! 读回它；此前的失败没有回滚记录，由入口追加一条通用失败回执。回执写不进去时把写入错误接在
//! 原错误之后返回，不掩盖失败原因，也不再改用随机标识补写第二条。

use auction_time::Timestamp;
use uuid::Uuid;

use super::install::{RetainedBinary, RetainedIdentity};
use super::{artifact, role_binary};
use crate::app::progress;
use crate::receipt;
use crate::receipt::{
    Action, DeployRecord, DeploymentReceipt, MigrationRecord, Outcome, ReceiptContext, RoleIdentity,
};
use crate::service::{self, ManagedRole, Systemctl};
use crate::support::files::PrivateDirectory;

/// 停机后流程的失败；`recorded` 为真表示此调用已按对应标识收口，不再追加通用回执，无论发布成功与否。
pub(super) struct Failure {
    pub(super) error: anyhow::Error,
    pub(super) recorded: bool,
}

impl Failure {
    /// 保留旧程序之前的失败：还没有回滚记录，由入口追加通用失败回执。
    pub(super) fn unrecorded(error: anyhow::Error) -> Self {
        Self { error, recorded: false }
    }
}

impl From<anyhow::Error> for Failure {
    fn from(error: anyhow::Error) -> Self {
        Self::unrecorded(error)
    }
}

/// 保留旧程序之后的失败回执；标识与回滚记录相同，并携带制品与回滚证据。`state` 是持锁时固定的
/// 部署状态目录。
pub(super) struct FailedReceipt<'a> {
    pub(super) state: &'a PrivateDirectory,
    pub(super) receipt_id: Uuid,
    pub(super) action: Action,
    pub(super) started_at: Timestamp,
    pub(super) context: &'a ReceiptContext,
    pub(super) deploy: &'a DeployRecord,
}

impl FailedReceipt<'_> {
    /// 发布失败回执并返回已收口的失败。若发布出错，按固定标识读取既有回执；读取失败也不推断该标识下
    /// 没有文件，不换标识补写第二条。
    pub(super) fn publish(
        &self,
        failure: anyhow::Error,
        migration: Option<MigrationRecord>,
        artifacts: anyhow::Result<Vec<receipt::Artifact>>,
    ) -> Failure {
        let published = artifacts.and_then(|artifacts| {
            receipt::record_deploy(
                self.state,
                self.receipt_id,
                self.action,
                Outcome::Failed,
                self.started_at,
                self.context,
                migration,
                artifacts,
                self.deploy.clone(),
                Some(receipt::failure_text(&failure)),
            )
        });
        let error = match published {
            Ok(_) => failure,
            Err(record_error) => {
                let existing = match receipt::read(self.state, self.receipt_id) {
                    Ok(receipt) => format!(
                        "同一标识的现有回执可读（action={:?}, outcome={:?}）；未覆盖该回执，请核对后再重试或回退",
                        receipt.action, receipt.outcome
                    ),
                    Err(read_error) => format!(
                        "同一标识的回执状态无法确认：{read_error:#}；重试或回退前先核对该标识"
                    ),
                };
                anyhow::anyhow!("{failure:#}；失败回执未写入：{record_error:#}；{existing}")
            }
        };
        Failure { error, recorded: true }
    }
}

/// 停机后失败的收口：已记录的直接返回原错误；否则追加一条通用失败回执，回执写入失败时
/// 返回合并两段原因的错误。
pub(super) async fn settle(
    state: &PrivateDirectory,
    systemctl: &Systemctl,
    action: Action,
    started_at: Timestamp,
    context: &ReceiptContext,
    failure: Failure,
) -> anyhow::Error {
    let Failure { error, recorded } = failure;
    if recorded {
        return error;
    }
    match service::record_failure(state, systemctl, action, started_at, context, &error).await {
        Ok(()) => error,
        Err(combined) => combined,
    }
}

pub(super) fn artifact_projection(artifact: &artifact::Artifact) -> serde_json::Value {
    serde_json::json!({
        "directory": artifact.root,
        "package": artifact.package,
        "source_commit": artifact.source_commit,
        "source_fingerprint": artifact.source_fingerprint,
        "binaries": artifact.binaries.iter().map(|binary| serde_json::json!({
            "name": binary.name,
            "sha256": binary.sha256,
            "bytes": binary.bytes,
        })).collect::<Vec<_>>(),
        "public_payload_sha256": artifact.public_payload_sha256,
        "admin_payload_sha256": artifact.admin_payload_sha256,
    })
}

/// 与保留副本实际摘要绑定的身份；deploy 与 rollback 保留程序时共用。只在“成功且含部署身份”
/// 的回执里查找（过滤先于限量，较新的 `service start/restart` 回执不会把它挤出窗口），按完成
/// 时间倒序取第一条三角色程序摘要与 `binaries` 完全一致、且身份可信的回执；失败回执的 deploy 段
/// 只是尝试安装的目标，从不作为来源，v1 rollback 回执的身份混有被回退版本的证据，同样跳过并
/// 继续向前找。找不到时角色身份全部为 null，CLI 只记录实际摘要。回执目录读不出时同样记为未知
/// 并在 stderr 说明：身份只服务追溯与回退判定，未知时回退按观测版本假设、只允许同版本并标明来源。
pub(super) fn installed_identity(
    state: &PrivateDirectory,
    binaries: &[RetainedBinary],
) -> RetainedIdentity {
    let unknown = RetainedIdentity { roles: RoleIdentity::default(), cli: None };
    let receipts = match receipt::list_matching(state, usize::MAX, |receipt| {
        receipt.outcome == Outcome::Succeeded && receipt.deploy.is_some()
    }) {
        Ok(receipts) => receipts,
        Err(error) => {
            progress(format_args!("读取部署回执失败，保留程序的身份记为未知：{error:#}"));
            return unknown;
        }
    };
    receipts
        .iter()
        .filter(|receipt| roles_match(receipt, binaries))
        .find_map(|receipt| {
            let (roles, cli) = receipt.proven_identity()?;
            Some(RetainedIdentity { roles, cli: Some(cli) })
        })
        .unwrap_or(unknown)
}

/// 回执记录的三个运行角色程序摘要与保留副本逐个相同；任一缺失都不算匹配。
pub(super) fn roles_match(receipt: &DeploymentReceipt, binaries: &[RetainedBinary]) -> bool {
    ManagedRole::START_ORDER.into_iter().all(|role| {
        let retained = binaries
            .iter()
            .find(|binary| binary.name == role_binary(role))
            .map(|binary| binary.sha256.as_str());
        let recorded = receipt
            .artifacts
            .iter()
            .find(|artifact| artifact.role == role.name())
            .and_then(|artifact| artifact.sha256.as_deref());
        retained.is_some() && retained == recorded
    })
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        path::{Path, PathBuf},
    };

    use auction_time::Timestamp;
    use uuid::Uuid;

    use super::{FailedReceipt, Failure, installed_identity, settle};
    use crate::cli::UnitArgs;
    use crate::deploy::install::{RetainRequest, RetainedBinary, RetainedIdentity, latest, retain};
    use crate::receipt::{
        Action, Artifact, CliIdentity, DeployRecord, DeploySection, DeploymentReceipt,
        LEGACY_FORMAT, LegacyDeployRecord, MigrationRecord, Outcome, ReceiptContext, RoleIdentity,
        append, list as list_receipts, read as read_receipt, receipts_dir, record_deploy,
    };
    use crate::service::Systemctl;
    use crate::support::files::PrivateDirectory;

    struct TempRoot(PathBuf);

    impl TempRoot {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("auction-deploy-receipts-{}", Uuid::new_v4()));
            fs::create_dir_all(&root).expect("建立临时目录");
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).expect("设置权限");
            Self(root)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn install_dir(&self) -> PathBuf {
            let directory = self.0.join("install");
            fs::create_dir_all(&directory).expect("建立安装目录");
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).expect("设置权限");
            for name in crate::deploy::artifact::REQUIRED_BINARIES {
                let path = directory.join(name);
                fs::write(&path, format!("old-{name}")).expect("写入旧程序");
                fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("设置权限");
            }
            directory
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// 按生产口径核对并固定部署状态目录（不存在时以 0700 创建）。
    fn pin(path: &Path) -> PrivateDirectory {
        PrivateDirectory::open_trusted(path, true).expect("固定状态目录").expect("状态目录存在")
    }

    fn retain_unknown(
        state: &PrivateDirectory,
        install: &Path,
    ) -> crate::deploy::install::RollbackRecord {
        let request = RetainRequest {
            deploy_receipt_id: Uuid::new_v4(),
            install_dir: install,
            programs: &crate::deploy::artifact::REQUIRED_BINARIES,
            observed_database_version: 94,
            installed_role_schema_version: Some(96),
        };
        retain(state, &request, |_| {
            Ok(RetainedIdentity { roles: RoleIdentity::default(), cli: None })
        })
        .expect("保留旧程序")
        .0
    }

    fn identity(tag: char, schema: i64) -> (RoleIdentity, CliIdentity) {
        let package = Some(format!("auction-rust-0.1.0-{}", tag.to_string().repeat(12)));
        let commit = Some(tag.to_string().repeat(40));
        (
            RoleIdentity {
                artifact_directory: Some(format!("/srv/{tag}")),
                package: package.clone(),
                source_commit: commit.clone(),
                source_fingerprint: Some(tag.to_string().repeat(64)),
                public_payload_sha256: Some(tag.to_string().repeat(64)),
                admin_payload_sha256: Some(tag.to_string().repeat(64)),
                schema_version: Some(schema),
            },
            CliIdentity {
                sha256: tag.to_string().repeat(64),
                package,
                source_commit: commit,
                schema_version: Some(schema),
            },
        )
    }

    fn deploy_record(install: &Path, rollback_record_id: Uuid) -> DeployRecord {
        let (roles, cli) = identity('a', 96);
        DeployRecord {
            install_directory: install.display().to_string(),
            roles,
            cli,
            database_version: 94,
            migration_drill_id: None,
            backup_archive: Some("/var/backups/auction/auction-deploy.dump".to_owned()),
            backup_manifest: Some("/var/backups/auction/auction-deploy.dump.manifest".to_owned()),
            rollback_record_id: Some(rollback_record_id),
        }
    }

    fn receipt_files(state: &Path) -> usize {
        match fs::read_dir(receipts_dir(state)) {
            Ok(entries) => entries
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().ends_with(".json"))
                .count(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
            Err(error) => panic!("读取回执目录失败：{error}"),
        }
    }

    fn units(state: &Path) -> UnitArgs {
        UnitArgs {
            // 只在需要收集运行证据时被调用；/bin/true 不输出 unit 属性，等价于“没有运行中的角色”。
            systemctl: PathBuf::from("/bin/true"),
            state_dir: state.to_path_buf(),
            worker_unit: "auction-worker.service".to_owned(),
            admin_unit: "auction-admin-api.service".to_owned(),
            public_unit: "auction-public-api.service".to_owned(),
        }
    }

    /// 保留旧程序之后的失败只留下一条回执，且标识等于回滚记录，rollback 能按记录读回。
    #[tokio::test]
    async fn failure_after_retain_publishes_exactly_one_receipt_with_record_id() {
        let root = TempRoot::new();
        let install = root.install_dir();
        let state = pin(&root.path().join("state"));
        let record = retain_unknown(&state, &install);
        let context = ReceiptContext::resolve(None, Some("operator")).expect("上下文");
        let deploy = deploy_record(&install, record.record_id);
        let failed = FailedReceipt {
            state: &state,
            receipt_id: record.deploy_receipt_id,
            action: Action::Deploy,
            started_at: auction_time::Timestamp::now(),
            context: &context,
            deploy: &deploy,
        };

        let failure = failed.publish(anyhow::anyhow!("startup failed"), None, Ok(Vec::new()));
        assert!(failure.recorded);
        let units = units(&state.path);
        let systemctl = Systemctl::new(&units).expect("测试 systemctl");
        let error = settle(
            &state,
            &systemctl,
            Action::Deploy,
            auction_time::Timestamp::now(),
            &context,
            failure,
        )
        .await;

        assert_eq!(format!("{error:#}"), "startup failed");
        assert_eq!(receipt_files(&state.path), 1, "已记录的失败不得再追加通用回执");
        let published = read_receipt(&state, record.record_id).expect("按回滚记录读回失败回执");
        assert_eq!(published.outcome, Outcome::Failed);
        assert_eq!(published.failure.as_deref(), Some("startup failed"));
        assert_eq!(
            published.deploy.expect("部署证据").rollback_record_id(),
            Some(record.record_id)
        );
        assert_eq!(latest(&state).expect("最新回滚记录").0.record_id, record.record_id);
    }

    /// 同一标识已经发布时补写失败回执会被无覆盖发布拒绝：原错误保留，回执仍只有一条。
    #[test]
    fn republishing_the_same_id_keeps_the_original_error() {
        let root = TempRoot::new();
        let install = root.install_dir();
        let state = pin(&root.path().join("state"));
        let record = retain_unknown(&state, &install);
        let context = ReceiptContext::resolve(None, Some("operator")).expect("上下文");
        let deploy = deploy_record(&install, record.record_id);
        let failed = FailedReceipt {
            state: &state,
            receipt_id: record.deploy_receipt_id,
            action: Action::Deploy,
            started_at: auction_time::Timestamp::now(),
            context: &context,
            deploy: &deploy,
        };
        assert!(failed.publish(anyhow::anyhow!("first"), None, Ok(Vec::new())).recorded);

        let failure = failed.publish(anyhow::anyhow!("receipt write failed"), None, Ok(Vec::new()));
        assert!(failure.recorded);
        let text = format!("{:#}", failure.error);
        assert!(
            text.contains("receipt write failed")
                && text.contains("同一标识的现有回执可读")
                && text.contains("outcome=Failed"),
            "{text}"
        );
        assert_eq!(receipt_files(&state.path), 1);
        assert_eq!(
            read_receipt(&state, record.record_id).expect("原回执").failure.as_deref(),
            Some("first")
        );

        let failure = failed.publish(
            anyhow::anyhow!("launch failed"),
            None,
            Err(anyhow::anyhow!("no digest")),
        );
        assert!(failure.recorded);
        let text = format!("{:#}", failure.error);
        assert!(
            text.contains("launch failed") && text.contains("同一标识的现有回执可读"),
            "{text}"
        );
        assert_eq!(receipt_files(&state.path), 1);
    }
    #[test]
    fn success_receipt_is_reported_when_failure_republication_collides() {
        let root = TempRoot::new();
        let install = root.install_dir();
        let state = pin(&root.path().join("state"));
        let record = retain_unknown(&state, &install);
        let mut success = receipt(0, Action::Deploy, Outcome::Succeeded, 'a');
        success.receipt_id = record.deploy_receipt_id;
        append(&state, &success).expect("模拟已发布的成功回执");

        let context = ReceiptContext::resolve(None, Some("operator")).expect("上下文");
        let deploy = deploy_record(&install, record.record_id);
        let failed = FailedReceipt {
            state: &state,
            receipt_id: record.deploy_receipt_id,
            action: Action::Deploy,
            started_at: success.started_at,
            context: &context,
            deploy: &deploy,
        };
        let failure = failed.publish(anyhow::anyhow!("成功回执目录同步失败"), None, Ok(Vec::new()));
        assert!(failure.recorded);
        let text = format!("{:#}", failure.error);
        assert!(text.contains("成功回执目录同步失败"), "{text}");
        assert!(text.contains("失败回执未写入") && text.contains("outcome=Succeeded"), "{text}");
        assert_eq!(
            read_receipt(&state, record.record_id).expect("成功回执仍可读").outcome,
            Outcome::Succeeded
        );
        assert_eq!(receipt_files(&state.path), 1);
    }

    /// 保留旧程序之前的失败没有回滚记录：入口追加一条通用失败回执（无 deploy 段）。
    #[tokio::test]
    async fn unrecorded_failure_appends_one_generic_receipt() {
        let root = TempRoot::new();
        let state = pin(&root.path().join("state"));
        let units = units(&state.path);
        let systemctl = Systemctl::new(&units).expect("测试 systemctl");
        let context = ReceiptContext::resolve(None, Some("operator")).expect("上下文");

        let error = settle(
            &state,
            &systemctl,
            Action::Deploy,
            auction_time::Timestamp::now(),
            &context,
            Failure::unrecorded(anyhow::anyhow!("backup failed")),
        )
        .await;

        assert_eq!(format!("{error:#}"), "backup failed");
        let receipts = list_receipts(&state, 10).expect("列出回执");
        assert_eq!(receipts.len(), 1);
        assert_eq!(receipts[0].action, Action::Deploy);
        assert_eq!(receipts[0].outcome, Outcome::Failed);
        assert!(receipts[0].deploy.is_none());
        assert!(receipts[0].artifacts.is_empty());
    }

    /// 三角色程序摘要固定为 `<tag>-<程序名>` 的保留副本与对应的运行证据。
    fn binaries(tag: char) -> Vec<RetainedBinary> {
        ["auction-worker", "auction-admin-api", "auction-public-api", "auction-cli"]
            .into_iter()
            .map(|name| RetainedBinary {
                name: name.to_owned(),
                sha256: crate::support::digest::sha256_bytes(format!("{tag}-{name}").as_bytes()),
                bytes: 8,
            })
            .collect()
    }

    fn artifacts(tag: char) -> Vec<Artifact> {
        binaries(tag)
            .into_iter()
            .filter_map(|binary| {
                let role = match binary.name.as_str() {
                    "auction-worker" => "worker",
                    "auction-admin-api" => "admin",
                    "auction-public-api" => "public",
                    _ => return None,
                };
                Some(Artifact {
                    role: role.to_owned(),
                    path: Some(format!("/opt/auction/bin/{}", binary.name)),
                    sha256: Some(binary.sha256),
                    bytes: Some(binary.bytes),
                })
            })
            .collect()
    }

    fn receipt(minutes: i64, action: Action, outcome: Outcome, tag: char) -> DeploymentReceipt {
        let started = auction_time::Timestamp::from_timestamp(1_790_000_000, 0).expect("固定时间")
            + auction_time::Duration::minutes(minutes);
        let deploy = matches!(action, Action::Deploy | Action::Rollback).then(|| {
            let (roles, cli) = identity(tag, 90 + minutes);
            DeploySection::Current(DeployRecord {
                install_directory: "/opt/auction/bin".to_owned(),
                roles,
                cli,
                database_version: 90,
                migration_drill_id: None,
                backup_archive: None,
                backup_manifest: None,
                rollback_record_id: Some(Uuid::new_v4()),
            })
        });
        let migration = if action == Action::Deploy && outcome == Outcome::Succeeded {
            let expected_version = 90 + minutes;
            Some(MigrationRecord {
                before_version: 90,
                after_version: expected_version,
                expected_version,
                applied: minutes,
            })
        } else {
            None
        };
        DeploymentReceipt {
            format: crate::receipt::FORMAT.to_owned(),
            receipt_id: Uuid::new_v4(),
            action,
            outcome,
            started_at: started,
            finished_at: started + auction_time::Duration::seconds(30),
            actor: "operator".to_owned(),
            actor_uid: 0,
            backup_id: None,
            migration,
            artifacts: artifacts(tag),
            failure: (outcome == Outcome::Failed).then(|| "启动失败".to_owned()),
            deploy,
        }
    }

    /// 身份查询只看“成功且含部署身份”的回执，过滤先于限量：部署后 25 条更新的
    /// start/restart 回执与一条失败的 deploy 回执都不会挤掉或冒充它；摘要不符的回执不采用。
    #[test]
    fn identity_lookup_skips_service_and_failed_receipts() {
        let root = TempRoot::new();
        let state = pin(&root.path().join("state"));
        append(&state, &receipt(0, Action::Deploy, Outcome::Succeeded, 'a')).expect("A 部署");
        for minutes in 1..=25 {
            let action = if minutes % 2 == 0 { Action::Start } else { Action::Restart };
            append(&state, &receipt(minutes, action, Outcome::Succeeded, 'a')).expect("服务回执");
        }
        append(&state, &receipt(30, Action::Deploy, Outcome::Failed, 'b')).expect("失败部署");

        let found = installed_identity(&state, &binaries('a'));
        assert_eq!(found.roles, identity('a', 90).0, "身份来自 A 的成功部署回执");
        assert_eq!(found.cli, Some(identity('a', 90).1));

        let unknown = installed_identity(&state, &binaries('c'));
        assert_eq!(unknown.roles, RoleIdentity::default(), "没有摘要一致的回执时身份未知");
        assert!(unknown.cli.is_none());
    }

    /// v1 rollback 回执的身份混有被回退版本的证据：跳过它继续向前，找到同一组程序的 v1 deploy
    /// 回执，schema 取其迁移记录的期望版本。
    #[test]
    fn identity_lookup_skips_legacy_rollback_sections() {
        let root = TempRoot::new();
        let state = pin(&root.path().join("state"));
        let legacy = |minutes: i64, action: Action, tag: char| {
            let mut value = receipt(minutes, action, Outcome::Succeeded, tag);
            value.format = LEGACY_FORMAT.to_owned();
            value.migration = (action == Action::Deploy).then_some(MigrationRecord {
                before_version: 93,
                after_version: 94,
                expected_version: 94,
                applied: 1,
            });
            value.deploy = Some(DeploySection::Legacy(LegacyDeployRecord {
                artifact_directory: format!("/srv/{tag}"),
                package: format!("auction-rust-0.1.0-{}", tag.to_string().repeat(12)),
                source_commit: tag.to_string().repeat(40),
                source_fingerprint: tag.to_string().repeat(64),
                // v1 rollback 回执混入了被回退版本 B 的制品证据。
                cli_sha256: "b".repeat(64),
                install_directory: "/opt/auction/bin".to_owned(),
                public_payload_sha256: tag.to_string().repeat(64),
                admin_payload_sha256: tag.to_string().repeat(64),
                migration_drill_id: None,
                backup_archive: None,
                backup_manifest: None,
                rollback_record_id: Some(Uuid::new_v4()),
            }));
            value
        };
        append(&state, &legacy(0, Action::Deploy, 'a')).expect("v1 A 部署");
        append(&state, &legacy(10, Action::Deploy, 'b')).expect("v1 B 部署");
        // v1 回退到 A：运行证据是 A 的程序，deploy 段却仍是 B 的制品。
        let mut back = legacy(20, Action::Rollback, 'b');
        back.artifacts = artifacts('a');
        append(&state, &back).expect("v1 回退");

        let found = installed_identity(&state, &binaries('a'));
        assert_eq!(found.roles.package.as_deref(), Some("auction-rust-0.1.0-aaaaaaaaaaaa"));
        assert_eq!(found.roles.schema_version, Some(94));
    }

    /// 回执目录损坏时身份记为未知、不阻断部署；成功回执仍需记录。
    #[test]
    fn unreadable_receipts_leave_the_identity_unknown() {
        let root = TempRoot::new();
        let state = pin(&root.path().join("state"));
        let context = ReceiptContext::resolve(None, Some("operator")).expect("上下文");
        record_deploy(
            &state,
            Uuid::new_v4(),
            Action::Deploy,
            Outcome::Succeeded,
            Timestamp::now(),
            &context,
            Some(MigrationRecord {
                before_version: 94,
                after_version: 96,
                expected_version: 96,
                applied: 2,
            }),
            artifacts('a'),
            deploy_record(Path::new("/opt/auction/bin"), Uuid::new_v4()),
            None,
        )
        .expect("写入回执");
        fs::write(receipts_dir(&state.path).join("broken.json"), b"{").expect("损坏回执");
        let found = installed_identity(&state, &binaries('a'));
        assert_eq!(found.roles, RoleIdentity::default());
    }
}
