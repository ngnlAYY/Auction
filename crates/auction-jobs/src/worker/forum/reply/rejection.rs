//! 论坛拒绝与警告的安全英文投影；不改变竞价决定、幂等状态或重试分类，
//! 未知码与任意详情均回退，不拼入本地错误、SQL、凭据或用户输入。

use auction_application::{ApplicationError, CommandLedgerError, CompetitionTransactionError};
use auction_domain::DomainError;

/// 从已确定不可重试的应用错误提取展示分类；调用方仍按原策略决定是否重试。
pub(super) fn rejection_code(error: &ApplicationError) -> &'static str {
    match error {
        ApplicationError::Domain(error) => match error {
            DomainError::AlreadyLeading => "already_leading",
            DomainError::BidTooLow { .. } => "bid_too_low",
            DomainError::InvalidAmount { .. } => "invalid_amount",
            DomainError::AuctionClosed => "auction_closed",
            DomainError::InvalidBidState(_) => "invalid_state",
            DomainError::ActiveProxyExists => "active_proxy_exists",
            DomainError::InvalidProxyMaximum => "invalid_proxy_maximum",
            DomainError::ProxyAuthorizationNotActive => "proxy_not_active",
            DomainError::ProxyMaximumNotLower => "proxy_maximum_not_lower",
            DomainError::ProxyMaximumBelowBinding => "proxy_maximum_below_binding",
            DomainError::ProxyReentryMaximumNotHigher => "proxy_reentry_maximum_not_higher",
            DomainError::ProxyDisabled => "proxy_disabled",
            DomainError::ProxyOutbidLimit => "proxy_outbid_limit",
            DomainError::ProxySlowFinal => "proxy_slow_final",
            _ => "command_rejected",
        },
        ApplicationError::CompetitionTransaction(error) => match error {
            CompetitionTransactionError::NotFound => "lot_not_found",
            CompetitionTransactionError::BidderIneligible => "bidder_ineligible",
            CompetitionTransactionError::StaleState => "state_changed",
            CompetitionTransactionError::InvalidState => "invalid_state",
            CompetitionTransactionError::CommandLedger(CommandLedgerError::Conflict) => {
                "command_conflict"
            }
            _ => "command_rejected",
        },
        ApplicationError::CommandLedger(CommandLedgerError::Conflict) => "command_conflict",
        ApplicationError::CapabilityUnavailable(_) => "capability_unavailable",
        _ => "command_rejected",
    }
}

