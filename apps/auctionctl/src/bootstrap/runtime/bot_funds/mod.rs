//! bot资金只读预览/显式对账入口；preview不装配远端或取得owner。

#[cfg(feature = "postgres")]
mod evidence;
#[cfg(feature = "postgres")]
pub(super) use evidence::build_for_world;

use clap::Args;
use uuid::Uuid;

/// 每次写入要求完整精确确认；UID仅是目标断言，不替代远端认证。
#[derive(Debug, Args)]
pub(crate) struct BotFundsArgs {
    #[arg(long)]
    expected_uid: String,
    #[arg(long)]
    execute: bool,
    #[arg(long, requires = "execute")]
    command_id: Option<Uuid>,
    #[arg(long, requires = "execute")]
    actor: Option<String>,
    #[arg(long, requires = "execute")]
    expected_version: Option<i64>,
    #[arg(long, requires = "execute")]
    preview_digest: Option<String>,
    #[arg(long, requires = "execute")]
    proprietary_credits: Option<i64>,
    #[arg(long, requires = "execute")]
    compensation_credits: Option<i64>,
    #[arg(long, requires = "execute")]
    confirm_write: Option<String>,
    /// 确认专用账号无并发人工/其他程序资金写入、无未登记外部负债，全部程序已同版升级。
    #[arg(long, requires = "execute")]
    dedicated_account_no_external_liabilities: bool,
}

#[cfg(feature = "postgres")]
pub(super) async fn run(args: BotFundsArgs) -> anyhow::Result<()> {
    use anyhow::Context;
    use auction_application::{
        FundingCashGateProvider, FundingReconciliationCommand, FundingReconciliationReader,
        FundingReconciliationService,
    };
    use auction_config::{AppConfig, Role};
    use auction_storage::DatabaseStore;
    use std::{
        sync::Arc,
        time::{Duration, Instant},
    };

    let config = AppConfig::load(Role::Worker).context("load Worker configuration")?;
    let store = Arc::new(
        DatabaseStore::connect(config.database_url().context("Worker database unavailable")?)
            .await
            .context("connect PostgreSQL")?,
    );
    if !args.execute {
        let preview = store.preview_funding(args.expected_uid).await?;
        println!("{}", serde_json::to_string_pretty(&preview)?);
        let digest = super::hex_digest(&preview.digest);
        println!("preview_digest={digest}");
        println!(
            "只读预览：远端身份/余额尚未核验，无可用额度。apply要求专用账号、无并发资金写入/未登记负债、停止旧Worker并同版升级。确认格式：RECONCILE BOT FUNDS <UID> <VERSION> <PROPRIETARY> <COMPENSATION> <DIGEST>"
        );
        return Ok(());
    }
    let command = FundingReconciliationCommand {
        command_id: args.command_id.context("--command-id required")?,
        actor: args.actor.context("--actor required")?,
        expected_uid: args.expected_uid,
        expected_version: args.expected_version.context("--expected-version required")?,
        preview_digest: parse_digest(&args.preview_digest.context("--preview-digest required")?)?,
        proprietary_credits: args.proprietary_credits.context("--proprietary-credits required")?,
        compensation_credits: args
            .compensation_credits
            .context("--compensation-credits required")?,
        confirm: args.confirm_write.context("--confirm-write required")?,
        dedicated_account_no_external_liabilities: args.dedicated_account_no_external_liabilities,
    };
    command.validate()?;
    if let Some(result) = store.replay_funding(command.clone()).await? {
        println!("{}", serde_json::to_string_pretty(&result)?);
        anyhow::ensure!(
            result.account_status == "ready",
            "原命令结果：资金账户已冻结，需处理账实缺口"
        );
        return Ok(());
    }
    // 不接线就拒绝；D唯一实现提供V1，不能由CLI命令行或配置绕过。
    store.funding_cash_gate().context("完整现金发送门尚未接线，不能执行对账")?;
    let deadline =
        Instant::now().checked_add(Duration::from_secs(30)).context("deadline overflow")?;
    let result = tokio::time::timeout(Duration::from_secs(30), async {
        let reader = evidence::build(&config, store.clone()).await?;
        let service =
            FundingReconciliationService::new(store.clone(), store.clone(), reader, store.clone());
        service.apply(command, deadline).await.map_err(anyhow::Error::from)
    })
    .await
    .context("资金对账截止；如已开始提交必须按原command-id核对")??;
    println!("{}", serde_json::to_string_pretty(&result)?);
    anyhow::ensure!(result.account_status == "ready", "资金账实缺口：已冻结新增支出");
    Ok(())
}

#[cfg(feature = "postgres")]
fn parse_digest(value: &str) -> anyhow::Result<[u8; 32]> {
    anyhow::ensure!(
        value.len() == 64
            && value.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)),
        "preview digest must be canonical SHA-256 hex"
    );
    let mut digest = [0; 32];
    for (index, byte) in digest.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)?;
    }
    Ok(digest)
}

#[cfg(not(feature = "postgres"))]
pub(super) async fn run(_args: BotFundsArgs) -> anyhow::Result<()> {
    anyhow::bail!("bot-funds requires auctionctl postgres feature")
}
