//! 旧结算单笔预览与证据修复装配；默认不取得凭据 owner，也不访问远端。

mod args;
#[cfg(feature = "postgres")]
mod collected;
#[cfg(feature = "postgres")]
mod output;
#[cfg(feature = "postgres")]
mod self_send;

pub(crate) use args::LegacySettlementAction;

#[cfg(feature = "postgres")]
pub(super) async fn run(action: LegacySettlementAction) -> anyhow::Result<()> {
    match action {
        LegacySettlementAction::SelfSend { args, reason } => self_send::run(args, reason).await,
        LegacySettlementAction::AdoptCollected { args } => collected::run(args).await,
    }
}

#[cfg(not(feature = "postgres"))]
pub(super) async fn run(_action: LegacySettlementAction) -> anyhow::Result<()> {
    anyhow::bail!("legacy-settlement 要求 auctionctl postgres feature")
}