/// 返回闭集错误码及英文提示；仅解释既有拒绝，不补写账本或重新执行出价。
pub(super) fn rejection_message(
    error_code: Option<&str>,
    safe_detail: Option<&str>,
) -> (&'static str, &'static str) {
    let code = match error_code {
        // 只为旧通用分类兼容历史诊断；明确的新分类优先，未知新码不能借详情伪装成另一原因。
        None | Some("command_rejected") => legacy_rejection_code(safe_detail),
        Some(code) => code,
    };
    match code {
        "already_leading" => (
            "already_leading",
            "Bidder was already leading this lot when processed; this additional ordinary bid was not accepted. Check the latest leader before bidding again.",
        ),
        "bid_too_low" => (
            "bid_too_low",
            "Bid was below the required minimum when processed. Check the latest price and minimum increment, then submit a new reply with a valid bid; editing this reply will not retry it.",
        ),
        "invalid_amount" => (
            "invalid_amount",
            "Invalid bid amount. Use a positive multiple of 1,000 credits within the auction limits, then submit a new reply; editing this reply will not retry it.",
        ),
        "auction_closed" => (
            "auction_closed",
            "The effective bidding deadline had passed when processed; this bid was not accepted. Check the final auction status; reposting cannot reopen bidding.",
        ),
        "lot_not_found" => (
            "lot_not_found",
            "Auction lot could not be found. Check the latest catalog and auction status before submitting a new reply with the correct lot code.",
        ),
        "bidder_ineligible" => (
            "bidder_ineligible",
            "Bidder was not eligible when processed; this command was not accepted. Contact the auction administrator before bidding again.",
        ),
        "state_changed" => (
            "state_changed",
            "Auction or lot state changed; this command was not accepted. Check the latest status before submitting a new reply.",
        ),
        "invalid_state" => (
            "invalid_state",
            "This command could not be applied in the current auction or lot state. Check the latest status; contact the auction administrator if bidding should be available.",
        ),
        "command_conflict" => (
            "command_conflict",
            "This command conflicts with an earlier request. Check the recorded result and contact the auction administrator with the post and line number; do not repost blindly.",
        ),
        "capability_unavailable" => (
            "capability_unavailable",
            "This operation is unavailable. Contact the auction administrator with the post and line number before trying again.",
        ),
        "active_proxy_exists" => (
            "active_proxy_exists",
            "An active proxy already exists for this bidder and lot; check it on the auction website.",
        ),
        "invalid_proxy_maximum" => (
            "invalid_proxy_maximum",
            "An active proxy maximum may only be lowered; check it on the auction website.",
        ),
        "proxy_not_active" => (
            "proxy_not_active",
            "Proxy authorization is no longer active; check its status on the auction website.",
        ),
        "proxy_maximum_not_lower" => (
            "proxy_maximum_not_lower",
            "New proxy maximum must be lower than the existing maximum; manage proxy settings on the auction website.",
        ),
        "proxy_maximum_below_binding" => (
            "proxy_maximum_below_binding",
            "New proxy maximum cannot be below the amount already committed; review proxy settings on the auction website.",
        ),
        "proxy_reentry_maximum_not_higher" => (
            "proxy_reentry_maximum_not_higher",
            "Proxy re-entry requires a higher maximum than the previous authorization; review proxy settings on the auction website.",
        ),
        "proxy_disabled" => (
            "proxy_disabled",
            "Proxy bidding is disabled for this auction; check the auction rules on the website.",
        ),
        "proxy_outbid_limit" => (
            "proxy_outbid_limit",
            "Proxy re-entry limit has been reached; check your proxy status on the auction website.",
        ),
        "proxy_slow_final" => (
            "proxy_slow_final",
            "A slow proxy cannot be started during the final phase; check the auction status on the website.",
        ),
        "request_hash_failed" => (
            "request_hash_failed",
            "Notification request could not be prepared. Check the bid result separately; contact the auction administrator with the post and line number. Do not repeat a bid to request a notification.",
        ),
        "owner_result_invalid" => (
            "owner_result_invalid",
            "Notification result could not be confirmed. Check the bid result separately; contact the auction administrator with the post and line number. Do not repeat a bid to request a notification.",
        ),
        "notification_rejected" => (
            "notification_rejected",
            "Notification request was rejected. Check the bid result separately; contact the auction administrator with the post and line number. Do not repeat a bid to request a notification.",
        ),
        _ => (
            "command_rejected",
            "Command rejected. Check the recorded auction result and contact the auction administrator with the post and line number before trying again.",
        ),
    }
}

