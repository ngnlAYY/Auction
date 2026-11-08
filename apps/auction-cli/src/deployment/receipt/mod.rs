//! 部署回执：分体部署每次 `service start/restart` 的结果写入状态目录 `<state-dir>/receipts/`，
//! 由 `auction-cli service receipt list/show` 查询，并作为 `[backup.scope].deployment_receipts_dir`
//! 的来源随备份归档。回执只追加，不参与启停决策，也没有独立状态机。

use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

use anyhow::Context;
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};

use crate::cli::{ReceiptArgs, ReceiptCommand};
use crate::support::files;

mod model;
mod store;

#[cfg(feature = "postgres")]
pub(crate) use model::DeployRecord;
pub(crate) use model::{
    Action, Artifact, DeploymentReceipt, MAX_FAILURE_CHARS, MigrationRecord, Outcome,
    validate_actor, validate_backup_id,
};
pub(crate) use store::{append, list, read, receipts_dir};

/// 一次部署调用的回执上下文；显式参数优先于进程环境。
pub(crate) struct ReceiptContext {
    actor: String,
    actor_uid: u32,
    backup_id: Option<String>,
}

impl ReceiptContext {
    pub(crate) fn resolve(backup_id: Option<&str>, actor: Option<&str>) -> anyhow::Result<Self> {
        let actor = match actor {
            Some(value) => {
                validate_actor(value)?;
                value.to_owned()
            }
            None => environment_actor()?,
        };
        let backup_id = match backup_id {
            Some(value) => {
                validate_backup_id(value)?;
                Some(value.to_owned())
            }
            None => None,
        };
        Ok(Self { actor, actor_uid: files::effective_uid(), backup_id })
    }

    /// deploy 在停机备份发布后才能确定恢复点标识；回执仍只写一次。
    #[cfg(feature = "postgres")]
    pub(crate) fn set_backup_id(&mut self, backup_id: Option<&str>) {
        self.backup_id = backup_id.map(str::to_owned);
    }
}

/// 依次尝试 `SUDO_USER`、`USER`、`LOGNAME`；都无法解析时退化为有效 UID。
fn environment_actor() -> anyhow::Result<String> {
    for key in ["SUDO_USER", "USER", "LOGNAME"] {
        if let Ok(value) = std::env::var(key) {
            let value = value.trim();
            if validate_actor(value).is_ok() {
                return Ok(value.to_owned());
            }
        }
    }
    Ok(format!("uid:{}", files::effective_uid()))
}

/// 生成并追加一条回执；返回已发布的事实供调用方在 stdout 中回显标识。
#[allow(clippy::too_many_arguments)]
pub(crate) fn record(
    state_dir: &Path,
    action: Action,
    outcome: Outcome,
    started_at: DateTime<Utc>,
    context: &ReceiptContext,
    migration: Option<MigrationRecord>,
    artifacts: Vec<Artifact>,
    failure: Option<String>,
) -> anyhow::Result<DeploymentReceipt> {
    let receipt = DeploymentReceipt {
        format: model::FORMAT.to_owned(),
        receipt_id: uuid::Uuid::new_v4(),
        action,
        outcome,
        started_at,
        finished_at: Utc::now(),
        actor: context.actor.clone(),
        actor_uid: context.actor_uid,
        backup_id: context.backup_id.clone(),
        migration,
        artifacts,
        failure,
        deploy: None,
    };
    append(state_dir, &receipt)?;
    Ok(receipt)
}

/// `deploy`/`rollback` 回执：追加同一格式，额外记录制品、演练、备份与回滚证据。
///
/// `receipt_id` 由调用方在保留旧程序前分配，并与 `RollbackRecord::deploy_receipt_id`
/// 保持同一标识；`rollback` 依赖该字段按原标识读回部署回执。
#[cfg(feature = "postgres")]
#[allow(clippy::too_many_arguments)]
pub(crate) fn record_deploy(
    state_dir: &Path,
    receipt_id: uuid::Uuid,
    action: Action,
    outcome: Outcome,
    started_at: DateTime<Utc>,
    context: &ReceiptContext,
    migration: Option<MigrationRecord>,
    artifacts: Vec<Artifact>,
    deploy: DeployRecord,
    failure: Option<String>,
) -> anyhow::Result<DeploymentReceipt> {
    if receipt_id.is_nil() {
        anyhow::bail!("部署回执标识不能是 nil UUID");
    }
    let receipt = DeploymentReceipt {
        format: model::FORMAT.to_owned(),
        receipt_id,
        action,
        outcome,
        started_at,
        finished_at: Utc::now(),
        actor: context.actor.clone(),
        actor_uid: context.actor_uid,
        backup_id: context.backup_id.clone(),
        migration,
        artifacts,
        failure,
        deploy: Some(deploy),
    };
    append(state_dir, &receipt)?;
    Ok(receipt)
}

