//! 从固定的账本回执恢复端口结果，无需读取挂单的可变状态。

use auction_application::consignment::{
    ConsignmentError, ListingAutoCloseEffect, ListingBusinessRejection, ListingClosed,
    ListingCommandOutcome, ListingCommandRejection, ListingLookupEffect, ListingPriceDisposition,
    ListingPriceReceipt, ListingRegistered, LookupTransientReason, StoredListingDecision,
};
use auction_domain::consignment::{
    ListingMode, ListingPrice, ListingReason, ListingSnapshot, ListingVersion, UnknownListingReason,
};
use auction_time::Timestamp;
use serde::Deserialize;
use serde_json::Value;
use sqlx::PgConnection;
use uuid::Uuid;

use super::errors::{WriteError, invalid_projection};
use super::facts::Decision;
use super::ledger::{CommandWriter, StoredReceipt};

#[derive(Deserialize)]
struct Body<'a> {
    listing_id: Option<Uuid>,
    listing_version: Option<u64>,
    state: Option<&'a str>,
    price: Option<i64>,
    disposition: &'a str,
    expires_at: Option<Timestamp>,
    reason_code: Option<&'a str>,
    next_attempt_at: Option<Timestamp>,
    last_reason: Option<&'a str>,
}

impl<'a> Body<'a> {
    fn read(value: &'a Value) -> Result<Self, WriteError> {
        let Ok(body) = Self::deserialize(value) else {
            return Err(corrupt());
        };
        Ok(body)
    }

    fn listing_id(&self) -> Result<Uuid, WriteError> {
        self.listing_id.filter(|listing_id| !listing_id.is_nil()).ok_or_else(corrupt)
    }

    fn version(&self) -> Result<ListingVersion, WriteError> {
        ListingVersion::new(self.listing_version.ok_or_else(corrupt)?).map_err(invalid_projection)
    }

    fn price(&self) -> Result<ListingPrice, WriteError> {
        ListingPrice::new(self.price.ok_or_else(corrupt)?).map_err(invalid_projection)
    }

    async fn snapshot(&self, conn: &mut PgConnection) -> Result<ListingSnapshot, WriteError> {
        // 模式在数据库里不可变。其余字段只能来自原回执，不能从当前挂单补齐。
        let mode: String =
            sqlx::query_scalar("SELECT mode FROM public.consignment_listings WHERE id = $1")
                .bind(self.listing_id()?)
                .fetch_one(conn)
                .await?;
        Ok(ListingSnapshot {
            mode: mode.parse::<ListingMode>().map_err(invalid_projection)?,
            state: self.state.ok_or_else(corrupt)?.parse().map_err(invalid_projection)?,
            state_version: self.version()?,
            price: self.price()?,
            expires_at: self.expires_at.ok_or_else(corrupt)?,
            reason: self
                .reason_code
                .map(ListingReason::parse)
                .transpose()
                .map_err(|UnknownListingReason| corrupt())?,
        })
    }
}

fn corrupt() -> WriteError {
    WriteError::invariant("consignment_stored_command_receipt")
}

/// 已仲裁拒绝即使没有挂单也追加安全回执及审计。
pub(super) async fn reject<T>(
    writer: CommandWriter<'_, '_>,
    listing_id: Option<Uuid>,
    reason: ListingBusinessRejection,
    now: Timestamp,
) -> Result<ListingCommandOutcome<T>, WriteError> {
    writer.finish(Decision::Rejected { listing_id, reason }, now).await?;
    Ok(ListingCommandOutcome::Rejected(ListingCommandRejection::Recorded(reason)))
}

fn rejection(stored: &StoredReceipt) -> Result<ListingBusinessRejection, WriteError> {
    let reason = match stored.code.as_str() {
        "listing_not_found" => ListingBusinessRejection::ListingNotFound,
        "participant_ineligible" => ListingBusinessRejection::ParticipantIneligible,
        "already_open" => ListingBusinessRejection::AlreadyOpen,
        "equipment_busy" => ListingBusinessRejection::EquipmentBusy,
        "history_unproven" => ListingBusinessRejection::HistoryUnproven,
        "history_owner_mismatch" => ListingBusinessRejection::HistoryOwnerMismatch,
        "history_alias_conflict" => ListingBusinessRejection::HistoryAliasConflict,
        "fee_changed" => ListingBusinessRejection::FeeChanged,
        "stale_version" => ListingBusinessRejection::StaleVersion,
        "listing_not_active" => ListingBusinessRejection::ListingNotActive,
        "world_unavailable" => ListingBusinessRejection::WorldUnavailable,
        "registration_rate_limited" => ListingBusinessRejection::RegistrationRateLimited,
        "listing_limit_reached" => ListingBusinessRejection::ListingLimitReached,
        _ => return Err(corrupt()),
    };
    if stored.status != reason.status().cast_signed() {
        return Err(corrupt());
    }
    Ok(reason)
}

