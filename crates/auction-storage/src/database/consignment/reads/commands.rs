//! 按原玩家身份读取、校验账本中的安全业务决定。

use auction_application::consignment::{
    ConsignmentError, ListingBusinessRejection, ListingClosed, ListingPriceDisposition,
    ListingPriceReceipt, ListingRegistered, OwnedListingCommandReceipt, OwnedListingDecision,
    StoredListingDecision,
};
use auction_domain::consignment::{ListingPrice, ListingState, ListingVersion};
use auction_domain::{CommandId, UserId};
use serde::Deserialize;
use sqlx::types::Json;
use uuid::Uuid;

use crate::database::DatabaseStore;
use crate::database::consignment::reads::{storage_error, unavailable};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    command_id: Uuid,
    operation: String,
    listing_id: Option<Uuid>,
    listing_version: Option<u64>,
    state: Option<String>,
    disposition: String,
    price: Option<i64>,
}

impl Receipt {
    fn decision(&self, code: &str) -> Result<OwnedListingDecision, ConsignmentError> {
        let decision = match (self.operation.as_str(), self.disposition.as_str()) {
            (_, "rejected") => return self.rejected(code),
            ("register", "registered")
                if self.listing_version == Some(1)
                    && self.state.as_deref() == Some("resolving") =>
            {
                OwnedListingDecision::Register(StoredListingDecision::Accepted(
                    ListingRegistered::new(self.target()?, self.required_price()?)?,
                ))
            }
            ("reprice", disposition @ ("repriced" | "unchanged")) => {
                let state: ListingState = self
                    .state
                    .as_deref()
                    .ok_or(ConsignmentError::Unavailable)?
                    .parse()
                    .map_err(unavailable)?;
                OwnedListingDecision::Reprice(StoredListingDecision::Accepted(
                    ListingPriceReceipt::new(
                        self.target()?,
                        self.version()?,
                        state,
                        self.required_price()?,
                        if disposition == "repriced" {
                            ListingPriceDisposition::Repriced
                        } else {
                            ListingPriceDisposition::Unchanged
                        },
                    )?,
                ))
            }
            ("withdraw", "closed") if self.state.as_deref() == Some("closed") => {
                OwnedListingDecision::Withdraw(StoredListingDecision::Accepted(ListingClosed::new(
                    self.target()?,
                    self.version()?,
                    self.price.map(ListingPrice::new).transpose().map_err(unavailable)?,
                )?))
            }
            _ => return Err(ConsignmentError::Unavailable),
        };
        Ok(decision)
    }

    fn rejected(&self, code: &str) -> Result<OwnedListingDecision, ConsignmentError> {
        // 端口拒绝变体只携带原因。底座的拒绝可以绑定目标，但不会附带版本、状态或标价。
        if self.listing_version.is_some() || self.state.is_some() || self.price.is_some() {
            return Err(ConsignmentError::Unavailable);
        }
        let reason = match (self.operation.as_str(), code) {
            ("register", "participant_ineligible") => {
                ListingBusinessRejection::ParticipantIneligible
            }
            ("register", "already_open") => ListingBusinessRejection::AlreadyOpen,
            ("register", "equipment_busy") => ListingBusinessRejection::EquipmentBusy,
            ("register", "history_unproven") => ListingBusinessRejection::HistoryUnproven,
            ("register", "history_owner_mismatch") => {
                ListingBusinessRejection::HistoryOwnerMismatch
            }
            ("register", "history_alias_conflict") => {
                ListingBusinessRejection::HistoryAliasConflict
            }
            ("register", "fee_changed") => ListingBusinessRejection::FeeChanged,
            ("register", "world_unavailable") => ListingBusinessRejection::WorldUnavailable,
            ("register", "registration_rate_limited") => {
                ListingBusinessRejection::RegistrationRateLimited
            }
            ("register", "listing_limit_reached") => ListingBusinessRejection::ListingLimitReached,
            ("reprice" | "withdraw", "listing_not_found") => {
                ListingBusinessRejection::ListingNotFound
            }
            ("reprice" | "withdraw", "stale_version") => ListingBusinessRejection::StaleVersion,
            ("reprice" | "withdraw", "listing_not_active") => {
                ListingBusinessRejection::ListingNotActive
            }
            _ => return Err(ConsignmentError::Unavailable),
        };
        match self.operation.as_str() {
            "register" => {
                Ok(OwnedListingDecision::Register(StoredListingDecision::Rejected(reason)))
            }
            "reprice" => Ok(OwnedListingDecision::Reprice(StoredListingDecision::Rejected(reason))),
            "withdraw" => {
                Ok(OwnedListingDecision::Withdraw(StoredListingDecision::Rejected(reason)))
            }
            _ => Err(ConsignmentError::Unavailable),
        }
    }

    fn target(&self) -> Result<Uuid, ConsignmentError> {
        self.listing_id.ok_or(ConsignmentError::Unavailable)
    }

    fn version(&self) -> Result<ListingVersion, ConsignmentError> {
        ListingVersion::new(self.listing_version.ok_or(ConsignmentError::Unavailable)?)
            .map_err(unavailable)
    }

    fn required_price(&self) -> Result<ListingPrice, ConsignmentError> {
        ListingPrice::new(self.price.ok_or(ConsignmentError::Unavailable)?).map_err(unavailable)
    }
}

pub(super) async fn owned_command(
    store: &DatabaseStore,
    seller: UserId,
    command_id: CommandId,
) -> Result<Option<OwnedListingCommandReceipt>, ConsignmentError> {
    // 不变量 1、7：以原 actor 而非当前挂单查询账本，内部命令在读取回执前排除。
    let found: Option<(i16, String, Json<Receipt>)> = sqlx::query_as(
        "SELECT result_status, result_code, result_body FROM public.consignment_commands
         WHERE command_id = $1 AND actor_kind = 'public_user' AND actor_ref = $2
           AND operation IN ('register', 'reprice', 'withdraw')",
    )
    .bind(command_id.as_uuid())
    .bind(seller.to_string())
    .fetch_optional(store.pool())
    .await
    .map_err(storage_error)?;
    let Some((status, code, Json(receipt))) = found else {
        return Ok(None);
    };
    let decision = receipt.decision(&code)?;
    if receipt.command_id != command_id.as_uuid()
        || decision.status_and_code() != (status.cast_unsigned(), code.as_str())
    {
        return Err(ConsignmentError::Unavailable);
    }
    Ok(Some(OwnedListingCommandReceipt { command_id, decision }))
}