/// 失败摘要只保留单行有限文本，不持久化完整错误链。
pub(crate) fn failure_text(error: &anyhow::Error) -> String {
    let text: String = format!("{error:#}")
        .chars()
        .map(|character| if character.is_control() { ' ' } else { character })
        .take(MAX_FAILURE_CHARS)
        .collect();
    let text = text.trim();
    if text.is_empty() { "部署操作失败".to_owned() } else { text.to_owned() }
}

/// 以运行中进程的 `/proc/<pid>/exe` 描述符计算程序摘要；路径不可读时仍记录角色，
/// 但摘要与字节数为 null，不能把未核实的程序身份写成已验证。
pub(crate) fn artifact_from_process(role: &str, pid: u32) -> Artifact {
    let mut artifact = Artifact::role(role).unwrap_or_else(|_| Artifact {
        role: role.to_owned(),
        path: None,
        sha256: None,
        bytes: None,
    });
    if pid == 0 {
        return artifact;
    }
    let executable = format!("/proc/{pid}/exe");
    artifact.path = std::fs::read_link(&executable)
        .ok()
        .and_then(|path| path.to_str().map(|path| path.trim_end_matches(" (deleted)").to_owned()));
    if let Ok((sha256, bytes)) = sha256_file(&executable) {
        artifact.sha256 = Some(sha256);
        artifact.bytes = Some(bytes);
    }
    artifact
}

fn sha256_file(path: &str) -> std::io::Result<(String, u64)> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    let mut total = 0_u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
        total += u64::try_from(read).unwrap_or_default();
    }
    Ok((lower_hex(&digest.finalize()), total))
}

fn lower_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        value.push(HEX[usize::from(byte >> 4)] as char);
        value.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    value
}

/// 只读查询：不取部署锁、不连接数据库、不创建或修改回执目录。
pub(crate) fn query(state_dir: &Path, args: ReceiptArgs) -> anyhow::Result<()> {
    match args.command {
        ReceiptCommand::List { limit } => {
            let receipts = list(state_dir, usize::try_from(limit)?)?;
            crate::app::print_json(&serde_json::json!({
                "directory": receipts_directory(state_dir),
                "count": receipts.len(),
                "receipts": receipts,
            }))
        }
        ReceiptCommand::Show { receipt_id } => {
            let receipt_id =
                uuid::Uuid::parse_str(&receipt_id).context("receipt_id 必须是 UUID 文本")?;
            crate::app::print_json(&read(state_dir, receipt_id)?)
        }
    }
}

fn receipts_directory(state_dir: &Path) -> PathBuf {
    receipts_dir(state_dir)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use chrono::Utc;
    use sha2::Digest;

    use super::{
        Action, Outcome, ReceiptContext, failure_text, lower_hex,
        model::{Artifact, MigrationRecord},
        query, record,
    };
    use crate::cli::{ReceiptArgs, ReceiptCommand};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    struct Fixture {
        root: std::path::PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "auction-deployment-receipt-context-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&root).expect("创建测试根目录");
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))
                .expect("设置测试根目录权限");
            Self { root }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn record_then_query_round_trips() {
        let fixture = Fixture::new();
        let context = ReceiptContext::resolve(Some("backup-1"), Some("operator")).expect("上下文");
        let receipt = record(
            &fixture.root,
            Action::Restart,
            Outcome::Succeeded,
            Utc::now(),
            &context,
            Some(MigrationRecord {
                before_version: 95,
                after_version: 96,
                expected_version: 96,
                applied: 1,
            }),
            vec![Artifact {
                role: "public".to_owned(),
                path: Some("/opt/auction/bin/auction-public-api".to_owned()),
                sha256: Some("b".repeat(64)),
                bytes: Some(2048),
            }],
            None,
        )
        .expect("写入回执");
        assert_eq!(receipt.actor, "operator");
        assert_eq!(receipt.actor_uid, crate::support::files::effective_uid());
        assert_eq!(receipt.backup_id.as_deref(), Some("backup-1"));

        query(&fixture.root, ReceiptArgs { command: ReceiptCommand::List { limit: 10 } })
            .expect("列出回执");
        query(
            &fixture.root,
            ReceiptArgs {
                command: ReceiptCommand::Show { receipt_id: receipt.receipt_id.to_string() },
            },
        )
        .expect("显示回执");
    }

    #[test]
    fn context_rejects_invalid_explicit_values() {
        assert!(ReceiptContext::resolve(Some(""), None).is_err());
        assert!(ReceiptContext::resolve(None, Some("bad\nactor")).is_err());
    }

    #[test]
    fn failure_text_is_single_line_and_bounded() {
        let error = anyhow::anyhow!("first line\nsecond\u{0} line");
        let text = failure_text(&error);
        assert!(!text.contains('\n'));
        assert!(!text.contains('\u{0}'));
        assert!(text.len() <= super::MAX_FAILURE_CHARS);
        assert_eq!(failure_text(&anyhow::anyhow!("")), "部署操作失败");
    }

    #[test]
    fn lower_hex_matches_sha256_known_value() {
        assert_eq!(
            lower_hex(&sha2::Sha256::digest(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