/// 复用登记、改价与主动关闭回执的公开成功或确定拒绝转换。
pub(super) fn replay<T>(
    stored: &StoredReceipt,
    decode: impl FnOnce(&StoredReceipt) -> Result<T, WriteError>,
) -> Result<ListingCommandOutcome<T>, WriteError> {
    let decision = if stored.status >= 400 {
        StoredListingDecision::Rejected(rejection(stored)?)
    } else {
        StoredListingDecision::Accepted(decode(stored)?)
    };
    Ok(ListingCommandOutcome::Replayed(decision))
}

/// 已仲裁的登记原回执。
pub(super) fn registered(stored: &StoredReceipt) -> Result<ListingRegistered, WriteError> {
    let body = Body::read(&stored.body)?;
    if stored.status != 202 || body.disposition != "registered" {
        return Err(corrupt());
    }
    ListingRegistered::new(body.listing_id()?, body.price()?).map_err(WriteError::from)
}

/// 已仲裁的改价原回执。
pub(super) fn repriced(stored: &StoredReceipt) -> Result<ListingPriceReceipt, WriteError> {
    let body = Body::read(&stored.body)?;
    let disposition = match body.disposition {
        "repriced" => ListingPriceDisposition::Repriced,
        "unchanged" => ListingPriceDisposition::Unchanged,
        _ => return Err(corrupt()),
    };
    ListingPriceReceipt::new(
        body.listing_id()?,
        body.version()?,
        body.state.ok_or_else(corrupt)?.parse().map_err(invalid_projection)?,
        body.price()?,
        disposition,
    )
    .map_err(WriteError::from)
}

/// 已仲裁的卖家取消或 Admin 下架原回执。
pub(super) fn closed(stored: &StoredReceipt) -> Result<ListingClosed, WriteError> {
    let body = Body::read(&stored.body)?;
    if body.disposition != "closed" {
        return Err(corrupt());
    }
    Ok(ListingClosed::new(
        body.listing_id()?,
        body.version()?,
        body.price.map(ListingPrice::new).transpose().map_err(invalid_projection)?,
    )?)
}

/// 已仲裁的解析观察原回执，重排分支只含安全分类与时刻。
pub(super) async fn lookup(
    conn: &mut PgConnection,
    stored: &StoredReceipt,
) -> Result<ListingCommandOutcome<ListingLookupEffect>, WriteError> {
    if stored.status >= 400 {
        return Ok(ListingCommandOutcome::Replayed(StoredListingDecision::Rejected(rejection(
            stored,
        )?)));
    }
    let body = Body::read(&stored.body)?;
    let effect = match body.disposition {
        "rescheduled" => ListingLookupEffect::Rescheduled {
            next_attempt_at: body.next_attempt_at.ok_or_else(corrupt)?,
            reason: match body.last_reason {
                Some("source_unavailable") => LookupTransientReason::SourceUnavailable,
                Some("source_timeout") => LookupTransientReason::SourceTimeout,
                Some("source_rate_limited") => LookupTransientReason::SourceRateLimited,
                _ => return Err(corrupt()),
            },
        },
        "published" | "rejected" | "closed" | "expired" => {
            ListingLookupEffect::ListingChanged(body.snapshot(conn).await?)
        }
        _ => return Err(ConsignmentError::Unavailable.into()),
    };
    Ok(ListingCommandOutcome::Replayed(StoredListingDecision::Accepted(effect)))
}

/// 已仲裁的自动清退原回执。
pub(super) async fn auto_closed(
    conn: &mut PgConnection,
    stored: &StoredReceipt,
) -> Result<ListingCommandOutcome<ListingAutoCloseEffect>, WriteError> {
    let body = Body::read(&stored.body)?;
    let effect = match body.disposition {
        "closed" | "expired" => ListingAutoCloseEffect::Terminated(body.snapshot(conn).await?),
        "conflicts_recorded" => ListingAutoCloseEffect::ConflictsRecorded,
        "unchanged" => ListingAutoCloseEffect::Unchanged,
        _ => return Err(corrupt()),
    };
    Ok(ListingCommandOutcome::Replayed(StoredListingDecision::Accepted(effect)))
}
