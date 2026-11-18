//! Admin lot 页面状态、编码和安全展示辅助。

use auction_contracts::EQUIPMENT_CATEGORY_CODES;
use auction_web_shared::t;

use super::state::LotUpdateNotice;

// 镜像 auction-domain 的 MAX_BID_AMOUNT，已存起拍价的读取边界独立保留。
pub(in crate::surface) const MAX_START_PRICE: i64 = 1_000_000_000;

/// 新起拍价必须允许玩家报出至少一口合法价格。
pub(in crate::surface) fn parse_start_price(value: &str) -> Option<i64> {
    value.trim().parse().ok().filter(|price| (1..=MAX_START_PRICE).contains(price))
}

pub(super) fn lot_update_notice_label(notice: LotUpdateNotice) -> String {
    match notice {
        LotUpdateNotice::Repriced => t!("admin-lots-reprice-done"),
        LotUpdateNotice::Withdrawn => t!("admin-lots-withdraw-done"),
    }
}

/// 按旧版拍卖页面的约定生成稳定的拍品编码：契约表前缀加至少两位补零序号，
/// 未知分类显示 `Lot` 前缀，与 Public 页面、邮件和论坛代码一致。
pub(in crate::surface) fn admin_lot_code_for(category: &str, lot_number: i32) -> String {
    let prefix = EQUIPMENT_CATEGORY_CODES
        .iter()
        .find(|(_, name, _)| *name == category)
        .map_or("Lot", |(_, _, prefix)| *prefix);
    format!("{prefix}{lot_number:02}")
}

/// 将提交编号压缩为旧版列表中的可读后缀，同时保留完整值供悬浮提示查看。
pub(super) fn submission_id_display(value: &str) -> String {
    // 旧版正则只在完整值末尾匹配 ASCII 数字；没有匹配时必须回退完整值。
    let suffix_start = value
        .char_indices()
        .rev()
        .take_while(|&(_, character)| character.is_ascii_digit())
        .last()
        .map(|(index, _)| index);
    let suffix = suffix_start.map(|index| &value[index..]);
    let fallback = || {
        let start = value.char_indices().rev().nth(7).map_or(0, |(index, _)| index);
        value[start..].to_owned()
    };
    let display = suffix
        .map(|suffix| suffix.trim_start_matches('0'))
        .filter(|suffix| !suffix.is_empty())
        .map_or_else(fallback, str::to_owned);
    format!("#{display}")
}

pub(super) fn proxy_mode_label(mode: &str) -> String {
    match mode {
        "fast" => t!("admin-lots-proxy-mode-fast"),
        "slow" => t!("admin-lots-proxy-mode-slow"),
        _ => mode.to_owned(),
    }
}

pub(super) fn proxy_status_label(status: &str) -> String {
    match status {
        "active" => t!("admin-lots-proxy-status-active"),
        "final_fast" => t!("admin-lots-proxy-status-final-fast"),
        "exhausted" => t!("admin-lots-proxy-status-exhausted"),
        "frozen" => t!("admin-lots-proxy-status-frozen"),
        "closed" => t!("admin-lots-proxy-status-closed"),
        _ => status.to_owned(),
    }
}

pub(super) fn equipment_details(name: &str, value: &serde_json::Value) -> String {
    let details = serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
    format!("{name}\n{details}")
}

#[cfg(test)]
mod tests {
    use super::{admin_lot_code_for, parse_start_price};

    #[test]
    fn lot_codes_match_forum_prefixes_and_fall_back_to_lot() {
        assert_eq!(admin_lot_code_for("Two-Handed", 7), "Two07");
        assert_eq!(admin_lot_code_for("Shield", 12), "Shd12");
        assert_eq!(admin_lot_code_for("Unknown", 1), "Lot01");
    }

    #[test]
    fn new_start_prices_stay_within_the_bid_range() {
        assert_eq!(parse_start_price("1"), Some(1));
        assert_eq!(parse_start_price("1000000000"), Some(1_000_000_000));
        for input in ["0", "1000000001", "-1", "1.5", "99999999999999999999"] {
            assert_eq!(parse_start_price(input), None, "{input}");
        }
    }
}
