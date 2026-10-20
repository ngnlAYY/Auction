//! 旧已收款单 lot 的责任认领；读取同会话 Main 现金与精确原 Sent，不执行远端写入。

use std::{sync::Arc, time::Duration};

use anyhow::Context;
use auction_application::{
    LegacyBotSellerCommand, LegacyBotSellerCommitOutcome, LegacyBotSellerStore,
};
use auction_config::{AppConfig, Role};
use auction_integrations::mooglemail::MoogleMailWorld;
use auction_storage::DatabaseStore;
use uuid::Uuid;

use super::{args::LegacyTargetArgs, output};

const OPERATION: &str = "adopt_collected";

pub(super) async fn run(args: LegacyTargetArgs) -> anyhow::Result<()> {
    let execution = args.validate()?;
    let config = AppConfig::load(Role::Worker).context("加载 Worker 配置失败")?;
    let store = Arc::new(
        DatabaseStore::connect(config.database_url().context("Worker 未配置 PostgreSQL")?)
            .await
            .context("连接 PostgreSQL 失败")?,
    );
    let Some(execution) = execution else {
        let preview =
            store.preview_legacy_bot_seller(args.obligation_id, args.expected_uid.clone()).await?;
        // 只生成版本与摘要绑定的确认模板；没有 command_id 便没有任何写入授权。
        let template = LegacyBotSellerCommand {
            command_id: Uuid::nil(),
            actor: String::new(),
            obligation_id: args.obligation_id,
            expected_uid: args.expected_uid.clone(),
            expected_state_version: preview.state_version,
            expected_digest: preview.digest,
        };
        return output::preview(
            OPERATION,
            &args,
            preview.state_version,
            &preview.digest,
            template.confirmation(),
            &preview,
        );
    };
    let command = LegacyBotSellerCommand {
        command_id: execution.command_id,
        actor: execution.actor,
        obligation_id: args.obligation_id,
        expected_uid: args.expected_uid,
        expected_state_version: execution.state_version,
        expected_digest: execution.digest,
    };
    command.validate()?;
    anyhow::ensure!(
        execution.confirm_write == command.confirmation(),
        "--confirm-write 与预览不一致"
    );
    // 原命令恢复先于当前预览和远端装配；已完成的义务不能阻断同 ID/同负载重放。
    if let Some(result) = store.replay_legacy_bot_seller(command.clone()).await? {
        return output::execution(
            OPERATION,
            args.obligation_id,
            command.command_id,
            "replayed",
            &result,
        );
    }
    let preview =
        store.preview_legacy_bot_seller(args.obligation_id, command.expected_uid.clone()).await?;
    anyhow::ensure!(
        preview.obligation_id == args.obligation_id
            && preview.state_version == command.expected_state_version
            && preview.digest == command.expected_digest,
        "目标、版本或摘要与原预览不一致，请重新预览；未知命令须保留原参数"
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let mut commit_started = false;
    let outcome = tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), async {
        let request = preview.evidence_request(&command, deadline)?;
        let reader =
            super::super::bot_funds::build_for_world(&config, store.clone(), MoogleMailWorld::Main)
                .await
                .context("证据装配失败；请核对 Worker 已停止及 active 凭据")?;
        let mut prepared = reader
            .prepare_internal_evidence_with_sent(request, preview.sent_target())
            .await
            // 仅公开端口的闭集静态分类，不展开集成错误或远端正文。
            .map_err(|error| {
                anyhow::anyhow!("真实联合身份、Main 现金或精确原 Sent 领取证据未通过：{error}")
            })?;
        prepared.check_current().await.context("证据 owner 已失效或超时")?;
        let guard = prepared.begin_commit()?;
        commit_started = true;
        let outcome = store.claim_legacy_bot_seller(&command, guard.snapshot()).await;
        match outcome {
            Ok(LegacyBotSellerCommitOutcome::Committed(result)) => {
                guard.confirm_committed();
                Ok(LegacyBotSellerCommitOutcome::Committed(result))
            }
            Ok(LegacyBotSellerCommitOutcome::FundingDeferred) => {
                // 资金门的安全处置已确定提交，不能当成业务回滚或认领成功。
                guard.confirm_committed();
                Ok(LegacyBotSellerCommitOutcome::FundingDeferred)
            }
            Ok(LegacyBotSellerCommitOutcome::RolledBack(error)) => {
                guard.confirm_rolled_back();
                Ok(LegacyBotSellerCommitOutcome::RolledBack(error))
            }
            Err(error) => Err(anyhow::Error::from(error)),
        }
    })
    .await;
    match outcome {
        Ok(Ok(LegacyBotSellerCommitOutcome::Committed(result))) => output::execution(
            OPERATION,
            args.obligation_id,
            command.command_id,
            "committed",
            &result,
        ),
        Ok(Ok(LegacyBotSellerCommitOutcome::FundingDeferred)) => {
            output::execution(
                OPERATION,
                args.obligation_id,
                command.command_id,
                "funding_deferred",
                &"资金门保留冻结或未知占用，认领尚未完成",
            )?;
            anyhow::bail!("资金门保留冻结或未知占用，认领尚未完成；请核对原命令与资金账户")
        }
        Ok(Ok(LegacyBotSellerCommitOutcome::RolledBack(error))) => {
            output::execution(
                OPERATION,
                args.obligation_id,
                command.command_id,
                "rolled_back",
                &error.to_string(),
            )?;
            Err(anyhow::Error::from(error)).context("历史卖家责任认领已确定回滚")
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
            anyhow::bail!("历史卖家责任认领未确认完成；请保留原 command-id 和完整参数重新查询")
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
            anyhow::bail!("历史卖家责任认领截止；请保留原 command-id 和完整参数重新查询")
        }
    }
}
