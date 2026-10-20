//! 缺失账户的只记账开户；共享真实身份及 clean-compose 会话守卫，不采集余额。

use std::{sync::Arc, time::Duration};

use anyhow::Context;
use auction_application::{
    FundingCommitOutcome, FundingPreview, FundingReconciliationReader, FundingRecordOnlyCommand,
    FundingRecordOnlyStore,
};
use auction_config::{AppConfig, Role};
use auction_integrations::mooglemail::MoogleMailWorld;
use auction_storage::DatabaseStore;
use uuid::Uuid;

use super::{BotFundsRecordOnlyArgs, output};

/// 先只读预览或重放，再在共享预算和证据守卫内执行单次开户。
pub(crate) async fn run(args: BotFundsRecordOnlyArgs) -> anyhow::Result<()> {
    let execution = args.validate()?;
    let config = AppConfig::load(Role::Worker).context("加载 Worker 配置失败")?;
    let store = Arc::new(
        DatabaseStore::connect(config.database_url().context("Worker 未配置 PostgreSQL")?)
            .await
            .context("连接 PostgreSQL 失败")?,
    );
    let Some(execution) = execution else {
        let preview = store.preview_funding(args.expected_uid.clone()).await?;
        ensure_initializable(&preview)?;
        // 这里只生成确认模板；缺失账户仍由写入 owner 在锁内重新核验。
        let template = FundingRecordOnlyCommand {
            command_id: Uuid::nil(),
            actor: String::new(),
            expected_uid: args.expected_uid.clone(),
            expected_version: preview.account_version,
            preview_digest: preview.digest,
            confirm: String::new(),
        };
        return output::preview(
            &args.expected_uid,
            preview.account_version,
            &preview.digest,
            template.confirmation(),
            &preview,
        );
    };
    let command = FundingRecordOnlyCommand {
        command_id: execution.command_id,
        actor: execution.actor,
        expected_uid: args.expected_uid,
        expected_version: execution.expected_version,
        preview_digest: execution.preview_digest,
        confirm: execution.confirm_write,
    };
    command.validate()?;
    // 先恢复原命令，避免已提交开户使当前预览拒绝，或要求重试重新取得远端 owner。
    if let Some(result) = store.replay_record_only(command.clone()).await? {
        return output::execution(&command.expected_uid, command.command_id, "replayed", &result);
    }
    let preview = store.preview_funding(command.expected_uid.clone()).await?;
    ensure_initializable(&preview)?;
    anyhow::ensure!(
        preview.account_version == command.expected_version
            && preview.digest == command.preview_digest,
        "版本或责任摘要与原预览不一致，请重新预览；未知命令须保留原参数"
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let mut commit_started = false;
    let outcome = tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), async {
        let request = command.evidence_request(deadline)?;
        let reader =
            super::super::bot_funds::build_for_world(&config, store.clone(), MoogleMailWorld::Main)
                .await
                .context("证据装配失败；请核对 Worker 已停止及 active 凭据")?;
        let mut prepared = reader
            .prepare_internal_evidence(request)
            .await
            .map_err(|error| anyhow::anyhow!("真实身份或干净草稿证据未通过：{error}"))?;
        prepared.check_current().await.context("证据 owner 已失效或超时")?;
        let guard = prepared.begin_commit()?;
        commit_started = true;
        let outcome = store.initialize_record_only(&command, guard.snapshot()).await;
        match outcome {
            Ok(FundingCommitOutcome::Committed(result)) => {
                guard.confirm_committed();
                Ok(FundingCommitOutcome::Committed(result))
            }
            Ok(FundingCommitOutcome::RolledBack(error)) => {
                guard.confirm_rolled_back();
                Ok(FundingCommitOutcome::RolledBack(error))
            }
            Err(error) => Err(anyhow::Error::from(error)),
        }
    })
    .await;
    match outcome {
        Ok(Ok(FundingCommitOutcome::Committed(result))) => {
            output::execution(&command.expected_uid, command.command_id, "committed", &result)
        }
        Ok(Ok(FundingCommitOutcome::RolledBack(error))) => {
            output::execution(
                &command.expected_uid,
                command.command_id,
                "rolled_back",
                &error.to_string(),
            )?;
            Err(anyhow::Error::from(error)).context("只记账开户已确定回滚")
        }
        Ok(Err(error)) => {
            let outcome = if commit_started { "outcome_unknown" } else { "evidence_unavailable" };
            output::execution(
                &command.expected_uid,
                command.command_id,
                outcome,
                &error.to_string(),
            )?;
            anyhow::bail!("只记账开户未确认完成；请保留原 command-id 和完整参数重新查询")
        }
        Err(_) => {
            let outcome = if commit_started { "outcome_unknown" } else { "evidence_unavailable" };
            output::execution(
                &command.expected_uid,
                command.command_id,
                outcome,
                &"共享 30 秒预算已耗尽",
            )?;
            anyhow::bail!("只记账开户截止；请保留原 command-id 和完整参数重新查询")
        }
    }
}

// 适用资格由数据库 owner 随完整预览投影；在取得远端 owner 前明确拒绝已有账户。
fn ensure_initializable(preview: &FundingPreview) -> anyhow::Result<()> {
    anyhow::ensure!(
        preview.facts["record_only_initialization"]["eligible"].as_bool() == Some(true),
        "只记账开户要求全局尚无资金账户；已有账户需专门转换流程，本命令拒绝切换"
    );
    Ok(())
}
