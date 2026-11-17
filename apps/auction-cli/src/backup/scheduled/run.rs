//! 定时备份入口与总预算收口；先持状态目录锁，再恢复旧记录和加载 Worker 配置。
//!
//! 回执与打卡只含阶段与固定错误码。被归为错误码的错误另以一行受限摘要写到 stderr（systemd
//! 下进入 journal）：正文与手动命令打印的错误同源，只含已脱敏的上下文，不进入回执与打卡。

use std::{path::Path, time::Duration};

use auction_config::AppConfig;
use auction_time::Timestamp;
use tokio::time::Instant;
use uuid::Uuid;

use super::{
    heartbeat,
    journal::Journal,
    model::{Outcome, RunReceipt, Stage},
    offsite, pipeline,
};
use crate::{
    app::{print_json, progress},
    config_dir::ConfigDir,
    support::text,
};

/// 原因摘要的字符上限；规则与部署回执的失败文本相同（单行、控制字符换成空格），只是更短。
const MAX_DETAIL_CHARS: usize = 512;

pub(in crate::backup) async fn run(
    config_dir: &ConfigDir,
    database: &str,
    output_dir: &Path,
    state_dir: &Path,
) -> anyhow::Result<()> {
    run_reporting(config_dir, database, output_dir, state_dir, &mut |line| progress(line)).await
}

/// `report` 接收每个被归为错误码的错误的原因摘要行；生产入口写到 stderr。
async fn run_reporting(
    config_dir: &ConfigDir,
    database: &str,
    output_dir: &Path,
    state_dir: &Path,
    report: &mut impl FnMut(String),
) -> anyhow::Result<()> {
    let started = Instant::now();
    let journal = Journal::begin(state_dir)?;
    let mut receipt = RunReceipt::new(database, output_dir);
    let mut config = None;
    receipt.enter_stage(Stage::Recover);
    match journal.recover(&mut receipt) {
        Ok(interrupted) => receipt.recovered_interrupted = u64::from(interrupted),
        Err(error) => {
            report(detail_line(receipt.run_id, receipt.stage, &error));
            receipt.fail("recovery_failed");
        }
    }
    if receipt.failure.is_none() {
        // Config 阶段先落运行记录；加载失败也能发布终态，不依赖 Public/Admin 配置。
        if let Err(error) = load_config(config_dir, &journal, &mut receipt, &mut config) {
            report(detail_line(receipt.run_id, receipt.stage, &error));
            receipt.fail("config_error");
        }
    } else {
        // 收口失败时保留旧 running.json，仅读取失败打卡配置，不进入备份或清理。
        config = config_dir.load_worker().ok();
    }
    if receipt.failure.is_none() {
        if let Some(config) = config.as_ref() {
            let deadline = started + Duration::from_secs(config.backup.timeout_seconds);
            let result = if Instant::now() >= deadline {
                Err(())
            } else {
                tokio::time::timeout_at(
                    deadline,
                    pipeline::execute(&journal, config, database, &mut receipt),
                )
                .await
                .map_err(|_| ())
            };
            match result {
                Ok(Ok(())) if Instant::now() <= deadline && receipt.failure.is_none() => {
                    receipt.outcome = Outcome::Completed;
                }
                Ok(Ok(())) if receipt.failure.is_some() => receipt.outcome = Outcome::Failed,
                Ok(Ok(())) => offsite::timeout(&mut receipt),
                Ok(Err(error)) => {
                    report(detail_line(receipt.run_id, receipt.stage, &error));
                    if receipt.failure.is_some() {
                        receipt.outcome = Outcome::Failed;
                    } else {
                        let code = receipt.stage.error_code();
                        receipt.fail(code);
                    }
                }
                // 超时没有错误对象，固定码 timeout 已足以说明原因。
                Err(()) => offsite::timeout(&mut receipt),
            }
        } else {
            receipt.fail("config_error");
        }
    }
    receipt.finished_at = Some(Timestamp::now());
    receipt.record_elapsed();
    if receipt.outcome == Outcome::Completed {
        receipt.stage = Stage::Receipt;
    }
    receipt.heartbeat_configured = heartbeat::address(config.as_ref(), receipt.outcome).is_some();
    if let Err(error) = journal.finish(&receipt) {
        receipt.stage = Stage::Receipt;
        report(detail_line(receipt.run_id, receipt.stage, &error));
        receipt.fail("receipt_failed");
        // 终态存储失败仍尝试一次失败打卡；运行记录保持原样供下次收口。
        heartbeat::send(&receipt, config.as_ref()).await;
        anyhow::bail!("定时备份失败：receipt_failed；请核对运行记录和状态目录");
    }
    heartbeat::send(&receipt, config.as_ref()).await;
    progress(format_args!(
        "backup run_id={} outcome={:?} stage={:?} error_code={:?}",
        receipt.run_id,
        receipt.outcome,
        receipt.stage,
        receipt.failure.as_ref().map(|failure| failure.error_code.as_str())
    ));
    print_json(&serde_json::json!({
        "outcome": receipt.outcome, "run_id": receipt.run_id,
        "receipt": journal.receipts.path.join(receipt.receipt_name()),
        "failure": receipt.failure,
    }))?;
    if receipt.outcome != Outcome::Completed {
        anyhow::bail!("定时备份失败；请按回执的 stage/error_code 排查");
    }
    Ok(())
}

