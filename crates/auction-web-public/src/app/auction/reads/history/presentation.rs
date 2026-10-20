//! 成交结果的本页展示分组、全局排序控件及精确中位数格式。

use super::*;
use std::collections::BTreeMap;

/// 展示方式独立于后端查询；切换分组不会重新请求相同的成交页。
#[component]
pub(super) fn HistoryResultControls(
    sort: String,
    grouping: String,
    on_sort_change: EventHandler<String>,
    on_group_change: EventHandler<String>,
) -> Element {
    rsx! {
        div { class: "public-history-view-controls",
            label {
                span { {t!("history-sort-label")} }
                select { name: "history_sort", value: sort,
                    onchange: move |event| on_sort_change.call(event.value()),
                    option { value: "closed_desc", {t!("history-sort-closed-desc")} }
                    option { value: "closed_asc", {t!("history-sort-closed-asc")} }
                    option { value: "price_desc", {t!("history-sort-price-desc")} }
                    option { value: "price_asc", {t!("history-sort-price-asc")} }
                }
            }
            label {
                span { {t!("history-group-label")} }
                select { name: "history_group", value: grouping,
                    onchange: move |event| on_group_change.call(event.value()),
                    option { value: "flat", {t!("history-group-flat")} }
                    option { value: "equipment", {t!("history-group-equipment")} }
                    option { value: "buyer", {t!("history-group-buyer")} }
                    option { value: "seller", {t!("history-group-seller")} }
                }
            }
        }
        p { class: "public-history-results-scope", {t!("history-results-scope")} }
    }
}

/// 分组只持有当前页的真实行；顺序沿用各组在全局排序页中的首次出现位置。
pub(super) struct HistoryGroup {
    pub key: String,
    pub label: String,
    pub rows: Vec<PriceHistoryEntry>,
}

/// 按本页已有字段收集分组，不补查其他分页或推算全结果组数。
pub(super) fn history_groups(rows: Vec<PriceHistoryEntry>, grouping: &str) -> Vec<HistoryGroup> {
    let mut positions = BTreeMap::<String, usize>::new();
    let mut groups = Vec::<HistoryGroup>::new();
    for row in rows {
        let label = match grouping {
            "equipment" => row.equipment_name.clone(),
            "buyer" => row.buyer_name.clone(),
            "seller" => row.seller_name.clone(),
            _ => String::new(),
        };
        if let Some(index) = positions.get(&label) {
            groups[*index].rows.push(row);
        } else {
            positions.insert(label.clone(), groups.len());
            groups.push(HistoryGroup { key: row.lot_id.clone(), label, rows: vec![row] });
        }
    }
    groups
}

/// 半个 Credit 直接按精确字符串展示，不能把 .5 拼到缩写后的 K/M/B 金额上。
pub(super) fn history_median_amount(stats: &PriceHistoryStats) -> String {
    let Some(exact) = stats.median_exact.as_deref() else {
        return super::format_amount(stats.median);
    };
    if let Some(whole) = exact.strip_suffix(".5")
        && let Ok(whole) = whole.parse::<i64>()
    {
        let amount = format!("{}.5", auction_web_shared::format_credit_amount_full(whole));
        return t!("history-credit-amount", amount: amount);
    }
    exact
        .parse::<i64>()
        .map(super::format_amount)
        .unwrap_or_else(|_| super::format_amount(stats.median))
}
