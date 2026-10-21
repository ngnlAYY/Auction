//! 装备拒收的固定英文说明；仅映射稳定原因码，不回显远端详情或敏感装备链接。

/// 为通知校验和正文渲染提供同一原因闭集，未知原因继续失败关闭。
pub(crate) fn equipment_rejection_copy(code: &str) -> Option<(&'static str, &'static str)> {
    let copy = match code {
        "auction_equipment_rejected" => (
            "We could not verify the equipment details.",
            "Please contact the auction operator with the original mail reference before resubmitting.",
        ),
        "start_price_invalid" => (
            "The starting-price instructions could not be processed.",
            "After the equipment is returned, correct the starting-price instructions\n\
             and resend with the subject \"auction\".\n\
             To set one starting price for all attachments, use: start 50k",
        ),
        "start_price_missing" => (
            "A starting-price entry is missing for an attached item.",
            "After the equipment is returned, add the missing starting prices\n\
             and resend with the subject \"auction\".",
        ),
        "start_price_duplicate" => (
            "More than one starting-price entry was provided for the same item.",
            "After the equipment is returned, keep only one starting-price entry per item\n\
             and resend with the subject \"auction\".",
        ),
        "start_price_identity_mismatch" => (
            "An equipment link does not match its attachment.",
            "After the equipment is returned, copy the correct links from its details pages\n\
             and resend with the subject \"auction\".",
        ),
        "start_price_attachment_not_found" => (
            "A starting-price entry refers to equipment that was not attached.",
            "After the equipment is returned, remove starting-price entries for unattached items\n\
             and resend with the subject \"auction\".",
        ),
        "quality_not_allowed" => (
            "The equipment does not meet the current quality requirements.",
            "After the equipment is returned, remove the ineligible items\n\
             and resubmit the remaining equipment with the subject \"auction\".",
        ),
        "prefix_not_allowed" => (
            "The equipment prefix is not accepted under the current submission rules.",
            "After the equipment is returned, remove items with unaccepted prefixes\n\
             and resubmit the remaining equipment with the subject \"auction\".",
        ),
        "suffix_not_allowed" => (
            "The equipment suffix is not accepted under the current submission rules.",
            "After the equipment is returned, remove items with unaccepted suffixes\n\
             and resubmit the remaining equipment with the subject \"auction\".",
        ),
        "category_not_allowed" => (
            "This equipment category is not currently accepted.",
            "After the equipment is returned, remove items in unaccepted categories\n\
             and resubmit the remaining equipment with the subject \"auction\".",
        ),
        "equipment_type_not_allowed" => (
            "This equipment kind is not currently accepted.",
            "After the equipment is returned, remove items of unaccepted kinds\n\
             and resubmit the remaining equipment with the subject \"auction\".",
        ),
        "level_not_allowed" => (
            "The equipment does not meet the current level requirements.",
            "After the equipment is returned, remove items that do not meet the level requirements\n\
             and resubmit the remaining equipment with the subject \"auction\".",
        ),
        "attributes_invalid" => (
            "The equipment attributes could not be verified.",
            "Please contact the auction operator with the original mail reference before resubmitting.",
        ),
        "tradeable_required" => (
            "Only tradable equipment can be submitted.",
            "After the equipment is returned, remove any non-tradable items\n\
             and resubmit the remaining equipment with the subject \"auction\".",
        ),
        _ => return None,
    };
    Some(copy)
}
