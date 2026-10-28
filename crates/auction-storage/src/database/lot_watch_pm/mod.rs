//! PM SQL 唯一 owner；持久领取、lot→queue fence、原 sender 对账与安全运维。
mod claims;
mod operations;
mod state;

use crate::database::DatabaseStore;
use auction_application::{
    ForumPmMessage, ForumPmReceipt, LotWatchError, LotWatchFuture, LotWatchPmClaim,
    LotWatchPmClaimRequest, LotWatchPmWorkStore, forum_pm_digest, valid_forum_pm_id,
};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

fn sql_error(error: sqlx::Error) -> LotWatchError {
    if crate::database::is_serialization_conflict(&error) {
        auction_application::CompetitionTransactionError::SerializationConflict.into()
    } else {
        LotWatchError::Unavailable
    }
}

impl LotWatchPmWorkStore for DatabaseStore {
    fn claim(
        &self,
        request: LotWatchPmClaimRequest,
    ) -> LotWatchFuture<'_, Option<LotWatchPmClaim>> {
        Box::pin(claims::claim(self, request))
    }
    fn start_sending(&self, claim: LotWatchPmClaim, sender: String) -> LotWatchFuture<'_, bool> {
        Box::pin(state::start(self, claim, sender))
    }
    fn release_unsent(&self, claim: LotWatchPmClaim) -> LotWatchFuture<'_, ()> {
        Box::pin(state::release(self, claim))
    }
    fn finish(
        &self,
        claim: LotWatchPmClaim,
        receipt: Option<ForumPmReceipt>,
    ) -> LotWatchFuture<'_, ()> {
        Box::pin(state::finish(self, claim, receipt))
    }
}

fn message(row: &sqlx::postgres::PgRow) -> Result<ForumPmMessage, LotWatchError> {
    let payload: serde_json::Value = row.try_get("payload").map_err(sql_error)?;
    let message: ForumPmMessage =
        serde_json::from_value(payload.clone()).map_err(|_| LotWatchError::Unavailable)?;
    let digest: Vec<u8> = row.try_get("request_digest").map_err(sql_error)?;
    let recipient: Option<String> = row.try_get("recipient").map_err(sql_error)?;
    let id: Uuid = row.try_get("notification_id").map_err(sql_error)?;
    let command_id: Uuid = row.try_get("command_id").map_err(sql_error)?;
    if serde_json::to_value(&message).map_err(|_| LotWatchError::Unavailable)? != payload
        || message.notification_id != id
        || command_id != id
        || recipient.as_deref() != Some(message.recipient_forum_account_id.as_str())
        || forum_pm_digest(&message)?.as_bytes().as_slice() != digest.as_slice()
    {
        return Err(LotWatchError::Unavailable);
    }
    Ok(message)
}