fn load_config(
    config_dir: &ConfigDir,
    journal: &Journal,
    receipt: &mut RunReceipt,
    available: &mut Option<AppConfig>,
) -> anyhow::Result<()> {
    pipeline::stage(journal, receipt, Stage::Config)?;
    let config = config_dir.load_worker()?;
    receipt.heartbeat_configured = config.backup_heartbeat_failure_url().is_some();
    *available = Some(config);
    if let Some(config) = available.as_ref() {
        config.validate_backup()?;
        receipt.policy = Some((&config.backup).into());
        receipt.offsite.enabled = config.backup.offsite.enabled;
    }
    journal.checkpoint(receipt)
}

/// 一行原因摘要：沿用手动命令的错误正文，控制字符换成空格并截断；错误链含任务 join 失败时
/// 只给出种类，与顶层错误输出一样不格式化 panic 内容。
fn detail_line(run_id: Uuid, stage: Stage, error: &anyhow::Error) -> String {
    let detail = match auction_process::task_error_kind(error.as_ref()) {
        Some(kind) => format!("任务异常：{kind}"),
        None => text::error_summary(error, MAX_DETAIL_CHARS)
            .unwrap_or_else(|| "错误没有正文".to_owned()),
    };
    format!("backup run_id={run_id} stage={stage:?} detail={detail}")
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

    use uuid::Uuid;

    use super::{MAX_DETAIL_CHARS, detail_line, run_reporting};
    use crate::backup::scheduled::model::Stage;
    use crate::config_dir::ConfigDir;

    struct TempRoot(PathBuf);

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn temp_root() -> TempRoot {
        let root = std::env::temp_dir().join(format!("auction-scheduled-run-{}", Uuid::new_v4()));
        fs::create_dir(&root).expect("建立临时目录");
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).expect("设置权限");
        TempRoot(root)
    }

    #[test]
    fn detail_line_is_a_bounded_single_line() {
        let run_id = Uuid::new_v4();
        let error = anyhow::anyhow!("第一行\n第二行\u{7}{}", "x".repeat(2 * MAX_DETAIL_CHARS))
            .context("创建备份失败");
        let line = detail_line(run_id, Stage::Create, &error);
        let prefix = format!("backup run_id={run_id} stage=Create detail=");
        let detail = line.strip_prefix(&prefix).expect("固定前缀");
        assert!(detail.starts_with("创建备份失败: 第一行 第二行 x"), "{line}");
        assert!(!line.chars().any(char::is_control), "{line}");
        assert_eq!(detail.chars().count(), MAX_DETAIL_CHARS);
        let empty = detail_line(run_id, Stage::Verify, &anyhow::anyhow!("\n"));
        assert!(empty.ends_with("stage=Verify detail=错误没有正文"), "{empty}");
    }

    /// 错误链含 panic 的 join 错误时只输出种类，panic 内容不进入 journal。
    #[tokio::test]
    async fn detail_line_does_not_format_panic_payloads() {
        let join_error =
            tokio::spawn(async { panic!("secret-panic-payload") }).await.expect_err("任务应 panic");
        let error = anyhow::Error::new(join_error).context("备份子任务失败");
        let line = detail_line(Uuid::new_v4(), Stage::Create, &error);
        assert!(line.ends_with("detail=任务异常：panic"), "{line}");
        assert!(!line.contains("secret-panic-payload"), "{line}");
    }

    /// 配置加载失败：回执仍只记固定码 config_error，原因摘要只交给报告行，且只有一行。
    #[tokio::test]
    async fn config_failure_reports_one_detail_line_outside_the_receipt() {
        let root = temp_root();
        let config = root.0.join("config");
        fs::create_dir(&config).expect("建立空配置目录");
        let config_dir = ConfigDir::new(&config).expect("配置目录");
        let state = root.0.join("state");
        let mut lines = Vec::new();
        let result =
            run_reporting(&config_dir, "auction", &root.0.join("out"), &state, &mut |line| {
                lines.push(line);
            })
            .await;
        assert!(result.is_err(), "配置缺失必须失败");
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].contains("stage=Config detail=加载 worker 配置失败"), "{lines:?}");

        let receipts = fs::read_dir(state.join("receipts"))
            .expect("回执目录")
            .map(|entry| entry.expect("目录项").path())
            .collect::<Vec<_>>();
        assert_eq!(receipts.len(), 1, "{receipts:?}");
        let text = fs::read_to_string(&receipts[0]).expect("读取回执");
        let receipt: serde_json::Value = serde_json::from_str(&text).expect("解析回执");
        assert_eq!(receipt["failure"]["error_code"], "config_error");
        assert_eq!(receipt["failure"]["stage"], "config");
        assert!(!text.contains("加载 worker 配置失败"), "回执不含错误正文：{text}");
    }
}
