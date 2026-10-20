//! 旧自寄记录的单笔装配；精确库存证据只在 opaque 提交守卫内借用。

use std::{sync::Arc, time::Duration};

use anyhow::Context;
use auction_application::{
    LegacySelfReconciliationCommand, LegacySelfReconciliationOutcome,
    LegacySelfReconciliationReader, LegacySelfReconciliationWriter,
};
use auction_config::{AppConfig, Role};
use auction_integrations::mooglemail::MoogleMailWorld;
use auction_storage::DatabaseStore;
use uuid::Uuid;

use super::{args::LegacyTargetArgs, output};

const OPERATION: &str = "self_send";

pub(super) async fn run(args: LegacyTargetArgs, reason: Option<String>) -> anyhow::Result<()> {
    let execution = args.validate()?;
    if args.execute {
        let reason = reason.as_deref().context("self-send execute 要求 --reason")?;
        anyhow::ensure!(
            !reason.is_empty()
                && reason == reason.trim()
                && reason.len() <= 512
                && !reason.chars().any(char::is_control),
            "--reason 必须是长度不超过 512 字节的非空核对原因"
        );
    }
    let config = AppConfig::load(Role::Worker).context("加载 Worker 配置失败")?;
    let store = Arc::new(
        DatabaseStore::connect(config.database_url().context("Worker 未配置 PostgreSQL")?)
            .await
            .context("连接 PostgreSQL 失败")?,
    );
    let Some(execution) = execution else {
        let target = store.preview_legacy_self(args.obligation_id).await?;
        // 只用于生成确认模板；预览不保存命令，也不装配任何远端 reader。
        let template = LegacySelfReconciliationCommand {
            command_id: Uuid::nil(),
            actor: String::new(),
            expected_uid: args.expected_uid.clone(),
            reason: String::new(),
            target: target.clone(),
        };
        return output::preview(
            OPERATION,
            &args,
            target.expected_fulfillment_version,
            &target.snapshot_digest,
            template.confirmation(),
            &target,
        );
    };

    // 已提交时必须恢复原冻结目标；不能先要求当前状态仍是旧 manual_reconcile。
    let target = match store.legacy_self_command_target(execution.command_id).await? {
        Some(target) => target,
        None => store.preview_legacy_self(args.obligation_id).await?,
    };
    anyhow::ensure!(
        target.obligation_id == args.obligation_id
            && target.expected_fulfillment_version == execution.state_version
            && target.snapshot_digest == execution.digest,
        "目标、版本或摘要与原预览不一致，请重新预览；未知命令须保留原参数"
    );
    let command = LegacySelfReconciliationCommand {
        command_id: execution.command_id,
        actor: execution.actor,
        expected_uid: args.expected_uid,
        reason: reason.context("self-send execute 要求 --reason")?,
        target,
    };
    command.validate()?;
    anyhow::ensure!(
        execution.confirm_write == command.confirmation(),
        "--confirm-write 与预览不一致"
    );
    if let Some(result) = store.replay_legacy_self(command.clone()).await? {
        return output::execution(
            OPERATION,
            args.obligation_id,
            command.command_id,
            "replayed",
            &result,
        );
    }
    let world =
        MoogleMailWorld::parse(command.target.world_name()).context("修复目标世界不受支持")?;
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let mut commit_started = false;
    let outcome = tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), async {
        // 原 advisory owner 是执行互斥门；Worker 须预先停止，不能与其并行读取/写入同账号。
        let reader = super::super::bot_funds::build_for_world(&config, store.clone(), world)
            .await
            .context("证据装配失败；请核对 Worker 已停止及 active 凭据")?;
        let mut prepared = reader
            .prepare_internal_evidence(command.evidence_request(deadline))
            .await
            // 该端口只返回闭集静态错误；保留分类，避免安全 JSON 仅剩通用上下文。
            .map_err(|error| anyhow::anyhow!("真实身份、干净草稿或精确库存证据未通过：{error}"))?;
        prepared.check_current().await.context("证据 owner 已失效或超时")?;
        let guard = prepared.begin_commit()?;
        commit_started = true;
        let outcome = store.reconcile_legacy_self(&command, guard.snapshot()).await;
        match outcome {
            Ok(LegacySelfReconciliationOutcome::Committed(result)) => {
                guard.confirm_committed();
                Ok(LegacySelfReconciliationOutcome::Committed(result))
            }
            Ok(LegacySelfReconciliationOutcome::RolledBack(error)) => {
                guard.confirm_rolled_back();
                Ok(LegacySelfReconciliationOutcome::RolledBack(error))
            }
            Err(error) => Err(anyhow::Error::from(error)),
        }
    })
    .await;
    match outcome {
        Ok(Ok(LegacySelfReconciliationOutcome::Committed(result))) => output::execution(
            OPERATION,
            args.obligation_id,
            command.command_id,
            "committed",
            &result,
        ),
        Ok(Ok(LegacySelfReconciliationOutcome::RolledBack(error))) => {
            output::execution(
                OPERATION,
                args.obligation_id,
                command.command_id,
                "rolled_back",
                &error.to_string(),
            )?;
            Err(anyhow::Error::from(error)).context("旧自寄修复已确定回滚")
        }
        Ok(Err(error)) => {
            let outcome = if commit_started { "outcome_unknown" } else { "evidence_unavailable" };
            output::execution(
                OPERATION,
                args.obligation_id,
                command.command_id,
                outcome,
                &error.to_string(),
            )?;
            anyhow::bail!("旧自寄修复未确认完成；请保留原 command-id 和完整参数重新查询")
        }
        Err(_) => {
            let outcome = if commit_started { "outcome_unknown" } else { "evidence_unavailable" };
            output::execution(
                OPERATION,
                args.obligation_id,
                command.command_id,
                outcome,
                &"共享 30 秒预算已耗尽",
            )?;
            anyhow::bail!("旧自寄修复截止；请保留原 command-id 和完整参数重新查询")
        }
    }
}
