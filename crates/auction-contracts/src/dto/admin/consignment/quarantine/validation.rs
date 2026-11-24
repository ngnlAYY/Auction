//! 隔离观察、解除分支与回执的严格校验。

use uuid::Uuid;

use super::ConsignmentDelistReceiptV2;
use super::ConsignmentHoldReceipt;
use super::MAX_ACCEPT_ITEMS;
use super::MAX_OBSERVATIONS;
use super::QuarantineAction;
use super::QuarantineDetail;
use super::QuarantineObservation;
use super::QuarantineObservationItem;
use super::QuarantineObservationItemKind;
use super::QuarantineRecheckReceipt;
use super::QuarantineReleaseOutcome;
use super::QuarantineReleaseReceipt;
use super::QuarantineReleaseRecord;
use super::QuarantineState;
use crate::ConsignmentDispositionV2;
use crate::ConsignmentOperationV2;
use crate::ListingStampV2;
use crate::ListingStateV2;
use crate::dto::public::consignment::wire_v2::SAFE_INTEGER_MAX;
use crate::dto::public::consignment::wire_v2::is_equipment_id;

impl QuarantineObservationItem {
    pub(super) fn validate(&self) -> Result<(), &'static str> {
        if !(1..=MAX_OBSERVATIONS as i64).contains(&self.ordinal) {
            return Err("观察项序号必须为正且不超过协议上限");
        }
        match self.kind {
            QuarantineObservationItemKind::Equipment => {
                if !self.external_equipment_id.as_deref().is_some_and(is_equipment_id)
                    || self.amount.is_some()
                {
                    return Err("装备观察项必须带规范装备编号且不得带金额");
                }
            }
            QuarantineObservationItemKind::Credits | QuarantineObservationItemKind::Cod => {
                if self.external_equipment_id.is_some()
                    || self.amount.is_none()
                    || self.amount.is_some_and(|amount| !(0..=SAFE_INTEGER_MAX).contains(&amount))
                {
                    return Err("金额观察项必须只带非负安全整数金额");
                }
            }
            QuarantineObservationItemKind::Unknown => {
                if self.external_equipment_id.is_some() || self.amount.is_some() {
                    return Err("未知附件观察项不得带装备编号或金额");
                }
            }
        }
        Ok(())
    }
}
impl QuarantineObservation {
    pub(super) fn validate(&self) -> Result<(), &'static str> {
        if self.observation_id.is_nil() {
            return Err("观察标识不得为零 UUID");
        }
        if !(0..=MAX_OBSERVATIONS as i64).contains(&self.attachment_count)
            || usize::try_from(self.attachment_count).ok() != Some(self.items.len())
        {
            return Err("观察附件数量必须与脱敏清单条数一致");
        }
        for value in [self.credits, self.cod].into_iter().flatten() {
            if !(0..=SAFE_INTEGER_MAX).contains(&value) {
                return Err("观察 Credits 或 COD 必须为非负安全整数");
            }
        }
        if has_duplicate_ordinals(&self.items) {
            return Err("观察项的序号不得重复");
        }
        Ok(())
    }
}
impl QuarantineReleaseOutcome {
    pub(super) fn validate(&self) -> Result<(), &'static str> {
        match self {
            Self::Accept { listing_changes } => validate_accept_listings(listing_changes),
            Self::Return {} | Self::ArchiveEmpty {} => Ok(()),
            Self::AcquireReturn {
                return_intake_id,
                original_round,
                take_work_id,
                take_delivery_id,
            } => validate_acquire_return(
                *return_intake_id,
                *original_round,
                *take_work_id,
                *take_delivery_id,
            ),
            Self::AuctionReturnCase { return_mail_id, return_evidence_id, obligation_id } => {
                validate_auction_return_case(*return_mail_id, *return_evidence_id, *obligation_id)
            }
            Self::AuctionRecoveryReturn {
                obligation_id,
                return_mail_id,
                recovery_id,
                recovery_handoff_id,
            } => validate_auction_recovery_return(
                *obligation_id,
                *return_mail_id,
                *recovery_id,
                *recovery_handoff_id,
            ),
        }
    }
}
impl QuarantineReleaseRecord {
    pub(super) fn validate(&self) -> Result<(), &'static str> {
        if self.release_id.is_nil() || self.decision_id.is_nil() {
            return Err("解除记录标识不得为零 UUID");
        }
        Ok(())
    }
}
impl QuarantineDetail {
    pub(super) fn validate(&self) -> Result<(), &'static str> {
        if self.quarantine_id.is_nil() || self.first_decision_id.is_nil() {
            return Err("隔离标识不得为零 UUID");
        }
        if !(0..=SAFE_INTEGER_MAX).contains(&self.observation_total) {
            return Err("隔离观察总数必须为非负安全整数");
        }
        let returned =
            i64::try_from(self.observations.len()).map_err(|_| "隔离观察条数超出协议范围")?;
        if self.observation_total < returned {
            return Err("隔离观察总数不得小于本次返回条数");
        }
        if self.observations.is_empty() || self.observations.len() > MAX_OBSERVATIONS {
            return Err("隔离详情必须返回 1 到协议上限条观察");
        }
        if has_duplicate_observation_ids(&self.observations) {
            return Err("隔离观察标识不得重复");
        }
        if has_duplicates(&self.allowed_actions) {
            return Err("隔离允许动作不得重复");
        }
        match self.state {
            QuarantineState::Active => {
                if self.release.is_some() {
                    return Err("活动隔离不得携带最终解除记录");
                }
                if self.allowed_actions.iter().any(|action| {
                    !matches!(action, QuarantineAction::Recheck | QuarantineAction::Release)
                }) {
                    return Err("活动隔离只允许重查或解除");
                }
            }
            QuarantineState::Released => {
                let Some(release) = &self.release else {
                    return Err("已解除隔离必须携带最终解除记录");
                };
                if !self.allowed_actions.is_empty() {
                    return Err("已解除隔离不得再提供动作");
                }
                if release.decision_id == self.first_decision_id {
                    return Err("最终决定必须与首次隔离决定不同");
                }
            }
        }
        Ok(())
    }
}
impl QuarantineReleaseReceipt {
    pub(super) fn validate(&self) -> Result<(), &'static str> {
        let common = match self {
            Self::Accept {
                schema_version,
                operation,
                disposition,
                command_id,
                quarantine_id,
                release_id,
                decision_id,
                ..
            }
            | Self::Return {
                schema_version,
                operation,
                disposition,
                command_id,
                quarantine_id,
                release_id,
                decision_id,
            }
            | Self::ArchiveEmpty {
                schema_version,
                operation,
                disposition,
                command_id,
                quarantine_id,
                release_id,
                decision_id,
            }
            | Self::AcquireReturn {
                schema_version,
                operation,
                disposition,
                command_id,
                quarantine_id,
                release_id,
                decision_id,
                ..
            }
            | Self::AuctionReturnCase {
                schema_version,
                operation,
                disposition,
                command_id,
                quarantine_id,
                release_id,
                decision_id,
                ..
            }
            | Self::AuctionRecoveryReturn {
                schema_version,
                operation,
                disposition,
                command_id,
                quarantine_id,
                release_id,
                decision_id,
                ..
            } => (
                *schema_version,
                *operation,
                *disposition,
                *command_id,
                *quarantine_id,
                *release_id,
                *decision_id,
            ),
        };
        if common.0 != 2
            || common.1 != ConsignmentOperationV2::ReleaseQuarantine
            || common.2 != ConsignmentDispositionV2::QuarantineReleased
        {
            return Err("解除隔离回执的版本、operation 或处置不一致");
        }
        if common.3.is_nil() || common.4.is_nil() || common.5.is_nil() || common.6.is_nil() {
            return Err("解除隔离回执的标识不得为零 UUID");
        }
        match self {
            Self::Accept { listing_changes, .. } => validate_accept_listings(listing_changes),
            Self::Return { .. } | Self::ArchiveEmpty { .. } => Ok(()),
            Self::AcquireReturn {
                return_intake_id,
                original_round,
                take_work_id,
                take_delivery_id,
                ..
            } => validate_acquire_return(
                *return_intake_id,
                *original_round,
                *take_work_id,
                *take_delivery_id,
            ),
            Self::AuctionReturnCase {
                return_mail_id, return_evidence_id, obligation_id, ..
            } => validate_auction_return_case(*return_mail_id, *return_evidence_id, *obligation_id),
            Self::AuctionRecoveryReturn {
                obligation_id,
                return_mail_id,
                recovery_id,
                recovery_handoff_id,
                ..
            } => validate_auction_recovery_return(
                *obligation_id,
                *return_mail_id,
                *recovery_id,
                *recovery_handoff_id,
            ),
        }
    }
}
impl QuarantineRecheckReceipt {
    pub(super) fn validate(&self) -> Result<(), &'static str> {
        if self.operation != ConsignmentOperationV2::RecheckQuarantine
            || self.disposition != ConsignmentDispositionV2::RecheckQueued
            || self.schema_version != 2
        {
            return Err("隔离重查回执的版本、operation 或处置不一致");
        }
        if self.command_id.is_nil() || self.quarantine_id.is_nil() || self.work_id.is_nil() {
            return Err("隔离重查回执的标识不得为零 UUID");
        }
        Ok(())
    }
}
impl ConsignmentHoldReceipt {
    pub(super) fn validate(&self) -> Result<(), &'static str> {
        let valid = matches!(
            (self.operation, self.disposition, self.state),
            (
                ConsignmentOperationV2::AdminDelist,
                ConsignmentDispositionV2::HoldSet,
                ListingStateV2::HeldOffMarket,
            ) | (
                ConsignmentOperationV2::ReleaseHold,
                ConsignmentDispositionV2::HoldReleased,
                ListingStateV2::HeldOffMarket,
            )
        );
        if !valid || self.schema_version != 2 {
            return Err("止售回执的版本、operation、处置或状态不一致");
        }
        if self.command_id.is_nil() || self.listing_id.is_nil() || self.hold_id.is_nil() {
            return Err("止售回执的标识不得为零 UUID");
        }
        Ok(())
    }
}

