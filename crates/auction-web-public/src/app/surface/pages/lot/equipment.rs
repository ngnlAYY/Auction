//! Public lot 装备属性和游戏来源展示。

use super::*;
use crate::api::{EquipmentDetail, PreviewEquipmentDetail};
use auction_contracts::PublicEquipmentStatResponse;
use std::collections::BTreeMap;

/// 详情页使用的统一展示模型；preview 与 live 的 wire DTO 在此处保持各自字段约束。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct EquipmentCardData {
    pub(crate) name: String,
    pub(crate) category: Option<String>,
    pub(crate) equipment_group: String,
    pub(crate) level: Option<i64>,
    pub(crate) stats: BTreeMap<String, PublicEquipmentStatResponse>,
    pub(crate) avg_roll: Option<f64>,
    pub(crate) game_equipment_url: String,
}

impl From<EquipmentDetail> for EquipmentCardData {
    fn from(equipment: EquipmentDetail) -> Self {
        Self {
            name: equipment.name,
            equipment_group: equipment.equipment_group,
            category: Some(equipment.category),
            level: equipment.level,
            stats: equipment.stats,
            avg_roll: equipment.avg_roll,
            game_equipment_url: equipment.game_equipment_url,
        }
    }
}

impl From<PreviewEquipmentDetail> for EquipmentCardData {
    fn from(equipment: PreviewEquipmentDetail) -> Self {
        Self {
            name: equipment.name,
            equipment_group: equipment.equipment_group,
            category: equipment.category,
            level: equipment.level,
            stats: equipment.stats,
            avg_roll: equipment.avg_roll,
            game_equipment_url: equipment.game_equipment_url,
        }
    }
}

#[component]
pub(crate) fn EquipmentCard(equipment: EquipmentCardData, leading: bool) -> Element {
    let mut stats_page = use_signal(|| 1_i64);
    let stats_page_size = 10_i64;
    let current_stats_page = stats_page().max(1);
    let stats_total = i64::try_from(equipment.stats.len()).unwrap_or(i64::MAX);
    let stats_start =
        usize::try_from(current_stats_page.saturating_sub(1).saturating_mul(stats_page_size))
            .unwrap_or(usize::MAX);
    let game_equipment_href = equipment.game_equipment_url.clone();
    let category_label = equipment
        .category
        .as_deref()
        .map(|category| catalog_category_label(catalog_category_key(category)));
    let leading_class = if leading { " public-auction-equipment--leading" } else { "" };
    rsx! {
        div {
            class: "public-auction-equipment{leading_class}",
            aria_describedby: if leading { "public-auction-equipment-leading" } else { "" },
            if leading {
                span {
                    id: "public-auction-equipment-leading",
                    class: "public-visually-hidden",
                    {t!("bid-leading")}
                }
            }
            if game_equipment_href.is_empty() {
                h2 {
                    TruncatedText {
                        value: equipment.name.clone(),
                        size: "wide",
                        class_name: "public-auction-equipment__name",
                        portal: true,
                    }
                }
            } else {
                h2 {
                    a {
                        class: "public-auction-equipment__name-link",
                        href: game_equipment_href.clone(),
                        target: "_blank",
                        rel: "noopener noreferrer",
                        aria_label: equipment.name.clone(),
                        title: equipment.name.clone(),
                        TruncatedText {
                            value: equipment.name.clone(),
                            size: "wide",
                            class_name: "public-auction-equipment__name",
                            focusable: false,
                            portal: true,
                        }
                    }
                }
            }
            div { class: "public-auction-equipment__facts",
                dl { class: "public-auction-equipment__facts-list",
                    div { class: "public-auction-equipment__fact",
                        dt { {t!("equipment-group")} }
                        dd { EquipmentGroupBadge { equipment_group: equipment.equipment_group.clone() } }
                    }
                    if let Some(category) = category_label.as_deref() {
                        div { class: "public-auction-equipment__fact",
                            dt { {t!("equipment-category")} }
                            dd {
                                TruncatedText {
                                    value: category.to_owned(),
                                    size: "compact",
                                    class_name: "public-auction-equipment__fact-value",
                                    portal: true,
                                }
                            }
                        }
                    }
                    if let Some(level) = equipment.level {
                        div { class: "public-auction-equipment__fact",
                            dt { {t!("equipment-level")} }
                            dd { "{level}" }
                        }
                    }
                    if let Some(avg_roll) = equipment.avg_roll {
                        div { class: "public-auction-equipment__fact",
                            dt { {t!("equipment-avg-roll")} }
                            dd { class: "public-auction-equipment__avg-roll", "{avg_roll:.1}%" }
                        }
                    }
                }
            }
            if !equipment.stats.is_empty() {
                div { class: "public-auction-equipment__stats-wrap",
                    table { class: "public-auction-equipment__stats ui-table--responsive",
                        caption { class: "public-visually-hidden", {t!("equipment-stats")} }
                        auction_web_shared::TableColumns { layout: "public-equipment-stats", widths: vec![42, 24, 17, 17] }
                        thead {
                            tr {
                                th { scope: "col", {t!("equipment-stat-name")} }
                                th { scope: "col", {t!("equipment-stat-value")} }
                                th { scope: "col", {t!("equipment-stat-base")} }
                                th { scope: "col", {t!("equipment-stat-roll")} }
                            }
                        }
                        tbody {
                            for (name, stat) in equipment.stats.iter().skip(stats_start).take(stats_page_size as usize) {
                                tr {
                                    th { scope: "row", "data-label": t!("equipment-stat-name"),
                                        TruncatedText { value: name.clone(), size: "compact", portal: true }
                                    }
                                    td { "data-label": t!("equipment-stat-value"),
                                        TruncatedText { value: stat.value.clone(), size: "compact", portal: true }
                                    }
                                    td { "data-label": t!("equipment-stat-base"), {stat.base.map(|value| format!("{value:.2}")).unwrap_or_else(|| t!("value-empty"))} }
                                    td { "data-label": t!("equipment-stat-roll"), {stat.roll.map(|value| format!("{value:.1}%")).unwrap_or_else(|| t!("value-empty"))} }
                                }
                            }
                        }
                    }
                }
                Pager {
                    page: current_stats_page,
                    page_size: stats_page_size,
                    total: Some(stats_total),
                    has_more: false,
                    label: t!("equipment-stats"),
                    previous_label: t!("catalog-previous"),
                    status: move |(page, pages)| t!("catalog-page", page: page, pages: pages),
                    next_label: t!("catalog-next"),
                    jump_label: t!("catalog-page-input"),
                    jump_button_label: t!("catalog-page-go"),
                    on_page_change: move |next_page| stats_page.set(next_page),
                }
            }
            if !equipment.game_equipment_url.is_empty() {
                a {
                    class: "public-auction-equipment__link",
                    href: game_equipment_href.clone(),
                    target: "_blank",
                    rel: "noopener noreferrer",
                    {t!("equipment-view-in-game")}
                }
            }
        }
    }
}
