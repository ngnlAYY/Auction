//! 结算与恢复页面：解析入口地址并挂载报表。

pub(in crate::surface::settlement) mod component;
#[cfg(test)]
mod tests;

use auction_web_shared::{AsyncState, AsyncStateKind, t};
use dioxus::prelude::*;

use super::query::{read_settlement_query, use_settlement_location_revision};
use component::AdminSettlementPage;

/// 加载并展示已结算拍卖的多条件报表页面。
pub(crate) fn page() -> Element {
    let location_revision = use_settlement_location_revision();
    let reload_generation = use_signal(|| 0_u64);
    // 同址通知也可能重绘页面，地址解析随共享位置版本更新。
    let query = use_memo(move || {
        let _ = location_revision();
        read_settlement_query()
    });
    match query() {
        Ok(initial_query) => rsx! {
            AdminSettlementPage { initial_query, location_revision, reload_generation }
        },
        Err(_invalid_link) => rsx! {
            section {
                class: "admin-auction-page admin-settlement-page",
                aria_labelledby: "settlement-title",
                h1 { id: "settlement-title", {t!("page-settlement-title")} }
                AsyncState {
                    kind: AsyncStateKind::Error,
                    title: t!("admin-settlement-invalid-link"),
                    detail: "",
                }
            }
        },
    }
}
