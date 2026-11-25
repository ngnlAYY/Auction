//! 找回与挑战请求的频率预算（裁定 J11）。
//!
//! 每浏览器绑定与每可信客户端 IP 各有滚动窗口上限；超限的响应与目标是否真实无关。

use auction_time::Duration;

use super::errors::RightsError;

/// 预算窗口。
pub const BUDGET_WINDOW: Duration = Duration::minutes(10);
/// 每浏览器绑定每窗口最多的找回与挑战请求数。
pub const BROWSER_BUDGET_LIMIT: u32 = 10;
/// 每可信客户端 IP 每窗口最多的找回与挑战请求数。
pub const IP_BUDGET_LIMIT: u32 = 30;

/// 窗口内已用次数是否仍低于上限。
pub const fn budget_allows(used_in_window: u32, limit: u32) -> bool {
    used_in_window < limit
}

/// 浏览器与 IP 两道预算都未用尽才放行。
pub fn request_within_budget(browser_used: u32, ip_used: u32) -> Result<(), RightsError> {
    if budget_allows(browser_used, BROWSER_BUDGET_LIMIT) && budget_allows(ip_used, IP_BUDGET_LIMIT)
    {
        return Ok(());
    }
    Err(RightsError::BudgetExceeded)
}

#[cfg(test)]
mod tests {
    use super::{BROWSER_BUDGET_LIMIT, IP_BUDGET_LIMIT, request_within_budget};
    use crate::consignment::rights::RightsError;

    #[test]
    fn budgets_are_ten_per_browser_and_thirty_per_ip() {
        assert_eq!((BROWSER_BUDGET_LIMIT, IP_BUDGET_LIMIT), (10, 30));
        assert_eq!(request_within_budget(9, 29), Ok(()));
        assert_eq!(request_within_budget(10, 0), Err(RightsError::BudgetExceeded));
        assert_eq!(request_within_budget(0, 30), Err(RightsError::BudgetExceeded));
    }
}
