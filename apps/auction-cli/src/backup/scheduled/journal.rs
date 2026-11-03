//! 状态目录独占锁内的运行记录与中断收口；终态只追加，运行中状态只有一个可替换文件。

use std::{fs::File, path::Path};

use anyhow::Context;
use chrono::Utc;
use uuid::Uuid;

use super::filesystem::{FileIdentity, PrivateDirectory};
use super::model::{FORMAT, Failure, Outcome, RunReceipt};
use crate::app::progress;

const RUNNING_FILE: &str = "running.json";

pub(super) struct Journal {
    pub(super) state: PrivateDirectory,
    pub(super) receipts: PrivateDirectory,
    _lock: File,
}

impl Journal {
    /// 先锁定状态目录；失败不写运行记录或回执，目录仍由当前持锁进程拥有。
    pub(super) fn begin(state_dir: &Path) -> anyhow::Result<Self> {
        let state = PrivateDirectory::prepare(state_dir)?;
        let lock = state.try_lock("scheduled.lock")?.context("已有定时备份在运行")?;
        let receipts = PrivateDirectory::prepare(&state.path.join("receipts"))?;
        Ok(Self { state, receipts, _lock: lock })
    }

    pub(super) fn checkpoint(&self, receipt: &RunReceipt) -> anyhow::Result<()> {
        self.state.write(RUNNING_FILE, &serde_json::to_vec_pretty(receipt)?, true)
    }

    /// 终态发布并 fsync 后才清除本次运行记录；残留的旧记录在恢复失败时保持原样。
    pub(super) fn finish(&self, receipt: &RunReceipt) -> anyhow::Result<()> {
        self.publish(receipt)?;
        if self.remove_running(receipt.run_id).is_err() {
            progress(format_args!(
                "backup run_id={} running_cleanup=failed；下次启动按已有终态收口",
                receipt.run_id
            ));
        }
        Ok(())
    }

    fn publish(&self, receipt: &RunReceipt) -> anyhow::Result<()> {
        self.receipts.write(&receipt.receipt_name(), &serde_json::to_vec_pretty(receipt)?, false)
    }

    /// 持锁后收口残留记录；无效证据原样改名保留，不能阻断不依赖它的新备份。
    pub(super) fn recover(&self, current: &mut RunReceipt) -> anyhow::Result<bool> {
        let bytes = match self.state.read(RUNNING_FILE) {
            Ok(bytes) => bytes,
            Err(error) if is_missing(&error) => return Ok(false),
            Err(error) if is_invalid_data(&error) => {
                return self.retain_invalid(current, "running_record_invalid");
            }
            Err(error) => return Err(error),
        };
        let mut previous: RunReceipt = match serde_json::from_slice(&bytes) {
            Ok(previous) => previous,
            Err(_) => return self.retain_invalid(current, "running_record_invalid"),
        };
        if previous.format != FORMAT
            || previous.run_id.is_nil()
            || previous.outcome != Outcome::Running
            || previous.finished_at.is_some()
        {
            return self.retain_invalid(current, "running_record_invalid");
        }
        let interrupted = match self.receipts.read(&previous.receipt_name()) {
            Ok(bytes) => {
                match serde_json::from_slice::<RunReceipt>(&bytes) {
                    Ok(terminal) if terminal_matches(&previous, &terminal) => {}
                    _ => return self.retain_invalid(current, "terminal_receipt_conflict"),
                }
                false
            }
            Err(error) if is_invalid_data(&error) => {
                return self.retain_invalid(current, "terminal_receipt_conflict");
            }
            Err(error) if is_missing(&error) => {
                previous.outcome = Outcome::Interrupted;
                previous.finished_at = Some(Utc::now());
                // 保留最后 checkpoint 的已知耗时，不把停机时长当作执行耗时，也不补发打卡。
                previous.failure =
                    Some(Failure { stage: previous.stage, error_code: "interrupted".to_owned() });
                self.publish(&previous)?;
                progress(format_args!("backup run_id={} outcome=interrupted", previous.run_id));
                true
            }
            Err(error) => return Err(error),
        };
        self.remove_running(previous.run_id)?;
        Ok(interrupted)
    }

    fn retain_invalid(
        &self,
        current: &mut RunReceipt,
        reason: &'static str,
    ) -> anyhow::Result<bool> {
        let name = format!(
            "running.invalid-{}-{}.json",
            Utc::now().format("%Y%m%dT%H%M%SZ"),
            current.run_id
        );
        self.state.publish_file(RUNNING_FILE, &name)?;
        current.recovery_warnings.push(reason.to_owned());
        progress(format_args!("warning={reason} retained_file={name}"));
        Ok(false)
    }

    fn remove_running(&self, run_id: Uuid) -> anyhow::Result<()> {
        let bytes = match self.state.read(RUNNING_FILE) {
            Ok(bytes) => bytes,
            Err(error) if is_missing(&error) => return Ok(()),
            Err(error) => return Err(error),
        };
        let running: RunReceipt = serde_json::from_slice(&bytes).context("解析运行记录失败")?;
        if running.run_id == run_id {
            let identity = FileIdentity::of(&self.state.open(RUNNING_FILE)?.metadata()?);
            self.state.remove(RUNNING_FILE, identity)?;
        }
        Ok(())
    }
}

fn is_missing(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound)
    })
}

fn is_invalid_data(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|error| error.kind() == std::io::ErrorKind::InvalidData)
    })
}

/// 只比较不会随阶段完成而变化的身份与已知 archive 证据；耗时、清理和校验标志可正常推进。
fn terminal_matches(running: &RunReceipt, terminal: &RunReceipt) -> bool {
    if terminal.format != FORMAT
        || terminal.run_id != running.run_id
        || terminal.started_at != running.started_at
        || terminal.database_name_sha256 != running.database_name_sha256
        || terminal.output_dir != running.output_dir
        || terminal.outcome == Outcome::Running
        || terminal.finished_at.is_none()
    {
        return false;
    }
    running.archive.as_ref().is_none_or(|archive| {
        terminal.archive.as_ref().is_some_and(|completed| {
            archive.archive_file_name == completed.archive_file_name
                && archive.manifest_file_name == completed.manifest_file_name
                && archive.archive_sha256 == completed.archive_sha256
                && archive.manifest_sha256 == completed.manifest_sha256
                && archive.schema_version == completed.schema_version
                && archive.created_at == completed.created_at
                && archive.archive_identity == completed.archive_identity
                && archive.manifest_identity == completed.manifest_identity
        })
    })
}
