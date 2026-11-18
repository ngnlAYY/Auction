//! Admin lot 表格和页面级分页编排。

use std::cmp::Ordering;

use auction_web_shared::t;
use auction_web_shared::{AsyncState, AsyncStateKind};
use dioxus::prelude::*;

use super::super::intake::AdminIntakeForm;
use super::super::router::Route;
use super::super::sort::{MobileSortControl, SortDirection, SortableTh, sort_rows};
use super::edit::LotEdit;
use super::presentation::lot_update_notice_label;
use super::proxy::AdminProxyHistory;
use super::row::AdminLotRow;
use super::state::LotUpdateNotice;
use crate::api::{AdminLot, AdminLotPage};
use crate::surface::pagination::{AdminPager as Pager, AdminPagination};

#[component]
pub(super) fn AdminLotTable(
    page: AdminLotPage,
    pagination: AdminPagination,
    edits_ready: bool,
    /// 当前排序列（空串 = 服务端原始顺序）与方向；变更经 `on_sort_change` 由页面写回 URL。
    sort: String,
    direction: SortDirection,
    /// 翻页回调：由页面把新页码 push 到地址栏并更新查询状态。
    on_page_change: EventHandler<i64>,
    on_sort_change: EventHandler<(String, SortDirection)>,
    reload_generation: Signal<u64>,
    draft: Signal<Option<LotEdit>>,
    notice: Signal<Option<LotUpdateNotice>>,
) -> Element {
    let selected_lot = use_signal(|| None::<(String, String)>);
    let allow_lot_update = page.auction.status == "draft";
    let is_empty = page.lots.is_empty();
    let mut lots = page.lots;
    sort_lots(&mut lots, &sort, direction);

    rsx! {
        section { class: "admin-auction-page", aria_labelledby: "auction-title",
            h1 { id: "auction-title",
                {t!("page-auction-lots-title", auction: page.auction.sequence)}
            }
            p { class: "admin-auction-intro",
                Link { to: Route::Auctions {}, {t!("admin-lots-back")} }
            }
            if let Some(notice) = notice() {
                p { class: "action-feedback action-feedback--success", role: "status",
                    {lot_update_notice_label(notice)}
                }
            }
            section {
                if matches!(page.auction.status.as_str(), "draft" | "preview")
                    && page.capabilities.manual_intake
                {
                    for world_id in [page.auction.world_id] {
                        AdminIntakeForm {
                            key: "{world_id}",
                            world_id,
                        }
                    }
                }
                AdminLotMobileSort {
                    sort: sort.clone(),
                    direction,
                    on_sort_change,
                }
                div { class: "admin-auction-table-wrap admin-auction-lot-table-wrap",
                    table { class: "admin-auction-table admin-auction-lot-table",
                        aria_label: t!("page-auction-lots-title", auction: page.auction.sequence),
                        auction_web_shared::TableColumns { layout: "admin-lots", widths: vec![7, 28, 10, 7, 9, 9, 12, 18] }
                        thead {
                            tr {
                                SortableTh {
                                    column: "code",
                                    label: t!("admin-lots-code"),
                                    active: sort.clone(),
                                    direction,
                                    on_sort: on_sort_change,
                                    omit_inactive_aria_sort: true,
                                }
                                th { scope: "col", {t!("admin-lots-equipment")} }
                                th { scope: "col", {t!("admin-lots-seller")} }
                                th { scope: "col", {t!("admin-lots-submission")} }
                                SortableTh {
                                    column: "status",
                                    label: t!("admin-lots-status"),
                                    active: sort.clone(),
                                    direction,
                                    on_sort: on_sort_change,
                                    omit_inactive_aria_sort: true,
                                }
                                SortableTh {
                                    column: "start_price",
                                    label: t!("admin-lots-start-price"),
                                    active: sort.clone(),
                                    direction,
                                    on_sort: on_sort_change,
                                    omit_inactive_aria_sort: true,
                                    class: "admin-auction-lot-start",
                                }
                                SortableTh {
                                    column: "current_price",
                                    label: t!("admin-lots-price"),
                                    active: sort,
                                    direction,
                                    on_sort: on_sort_change,
                                    omit_inactive_aria_sort: true,
                                    class: "admin-auction-lot-current",
                                }
                                th { scope: "col", class: "admin-auction-lot-actions", {t!("admin-lots-actions")} }
                            }
                        }
                        tbody {
                            for lot in lots {
                                // 按业务 ID 固定 key，避免刷新/重排后行内本地状态错位绑定到另一行。
                                AdminLotRow { key: "{lot.id}", lot, selected_lot, notice, allow_lot_update, edits_ready, draft }
                            }
                        }
                    }
                }
                if is_empty {
                    AsyncState {
                        kind: AsyncStateKind::Empty,
                        title: t!("admin-lots-empty"),
                        detail: "",
                    }
                }
                Pager {
                    pagination,
                    page: page.pagination.page,
                    page_size: page.pagination.page_size,
                    total: Some(page.pagination.total),
                    label: t!("admin-lots-pagination"),
                    on_page_change,
                }
            }
            for (lot_id, lot_code) in selected_lot() {
                AdminProxyHistory { key: "{lot_id}", lot_id, lot_code, selected_lot, reload_generation }
            }
        }
    }
}

