//! 单笔旧收款责任认领；只移动已真实隔离的现金责任，绝不重复发行旧收入。

mod preview;
mod record;
mod transaction;

use auction_application::{
    FundingError, FundingFuture, LegacyBotSellerCommand, LegacyBotSellerCommitOutcome,
    LegacyBotSellerPreview, LegacyBotSellerResult, LegacyBotSellerStore,
    SettlementInternalEvidenceSnapshot,
};
use uuid::Uuid;

impl LegacyBotSellerStore for crate::DatabaseStore {
    fn preview_legacy_bot_seller(
        &self,
        obligation_id: Uuid,
        expected_uid: String,
    ) -> FundingFuture<'_, LegacyBotSellerPreview> {
        Box::pin(async move {
            auction_domain::FundingAccountKey::new(&expected_uid).map_err(super::invalid)?;
            if obligation_id.is_nil() {
                return Err(FundingError::Invalid);
            }
            let mut tx = self.postgres_pool().begin().await.map_err(super::storage)?;
            sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
                .execute(&mut *tx)
                .await
                .map_err(super::storage)?;
            let target = preview::load(&mut tx, obligation_id, &expected_uid).await?;
            tx.commit().await.map_err(super::storage)?;
            Ok(target.preview)
        })
    }

    fn replay_legacy_bot_seller(
        &self,
        command: LegacyBotSellerCommand,
    ) -> FundingFuture<'_, Option<LegacyBotSellerResult>> {
        Box::pin(async move {
            command.validate()?;
            let mut tx = self.postgres_pool().begin().await.map_err(super::storage)?;
            sqlx::query("SET TRANSACTION READ ONLY")
                .execute(&mut *tx)
                .await
                .map_err(super::storage)?;
            let result = transaction::replay(&mut tx, &command).await?;
            tx.commit().await.map_err(super::storage)?;
            Ok(result)
        })
    }

    fn claim_legacy_bot_seller<'a>(
        &'a self,
        command: &'a LegacyBotSellerCommand,
        evidence: &'a SettlementInternalEvidenceSnapshot,
    ) -> FundingFuture<'a, LegacyBotSellerCommitOutcome> {
        Box::pin(transaction::run(self.postgres_pool(), command, evidence))
    }
}
