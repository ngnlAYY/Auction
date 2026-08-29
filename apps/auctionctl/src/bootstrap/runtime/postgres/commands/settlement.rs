//! 旧版已完成结算账本与当前权威事实的只读核验命令。

use uuid::Uuid;

#[cfg(feature = "postgres")]
use super::super::*;
#[cfg(feature = "postgres")]
use auction_application::LegacySettlementVerificationPort;
#[cfg(feature = "postgres")]
use auction_storage::DatabaseSettlementCommands;

/// 只输出脱敏数量和稳定身份；命令 payload、lot、用户及义务正文不会进入运维输出。
#[cfg(feature = "postgres")]
pub(crate) async fn verify_legacy_settlement(auction_id: Uuid) -> anyhow::Result<()> {
    if auction_id.is_nil() {
        anyhow::bail!("auction-id 必须是非零 UUID");
    }

    let config = AppConfig::load(Role::Worker).context("load worker configuration")?;
    let database_url = config.database_url().context("database_url is required")?;
    if !matches!(DatabaseBackend::from_url(database_url)?, DatabaseBackend::Postgres) {
        anyhow::bail!("verify-legacy-settlement 只支持 PostgreSQL 完整档");
    }
    let store = Arc::new(
        DatabaseStore::connect(database_url)
            .await
            .context("connect database for legacy settlement verification")?,
    );
    let verification = DatabaseSettlementCommands::new(store)
        .verify_legacy_settlement(auction_id)
        .await
        .context("verify legacy settlement ledger against authoritative facts")?;
    let summary = serde_json::json!({
        "verified": true,
        "auction_id": verification.auction_id,
        "workflow_id": verification.workflow_id,
        "claim_command_id": verification.claim_command_id,
        "complete_command_id": verification.complete_command_id,
        "due_at": verification.due_at,
        "state_version": verification.state_version,
        "sold_count": verification.sold_count,
        "unsold_count": verification.unsold_count,
        "obligation_count": verification.obligation_count,
        "line_count": verification.line_count,
    });
    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok(())
}

#[cfg(not(feature = "postgres"))]
pub(crate) async fn verify_legacy_settlement(_auction_id: Uuid) -> anyhow::Result<()> {
    anyhow::bail!("verify-legacy-settlement 需要 PostgreSQL 完整档；请使用 --features postgres")
}
