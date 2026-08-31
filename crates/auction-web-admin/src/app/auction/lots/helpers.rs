//! Admin lot 页面状态、编码和安全展示辅助。

use auction_web_shared::legacy_t as t;

use super::state::LotUpdateNotice;

pub(super) fn lot_status_label(status: &str) -> String {
    match status {
        "pending" => t!("admin-lots-status-pending"),
        "live" => t!("admin-lots-status-live"),
        "sold" => t!("admin-lots-status-sold"),
        "unsold" => t!("admin-lots-status-unsold"),
        "withdrawn" => t!("admin-lots-status-withdrawn"),
        _ => status.to_owned(),
    }
}

pub(super) fn lot_update_notice_label(notice: LotUpdateNotice) -> String {
    match notice {
        LotUpdateNotice::Repriced => t!("admin-lots-reprice-done"),
        LotUpdateNotice::Withdrawn => t!("admin-lots-withdraw-done"),
    }
}

/// 按旧版拍卖页面的约定生成稳定的拍品编码。
pub(in crate::app::surface) fn admin_lot_code_for(category: &str, lot_number: i32) -> String {
    let prefix = match category {
        "One-Handed" => "One",
        "Two-Handed" => "Two",
        "Staff" => "Sta",
        "Shield" => "Shd",
        "Cloth" => "Clo",
        "Light" => "Lig",
        "Heavy" => "Hea",
        _ => "Lot",
    };
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
    let fallback =
        || value.chars().rev().take(8).collect::<Vec<_>>().into_iter().rev().collect::<String>();
    let display = suffix
        .map(|suffix| suffix.trim_start_matches('0'))
        .filter(|suffix| !suffix.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(fallback);
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
