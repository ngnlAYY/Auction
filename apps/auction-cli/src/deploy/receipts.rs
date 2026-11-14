//! 部署回执与失败记录。
//!
//! 不变量：每次 `deploy`/`rollback` 调用最多追加一条回执。保留旧程序（写出回滚记录）之后的
//! 失败，以与回滚记录相同的标识发布带 deploy 段的失败回执，`rollback --receipt-id` 因此总能
//! 读回它；此前的失败没有回滚记录，由入口追加一条通用失败回执。回执写不进去时把写入错误接在
//! 原错误之后返回，不掩盖失败原因，也不再改用随机标识补写第二条。

use std::path::Path;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::artifact;
use crate::cli::UnitArgs;
use crate::receipt;
use crate::receipt::{Action, DeployRecord, MigrationRecord, Outcome, ReceiptContext};
use crate::service::{self, Systemctl};

/// 停机后流程的失败；`recorded` 为真表示本次调用已用回滚记录的标识尝试发布失败回执。
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

/// 保留旧程序之后的失败回执；标识与回滚记录相同，并携带制品与回滚证据。
pub(super) struct FailedReceipt<'a> {
    pub(super) state_dir: &'a Path,
    pub(super) receipt_id: Uuid,
    pub(super) action: Action,
    pub(super) started_at: DateTime<Utc>,
    pub(super) context: &'a ReceiptContext,
    pub(super) deploy: &'a DeployRecord,
}

impl FailedReceipt<'_> {
    /// 发布失败回执并返回已记录的失败。程序摘要无法取得或回执写入失败（含同一标识已经发布）
    /// 时，把原因接在原错误之后；无论哪种情况本次调用都不再追加第二条回执。
    pub(super) fn publish(
        &self,
        failure: anyhow::Error,
        migration: Option<MigrationRecord>,
        artifacts: anyhow::Result<Vec<receipt::Artifact>>,
    ) -> Failure {
        let published = artifacts.and_then(|artifacts| {
            receipt::record_deploy(
                self.state_dir,
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
            Err(record_error) => anyhow::anyhow!("{failure:#}；部署回执也未写入：{record_error:#}"),
        };
        Failure { error, recorded: true }
    }
}

/// 停机后失败的收口：已记录的直接返回原错误；否则追加一条通用失败回执，回执写入失败时
/// 返回合并两段原因的错误。
pub(super) async fn settle(
    units: &UnitArgs,
    systemctl: &Systemctl,
    action: Action,
    started_at: DateTime<Utc>,
    context: &ReceiptContext,
    failure: Failure,
) -> anyhow::Error {
    let Failure { error, recorded } = failure;
    if recorded {
        return error;
    }
    match service::record_failure(units, systemctl, action, started_at, context, &error).await {
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

/// 最近一次成功部署的源码提交；用于回滚记录的可追溯性，缺失时不伪造。
pub(super) fn previous_source_commit(units: &UnitArgs) -> Option<String> {
    let receipts = receipt::list(&units.state_dir, 20).ok()?;
    receipts
        .into_iter()
        .find(|receipt| receipt.outcome == Outcome::Succeeded)
        .and_then(|receipt| receipt.deploy)
        .map(|deploy| deploy.source_commit)
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        path::{Path, PathBuf},
    };

    use uuid::Uuid;

    use super::{FailedReceipt, Failure, settle};
    use crate::cli::UnitArgs;
    use crate::deploy::install::{latest, retain};
    use crate::receipt::{
        Action, DeployRecord, Outcome, ReceiptContext, list as list_receipts, read as read_receipt,
        receipts_dir,
    };
    use crate::service::Systemctl;

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

    fn deploy_record(install: &Path, rollback_record_id: Uuid) -> DeployRecord {
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
        let state = root.path().join("state");
        let record = retain(&state, Uuid::new_v4(), &install, 94, 96, None).expect("保留旧程序");
        let context = ReceiptContext::resolve(None, Some("operator")).expect("上下文");
        let deploy = deploy_record(&install, record.record_id);
        let failed = FailedReceipt {
            state_dir: &state,
            receipt_id: record.deploy_receipt_id,
            action: Action::Deploy,
            started_at: chrono::Utc::now(),
            context: &context,
            deploy: &deploy,
        };

        let failure = failed.publish(anyhow::anyhow!("startup failed"), None, Ok(Vec::new()));
        assert!(failure.recorded);
        let units = units(&state);
        let systemctl = Systemctl::new(&units).expect("测试 systemctl");
        let error =
            settle(&units, &systemctl, Action::Deploy, chrono::Utc::now(), &context, failure).await;

        assert_eq!(format!("{error:#}"), "startup failed");
        assert_eq!(receipt_files(&state), 1, "已记录的失败不得再追加通用回执");
        let published = read_receipt(&state, record.record_id).expect("按回滚记录读回失败回执");
        assert_eq!(published.outcome, Outcome::Failed);
        assert_eq!(published.failure.as_deref(), Some("startup failed"));
        assert_eq!(published.deploy.expect("部署证据").rollback_record_id, Some(record.record_id));
        assert_eq!(latest(&state).expect("最新回滚记录").0.record_id, record.record_id);
    }

    /// 同一标识已经发布时补写失败回执会被无覆盖发布拒绝：原错误保留，回执仍只有一条。
    #[test]
    fn republishing_the_same_id_keeps_the_original_error() {
        let root = TempRoot::new();
        let install = root.install_dir();
        let state = root.path().join("state");
        let record = retain(&state, Uuid::new_v4(), &install, 94, 96, None).expect("保留旧程序");
        let context = ReceiptContext::resolve(None, Some("operator")).expect("上下文");
        let deploy = deploy_record(&install, record.record_id);
        let failed = FailedReceipt {
            state_dir: &state,
            receipt_id: record.deploy_receipt_id,
            action: Action::Deploy,
            started_at: chrono::Utc::now(),
            context: &context,
            deploy: &deploy,
        };
        assert!(failed.publish(anyhow::anyhow!("first"), None, Ok(Vec::new())).recorded);

        let failure = failed.publish(anyhow::anyhow!("receipt write failed"), None, Ok(Vec::new()));
        assert!(failure.recorded);
        let text = format!("{:#}", failure.error);
        assert!(text.starts_with("receipt write failed；部署回执也未写入："), "{text}");
        assert_eq!(receipt_files(&state), 1);
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
        assert!(format!("{:#}", failure.error).starts_with("launch failed；部署回执也未写入："));
        assert_eq!(receipt_files(&state), 1);
    }

    /// 保留旧程序之前的失败没有回滚记录：入口追加一条通用失败回执（无 deploy 段）。
    #[tokio::test]
    async fn unrecorded_failure_appends_one_generic_receipt() {
        let root = TempRoot::new();
        let state = root.path().join("state");
        let units = units(&state);
        let systemctl = Systemctl::new(&units).expect("测试 systemctl");
        let context = ReceiptContext::resolve(None, Some("operator")).expect("上下文");

        let error = settle(
            &units,
            &systemctl,
            Action::Deploy,
            chrono::Utc::now(),
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
}