/// 对当前页 lot 行排序；`column` 为空或不在允许集合时保持服务端原始顺序。
fn sort_lots(lots: &mut [AdminLot], column: &str, direction: SortDirection) {
    sort_rows(lots, column, direction, |left, right, column| match column {
        "code" => left.lot_number.cmp(&right.lot_number),
        "status" => left.status.cmp(&right.status),
        "start_price" => left.start_price.cmp(&right.start_price),
        "current_price" => left.current_price.cmp(&right.current_price),
        _ => Ordering::Equal,
    });
}

#[component]
fn AdminLotMobileSort(
    sort: String,
    direction: SortDirection,
    on_sort_change: EventHandler<(String, SortDirection)>,
) -> Element {
    rsx! {
        MobileSortControl {
            columns: vec![
                ("code", t!("admin-lots-code")),
                ("status", t!("admin-lots-status")),
                ("start_price", t!("admin-lots-start-price")),
                ("current_price", t!("admin-lots-price")),
            ],
            sort,
            direction,
            on_sort_change,
            default_order_label: t!("admin-sort-default-order"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use auction_web_shared::i18n::{DEFAULT_ADMIN_LOCALE, build_bundle};
    use dioxus::dioxus_core::{AttributeValue, Mutation};
    use dioxus::prelude::*;
    use dioxus_i18n::prelude::try_use_init_i18n_from_bundle;

    use super::AdminLotMobileSort;
    use crate::surface::ADMIN_LOCALE_CATALOG;
    use crate::surface::sort::{SortDirection, SortableTh};

    fn sort_control(sort: String) -> Element {
        try_use_init_i18n_from_bundle(|| {
            build_bundle(
                &DEFAULT_ADMIN_LOCALE,
                ADMIN_LOCALE_CATALOG.default_ftl,
                &DEFAULT_ADMIN_LOCALE,
                None,
            )
        })
        .unwrap();
        rsx! {
            AdminLotMobileSort {
                sort: sort.clone(),
                direction: SortDirection::Desc,
                on_sort_change: |_| {},
            }
            table {
                thead { tr {
                    for column in ["code", "current_price"] {
                        SortableTh {
                            column, label: column.to_owned(), active: sort.clone(),
                            direction: SortDirection::Desc, on_sort: |_| {},
                            omit_inactive_aria_sort: true,
                        }
                    }
                } }
            }
        }
    }

    fn aria_sort_values(edits: &[Mutation]) -> Vec<&str> {
        edits
            .iter()
            .filter_map(|edit| match edit {
                Mutation::SetAttribute {
                    name: "aria-sort",
                    value: AttributeValue::Text(value),
                    ..
                } => Some(value.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn mobile_sort_exposes_the_four_columns_and_active_direction() {
        let mutations =
            VirtualDom::new_with_props(sort_control, "current_price".to_owned()).rebuild_to_vec();
        let selector = mutations
            .edits
            .iter()
            .find_map(|edit| match edit {
                Mutation::NewEventListener { name, id } if name == "change" => Some(*id),
                _ => None,
            })
            .unwrap();
        let mut options = BTreeSet::new();
        let mut selected = None;
        for edit in &mutations.edits {
            if let Mutation::SetAttribute {
                id,
                name: "value",
                value: AttributeValue::Text(value),
                ..
            } = edit
            {
                if *id == selector {
                    selected = Some(value.as_str());
                } else {
                    options.insert(value.as_str());
                }
            }
        }
        assert_eq!(options, BTreeSet::from(["code", "status", "start_price", "current_price"]));
        assert_eq!(selected, Some("current_price"));
        assert!(mutations.edits.iter().any(|edit| matches!(edit,
            Mutation::SetAttribute { name: "aria-label", value: AttributeValue::Text(value), .. }
                if value.contains("当前为降序")
        )));
        assert_eq!(aria_sort_values(&mutations.edits), vec!["descending"]);
        assert!(mutations.edits.iter().all(|edit| !matches!(
            edit,
            Mutation::SetAttribute { name: "disabled", value: AttributeValue::Bool(true), .. }
        )));
    }

    #[test]
    fn default_mobile_order_has_no_enabled_direction() {
        let mutations = VirtualDom::new_with_props(sort_control, String::new()).rebuild_to_vec();
        let button = mutations
            .edits
            .iter()
            .find_map(|edit| match edit {
                Mutation::NewEventListener { name, id } if name == "click" => Some(*id),
                _ => None,
            })
            .unwrap();
        assert!(mutations.edits.iter().any(|edit| matches!(edit,
            Mutation::SetAttribute { id, name: "disabled", value: AttributeValue::Bool(true), .. }
                if *id == button
        )));
        assert!(mutations.edits.iter().any(|edit| matches!(edit,
            Mutation::CreateTextNode { value, .. } if value == "默认顺序"
        )));
        assert!(mutations.edits.iter().all(|edit| !matches!(edit,
            Mutation::SetAttribute { id, name: "aria-label", value: AttributeValue::Text(value), .. }
                if *id == button && (value.contains("升序") || value.contains("降序"))
        )));
        assert!(aria_sort_values(&mutations.edits).is_empty());
    }
}
