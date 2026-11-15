//! 装备拒收的固定英文说明；仅映射拒收原因闭集，不回显远端详情或敏感装备链接。

use auction_application::mail::EquipmentRejectionReason;

/// 每个拒收原因的说明与后续步骤。match 没有通配分支：新增原因不写文案就编译失败；
/// 闭集外的结果码在调用方解析为原因时即失败关闭。
pub(crate) const fn equipment_rejection_copy(
    reason: EquipmentRejectionReason,
) -> (&'static str, &'static str) {
    match reason {
        EquipmentRejectionReason::Unverified => (
            "We could not verify the equipment details.",
            "Please contact the auction operator with the original mail reference before resubmitting.",
        ),
        EquipmentRejectionReason::StartPriceInvalid => (
            "The starting-price instructions could not be processed.",
            "After the equipment is returned, correct the starting-price instructions\n\
             and resend with the subject \"auction\".\n\
             To set one starting price for all attachments, use: start 50k",
        ),
        EquipmentRejectionReason::StartPriceMissing => (
            "A starting-price entry is missing for an attached item.",
            "After the equipment is returned, add the missing starting prices\n\
             and resend with the subject \"auction\".",
        ),
        EquipmentRejectionReason::StartPriceDuplicate => (
            "More than one starting-price entry was provided for the same item.",
            "After the equipment is returned, keep only one starting-price entry per item\n\
             and resend with the subject \"auction\".",
        ),
        EquipmentRejectionReason::StartPriceIdentityMismatch => (
            "An equipment link does not match its attachment.",
            "After the equipment is returned, copy the correct links from its details pages\n\
             and resend with the subject \"auction\".",
        ),
        EquipmentRejectionReason::StartPriceAttachmentNotFound => (
            "A starting-price entry refers to equipment that was not attached.",
            "After the equipment is returned, remove starting-price entries for unattached items\n\
             and resend with the subject \"auction\".",
        ),
        EquipmentRejectionReason::QualityNotAllowed => (
            "The equipment does not meet the current quality requirements.",
            "After the equipment is returned, remove the ineligible items\n\
             and resubmit the remaining equipment with the subject \"auction\".",
        ),
        EquipmentRejectionReason::PrefixNotAllowed => (
            "The equipment prefix is not accepted under the current submission rules.",
            "After the equipment is returned, remove items with unaccepted prefixes\n\
             and resubmit the remaining equipment with the subject \"auction\".",
        ),
        EquipmentRejectionReason::SuffixNotAllowed => (
            "The equipment suffix is not accepted under the current submission rules.",
            "After the equipment is returned, remove items with unaccepted suffixes\n\
             and resubmit the remaining equipment with the subject \"auction\".",
        ),
        EquipmentRejectionReason::CategoryNotAllowed => (
            "This equipment category is not currently accepted.",
            "After the equipment is returned, remove items in unaccepted categories\n\
             and resubmit the remaining equipment with the subject \"auction\".",
        ),
        EquipmentRejectionReason::EquipmentTypeNotAllowed => (
            "This equipment kind is not currently accepted.",
            "After the equipment is returned, remove items of unaccepted kinds\n\
             and resubmit the remaining equipment with the subject \"auction\".",
        ),
        EquipmentRejectionReason::LevelNotAllowed => (
            "The equipment does not meet the current level requirements.",
            "After the equipment is returned, remove items that do not meet the level requirements\n\
             and resubmit the remaining equipment with the subject \"auction\".",
        ),
        EquipmentRejectionReason::EquipmentConflict => (
            "This equipment is already registered in a previous auction and cannot be accepted again.",
            "The equipment will be returned. If this is a resale or the registration is wrong, \
             contact the auction operator with the original mail reference before resubmitting.",
        ),
        EquipmentRejectionReason::AttributesInvalid => (
            "The equipment attributes could not be verified.",
            "Please contact the auction operator with the original mail reference before resubmitting.",
        ),
        EquipmentRejectionReason::SeasonIntakeClosed => (
            "Submissions are closed until the new isekai season starts.",
            "The final auction before the season reset has already opened, and equipment from\n\
             the current season would be cleared by the reset.\n\
             After the new season starts, resend new-season equipment with the subject \"auction\".",
        ),
        EquipmentRejectionReason::TradeableRequired => (
            "Only tradable equipment can be submitted.",
            "After the equipment is returned, remove any non-tradable items\n\
             and resubmit the remaining equipment with the subject \"auction\".",
        ),
    }
}

#[cfg(test)]
mod tests {
    use auction_application::mail::EquipmentRejectionReason;

    use super::equipment_rejection_copy;

    #[test]
    fn duplicate_equipment_reason_has_explicit_copy() {
        let (reason, next) = equipment_rejection_copy(EquipmentRejectionReason::EquipmentConflict);
        assert!(reason.contains("already registered"));
        assert!(!next.is_empty());
        assert!(EquipmentRejectionReason::from_code("unknown_reason").is_none());
    }
}
