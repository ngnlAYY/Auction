//! Bot 资金账户：数据库只读预览、record_only 开户与 strict_cash 现金对账。
//! 写入前先按原命令重放；真实身份/现金只在显式执行时经同一 opaque guard 取得，30 秒共享预算。

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::Context;
use auction_application::{
    FundingCashGateProvider, FundingCommitOutcome, FundingPreview, FundingReconciliationCommand,
    FundingReconciliationReader, FundingReconciliationService, FundingRecordOnlyCommand,
    FundingRecordOnlyStore, lower_hex,
};
use auction_config::AppConfig;
use auction_storage::DatabaseStore;
use serde_json::json;
use uuid::Uuid;

use crate::app::print_json;
use crate::cli::{FundsCommand, FundsWriteArgs};
use crate::deployment::ConfigDir;
use crate::support::text;

mod evidence;

const BUDGET: Duration = Duration::from_secs(30);

pub(crate) async fn run(config_dir: &ConfigDir, command: FundsCommand) -> anyhow::Result<()> {
    let (config, store) = super::worker_store(config_dir).await?;
    let result = async {
        match command {
            FundsCommand::Preview { uid } => preview(&store, uid).await,
            FundsCommand::OpenRecordOnly { write } => {
                open_record_only(&config, &store, write).await
            }
            FundsCommand::Reconcile {
                write,
                proprietary_credits,
                compensation_credits,
                dedicated_account,
            } => {
                let command = FundingReconciliationCommand {
                    command_id: text::command_id(write.command_id, "--command-id")?,
                    actor: text::actor(&write.actor)?.to_owned(),
                    expected_uid: write.uid,
                    expected_version: write.expected_version,
                    preview_digest: text::sha256_hex(&write.preview_digest)?,
                    proprietary_credits,
                    compensation_credits,
                    confirm: write.confirm,
                    dedicated_account_no_external_liabilities: dedicated_account,
                };
                reconcile(&config, &store, command).await
            }
        }
    }
    .await;
    store.postgres_pool().close().await;
    result
}

async fn preview(store: &DatabaseStore, uid: String) -> anyhow::Result<()> {
    let preview = store.preview_funding(uid.clone()).await.context("读取资金预览失败")?;
    let digest = lower_hex(&preview.digest);
    let record_only = FundingRecordOnlyCommand {
        command_id: Uuid::nil(),
        actor: String::new(),
        expected_uid: uid.clone(),
        expected_version: preview.account_version,
        preview_digest: preview.digest,
        confirm: String::new(),
    };
    let mut projection = serde_json::to_value(&preview)?;
    // 摘要只以规范十六进制输出一次，避免字节数组与 preview_digest 并存造成误抄。
    if let Some(object) = projection.as_object_mut() {
        object.remove("digest");
    }
    print_json(&json!({
        "preview": projection,
        "preview_digest": digest,
        "record_only_eligible": record_only_eligible(&preview),
        "confirm_templates": {
            "open_record_only": record_only.confirmation(),
            "reconcile": format!(
                "RECONCILE BOT FUNDS {uid} {} <PROPRIETARY> <COMPENSATION> {digest}",
                preview.account_version
            ),
        },
        "note": "只读预览：未核验远端身份或余额，不代表任何可用额度",
    }))
}

/// 开户资格由数据库 owner 投影；已有账户不能经本命令切换为 record_only。
fn record_only_eligible(preview: &FundingPreview) -> bool {
    preview.facts["record_only_initialization"]["eligible"].as_bool() == Some(true)
}

