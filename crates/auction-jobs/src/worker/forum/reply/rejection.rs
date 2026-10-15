//! 论坛拒绝结果的安全英文投影；不改变竞价决定、幂等状态或重试分类。
//!
//! 新拒绝按类型持久化错误码，旧通用拒绝只接受已知完整诊断格式。未知码和任意详情均回退，
//! 不能把本地错误、SQL、凭据、代理上限或用户输入拼进固定 #2。

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
        "already_leading" => {
            ("already_leading", "Already leading this lot; another ordinary bid is not allowed.")
        }
        "bid_too_low" => (
            "bid_too_low",
            "Bid is below the required minimum; check the current price and minimum increment.",
        ),
        "invalid_amount" => (
            "invalid_amount",
            "Invalid bid amount; check the amount format, credit grid and allowed range.",
        ),
        "auction_closed" => {
            ("auction_closed", "Bidding deadline has passed; this bid was not accepted.")
        }
        "lot_not_found" => {
            ("lot_not_found", "Lot could not be found; check the auction and lot code.")
        }
        "bidder_ineligible" => (
            "bidder_ineligible",
            "Bidder is not eligible to bid; contact the auction administrator.",
        ),
        "state_changed" => (
            "state_changed",
            "Auction or lot state has changed; check the latest status before placing a new bid.",
        ),
        "invalid_state" => {
            ("invalid_state", "Current auction or lot state does not allow this command.")
        }
        "command_conflict" => (
            "command_conflict",
            "Command conflicts with an existing request; contact the auction administrator.",
        ),
        "capability_unavailable" => (
            "capability_unavailable",
            "This operation is not available; contact the auction administrator.",
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
            "New proxy maximum must be lower than the existing maximum.",
        ),
        "proxy_maximum_below_binding" => (
            "proxy_maximum_below_binding",
            "New proxy maximum cannot be below the amount already committed.",
        ),
        "proxy_reentry_maximum_not_higher" => (
            "proxy_reentry_maximum_not_higher",
            "Proxy re-entry requires a higher maximum than the previous authorization.",
        ),
        "proxy_disabled" => ("proxy_disabled", "Proxy bidding is disabled for this auction."),
        "proxy_outbid_limit" => ("proxy_outbid_limit", "Proxy re-entry limit has been reached."),
        "proxy_slow_final" => {
            ("proxy_slow_final", "A slow proxy cannot be started during the final phase.")
        }
        "request_hash_failed" => (
            "request_hash_failed",
            "Notification request could not be prepared; contact the auction administrator.",
        ),
        "owner_result_invalid" => (
            "owner_result_invalid",
            "Notification service returned an invalid result; check the bid result separately.",
        ),
        "notification_rejected" => (
            "notification_rejected",
            "Notification request was rejected; check the bid result separately.",
        ),
        _ => {
            ("command_rejected", "Command rejected; contact the auction administrator for details.")
        }
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