impl ConsignmentDelistReceiptV2 {
    pub(super) fn validate(&self) -> Result<(), &'static str> {
        let (schema_version, operation, command_id, listing_id) = match self {
            Self::Closed { schema_version, operation, command_id, listing_id, .. }
            | Self::HoldSet { schema_version, operation, command_id, listing_id, .. } => {
                (*schema_version, *operation, *command_id, *listing_id)
            }
        };
        if schema_version != 2 || operation != ConsignmentOperationV2::AdminDelist {
            return Err("下架回执的版本或 operation 不一致");
        }
        if command_id.is_nil() || listing_id.is_nil() {
            return Err("下架回执的标识不得为零 UUID");
        }
        match self {
            Self::Closed { state, .. } if *state != ListingStateV2::Closed => {
                Err("未代管下架回执必须为 closed")
            }
            Self::HoldSet { state, hold_id, .. }
                if *state != ListingStateV2::HeldOffMarket || hold_id.is_nil() =>
            {
                Err("代管件下架回执必须为 held_off_market 且带非零 hold ID")
            }
            _ => Ok(()),
        }
    }
}
fn validate_accept_listings(listings: &[ListingStampV2]) -> Result<(), &'static str> {
    if listings.is_empty() || listings.len() > MAX_ACCEPT_ITEMS {
        return Err("整封接受必须给出 1 到协议上限条逐件 L");
    }
    if listings.iter().any(|stamp| stamp.state != ListingStateV2::DepositAccepted) {
        return Err("整封接受的逐件 L 必须为 deposit_accepted");
    }
    if listings.iter().enumerate().any(|(index, stamp)| {
        listings[..index].iter().any(|other| other.listing_id == stamp.listing_id)
    }) {
        return Err("整封接受的逐件 L 不得重复挂单");
    }
    Ok(())
}