async fn open_record_only(
    config: &AppConfig,
    store: &Arc<DatabaseStore>,
    write: FundsWriteArgs,
) -> anyhow::Result<()> {
    let command = FundingRecordOnlyCommand {
        command_id: text::command_id(write.command_id, "--command-id")?,
        actor: text::actor(&write.actor)?.to_owned(),
        expected_uid: write.uid,
        expected_version: write.expected_version,
        preview_digest: text::sha256_hex(&write.preview_digest)?,
        confirm: write.confirm,
    };
    command.validate().map_err(|_| {
        anyhow::anyhow!("参数或确认短语无效；须逐字使用 funds preview 输出的 open_record_only 模板")
    })?;
    // 先恢复原命令，已提交的开户不应因当前预览变化而被拒绝。
    if let Some(result) = store.replay_record_only(command.clone()).await? {
        return print_json(&json!({ "outcome": "replayed", "result": result }));
    }
    let preview = store.preview_funding(command.expected_uid.clone()).await?;
    if !record_only_eligible(&preview) {
        anyhow::bail!("已存在资金账户，record_only 开户被拒绝");
    }
    if preview.account_version != command.expected_version
        || preview.digest != command.preview_digest
    {
        anyhow::bail!("账户版本或责任摘要已变化；请重新 preview（未知命令须保留原参数）");
    }
    let deadline = Instant::now() + BUDGET;
    let mut commit_started = false;
    let outcome = tokio::time::timeout(BUDGET, async {
        let request = command.evidence_request(deadline)?;
        let reader = evidence::build(config, Arc::clone(store)).await?;
        let mut prepared = reader
            .prepare_internal_evidence(request)
            .await
            .map_err(|error| anyhow::anyhow!("真实身份或干净草稿证据未通过：{error}"))?;
        prepared.check_current().await.context("证据 owner 已失效或超时")?;
        let guard = prepared.begin_commit()?;
        commit_started = true;
        match store.initialize_record_only(&command, guard.snapshot()).await {
            Ok(FundingCommitOutcome::Committed(result)) => {
                guard.confirm_committed();
                Ok(Ok(result))
            }
            Ok(FundingCommitOutcome::RolledBack(error)) => {
                guard.confirm_rolled_back();
                Ok(Err(error))
            }
            Err(error) => Err(anyhow::Error::from(error)),
        }
    })
    .await;
    match outcome {
        Ok(Ok(Ok(result))) => print_json(&json!({ "outcome": "committed", "result": result })),
        Ok(Ok(Err(error))) => {
            print_json(&json!({ "outcome": "rolled_back", "error": error.to_string() }))?;
            anyhow::bail!("record_only 开户已确定回滚：{error}")
        }
        Ok(Err(error)) => unknown(commit_started, &error.to_string()),
        Err(_) => unknown(commit_started, "30 秒预算已耗尽"),
    }
}

async fn reconcile(
    config: &AppConfig,
    store: &Arc<DatabaseStore>,
    command: FundingReconciliationCommand,
) -> anyhow::Result<()> {
    command.validate().map_err(|_| {
        anyhow::anyhow!(
            "参数或确认短语无效；须声明 --dedicated-account，并按 funds preview 的 reconcile 模板填入额度"
        )
    })?;
    if let Some(result) = store.replay_funding(command.clone()).await? {
        print_json(&json!({ "outcome": "replayed", "result": result }))?;
        return ready(&result.account_status);
    }
    store.funding_cash_gate().context("完整现金发送门尚未接线，不能执行对账")?;
    let result = tokio::time::timeout(BUDGET, async {
        let reader = evidence::build(config, Arc::clone(store)).await?;
        let service =
            FundingReconciliationService::new(store.clone(), store.clone(), reader, store.clone());
        service.apply(command, Instant::now() + BUDGET).await.map_err(anyhow::Error::from)
    })
    .await
    .context("资金对账超出 30 秒预算；如已开始提交，必须按原 command-id 与参数重新执行核对")??;
    print_json(&json!({ "outcome": "committed", "result": result }))?;
    ready(&result.account_status)
}

fn ready(status: &str) -> anyhow::Result<()> {
    if status == "ready" {
        Ok(())
    } else {
        anyhow::bail!("资金账户状态为 {status}：存在账实缺口，新增支出已冻结")
    }
}

fn unknown(commit_started: bool, reason: &str) -> anyhow::Result<()> {
    let outcome = if commit_started { "outcome_unknown" } else { "evidence_unavailable" };
    print_json(&json!({ "outcome": outcome, "error": reason }))?;
    anyhow::bail!("开户未确认完成（{outcome}）；保留原 command-id 与全部参数重新执行以核对")
}