/// 返回解析器与主题处理警告的闭集英文投影；未知码和原始详情均不得回显。
/// pending 只表示处理未完成，不证明出价已接受或未接受；通知重试前可能已有成功出价。
/// 代理行仍由解析器静默忽略，本函数不产生新警告，也不改变命令或重试策略。
pub(super) fn warning_message(code: &str) -> (&'static str, &'static str) {
    match code {
        "edited_post_ignored" => (
            "edited_post_ignored",
            "Edited post ignored; edits cannot create or correct commands and do not undo earlier accepted bids. Submit corrections in a new reply; check any earlier result first to avoid duplicates.",
        ),
        "invalid_command" => (
            "invalid_command",
            "Line not applied: start with a catalog lot code followed by a bid amount, for example One01 100k. Use the actual lot code and a valid bid in a new reply; editing this reply will not retry it.",
        ),
        "missing_action" => (
            "missing_action",
            "Line not applied: lot code needs a bid amount or notify command. Submit the complete line in a new reply; editing this reply will not retry it.",
        ),
        "unknown_lot" => (
            "unknown_lot",
            "Line not applied: at least one lot code is absent from the catalog. Check every code on this line against the latest catalog, then correct it in a new reply; editing this reply will not retry it.",
        ),
        "invalid_action" => (
            "invalid_action",
            "Line not applied: unrecognized action or invalid amount. For a bid, use a positive multiple of 1,000 credits, such as 100k or 1m, within auction limits. Submit corrections in a new reply; editing this reply will not retry it.",
        ),
        "extra_words" => (
            "extra_words",
            "Line not applied: unexpected text after the command. Use lot codes followed by a bid amount and optional notify, notify pm or notify mm, without commentary. Submit corrections in a new reply; editing this reply will not retry it.",
        ),
        "invalid_notify" => (
            "invalid_notify",
            "Entire line not applied, including any bid: notify allows only no suffix, pm or mm. Remove other text and submit the corrected line in a new reply; editing this reply will not retry it. Forum PM delivery is unavailable.",
        ),
        "invalid_catalog_row" => (
            "invalid_catalog_row",
            "Catalog row ignored because its lot code is invalid. Ask the auction administrator to check the catalog; do not bid on this row until corrected.",
        ),
        "invalid_start_price" => (
            "invalid_start_price",
            "Catalog custom start price could not be parsed; the parser used its default. Ask the auction administrator to confirm the authoritative start price before bidding.",
        ),
        "identity_missing" => (
            "identity_missing",
            "Commands in this post were skipped because the author's forum account could not be identified. Contact the auction administrator with the post number before submitting another bid.",
        ),
        "identity_unavailable" => (
            "identity_unavailable",
            "Commands in this post were skipped because the forum/MM player name could not be linked to an eligible auction user. Contact the auction administrator to resolve the account link before bidding again.",
        ),
        "identity_processing_pending" => (
            "identity_processing_pending",
            "Identity processing is pending, not confirmation of bid acceptance. Processing stopped here and will retry; later commands are waiting. Do not edit or repost while waiting, to avoid duplicate requests.",
        ),
        "admin_command_not_supported" => (
            "admin_command_not_supported",
            "start and cancel were not applied: these are Admin-only operations, not forum commands. Contact the auction administrator; a forum cancel cannot undo an accepted bid.",
        ),
        "forum_pm_unavailable" => (
            "forum_pm_unavailable",
            "Forum PM notifications are unavailable. MM notification processing is separate; check the bid result independently. Do not repeat a bid to request a notification.",
        ),
        "mm_notification_unavailable" => (
            "mm_notification_unavailable",
            "MoogleMail notifications are unavailable. Check the bid result separately and contact the auction administrator about notifications; do not repeat a bid to request one.",
        ),
        "lot_mapping_missing" => (
            "lot_mapping_missing",
            "Command for this lot was skipped because the catalog code could not be matched to an auction lot. Ask the auction administrator to check the catalog; other lots on this line may have separate results.",
        ),
        "command_processing_pending" => (
            "command_processing_pending",
            "Command processing is pending, not confirmation of bid acceptance. Processing stopped here and will retry; later commands are waiting. A bid may already have succeeded before notification processing stopped. Check results; do not edit or repost while waiting.",
        ),
        "command_pending" => (
            "command_pending",
            "Pending is not confirmation of acceptance; processing will retry. Check the bid result separately if this is a notification request. Do not edit or repost while waiting, to avoid duplicate requests.",
        ),
        _ => (
            "forum_warning",
            "This post needs review. Check the recorded auction result and contact the auction administrator with the post and line number before trying again.",
        ),
    }
}

fn legacy_rejection_code(detail: Option<&str>) -> &'static str {
    let Some(detail) = detail else { return "command_rejected" };
    match detail {
        "bidder is already leading the lot" => "already_leading",
        "auction is past its effective deadline" => "auction_closed",
        "bidder already owns the active proxy authorization" => "active_proxy_exists",
        "proxy maximum must be lowered while it is active" => "invalid_proxy_maximum",
        "proxy authorization is not active" => "proxy_not_active",
        "proxy maximum was not strictly lowered" => "proxy_maximum_not_lower",
        "proxy maximum is below the binding amount" => "proxy_maximum_below_binding",
        "proxy re-entry maximum was not raised" => "proxy_reentry_maximum_not_higher",
        "proxy bidding is disabled for this auction" => "proxy_disabled",
        "proxy re-entry quota has been exhausted" => "proxy_outbid_limit",
        "slow proxy is not allowed in the final phase" => "proxy_slow_final",
        "竞价投影不存在" => "lot_not_found",
        "竞价参与用户当前没有资格" => "bidder_ineligible",
        "管理 lot 的状态版本已经过期" => "state_changed",
        "管理 lot 当前状态不允许该命令" => "invalid_state",
        "命令 ID 与已有请求负载冲突" => "command_conflict",
        // 旧金额诊断只接受完整的规范 i64 格式。金额仅用于识别类型，绝不回显原文或数值。
        _ if legacy_amount(detail, "bid amount is below the minimum of ")
            .is_some_and(|minimum| minimum > 0) =>
        {
            "bid_too_low"
        }
        _ if legacy_amount(detail, "invalid amount: ").is_some() => "invalid_amount",
        _ => "command_rejected",
    }
}

fn legacy_amount(detail: &str, prefix: &str) -> Option<i64> {
    let value = detail.strip_prefix(prefix)?;
    let amount = value.parse::<i64>().ok()?;
    (amount.to_string() == value).then_some(amount)
}