fn validate_acquire_return(
    return_intake_id: Uuid,
    original_round: i64,
    take_work_id: Uuid,
    take_delivery_id: Uuid,
) -> Result<(), &'static str> {
    if return_intake_id.is_nil() || take_work_id.is_nil() || take_delivery_id.is_nil() {
        return Err("寄售退件接管引用不得为零 UUID");
    }
    if !(1..=SAFE_INTEGER_MAX).contains(&original_round) {
        return Err("寄售退件原轮次必须为正的安全整数");
    }
    Ok(())
}

fn validate_auction_return_case(
    return_mail_id: Uuid,
    return_evidence_id: Uuid,
    obligation_id: Option<Uuid>,
) -> Result<(), &'static str> {
    if return_mail_id.is_nil() || return_evidence_id.is_nil() {
        return Err("拍卖退件案件引用不得为零 UUID");
    }
    if obligation_id.is_some_and(|obligation_id| obligation_id.is_nil()) {
        return Err("拍卖退件义务引用不得为零 UUID");
    }
    Ok(())
}

fn validate_auction_recovery_return(
    obligation_id: Uuid,
    return_mail_id: Uuid,
    recovery_id: Uuid,
    recovery_handoff_id: Uuid,
) -> Result<(), &'static str> {
    if obligation_id.is_nil()
        || return_mail_id.is_nil()
        || recovery_id.is_nil()
        || recovery_handoff_id.is_nil()
    {
        return Err("拍卖恢复接管引用不得为零 UUID");
    }
    Ok(())
}

fn has_duplicates<T: PartialEq>(items: &[T]) -> bool {
    items.iter().enumerate().any(|(index, item)| items[..index].contains(item))
}

fn has_duplicate_ordinals(items: &[QuarantineObservationItem]) -> bool {
    items
        .iter()
        .enumerate()
        .any(|(index, item)| items[..index].iter().any(|other| other.ordinal == item.ordinal))
}

fn has_duplicate_observation_ids(items: &[QuarantineObservation]) -> bool {
    items.iter().enumerate().any(|(index, item)| {
        items[..index].iter().any(|other| other.observation_id == item.observation_id)
    })
}
