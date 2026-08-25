//! Public lot 装备属性和游戏来源展示。

use super::*;

#[component]
pub(crate) fn EquipmentCard(equipment: EquipmentDetail) -> Element {
    let mut stats_page = use_signal(|| 1_i64);
    let stats_page_size = 10_i64;
    let current_stats_page = stats_page().max(1);
    let stats_total = i64::try_from(equipment.stats.len()).unwrap_or(i64::MAX);
    let stats_start =
        usize::try_from(current_stats_page.saturating_sub(1).saturating_mul(stats_page_size))
            .unwrap_or(usize::MAX);
    rsx! {
        div { class: "public-auction-equipment",
            h2 { "{equipment.name}" }
            dl {
                dt { {t!("equipment-category")} }
                dd { {catalog_category_label(catalog_category_key(&equipment.category))} }
                if let Some(level) = equipment.level {
                    dt { {t!("equipment-level")} }
                    dd { "{level}" }
                }
            }
            if !equipment.info_summary.is_empty() {
                p { class: "public-auction-item-summary", "{equipment.info_summary}" }
            }
            if let Some(avg_roll) = equipment.avg_roll {
                p { class: "public-auction-equipment__avg-roll",
                    span { {t!("equipment-avg-roll")} }
                    strong { "{avg_roll:.1}%" }
                }
            }
            if !equipment.stats.is_empty() {
                div { class: "public-auction-equipment__stats-wrap",
                    table { class: "public-auction-equipment__stats",
                        caption { {t!("equipment-stats")} }
                        thead {
                            tr {
                                th { {t!("equipment-stat-name")} }
                                th { {t!("equipment-stat-value")} }
                                th { {t!("equipment-stat-base")} }
                                th { {t!("equipment-stat-roll")} }
                            }
                        }
                        tbody {
                            for (name, stat) in equipment.stats.iter().skip(stats_start).take(stats_page_size as usize) {
                                tr {
                                    th { scope: "row", "{name}" }
                                    td { "{stat.value}" }
                                    td { {stat.base.map(|value| format!("{value:.2}")).unwrap_or_else(|| t!("value-empty"))} }
                                    td { {stat.roll.map(|value| format!("{value:.1}%")).unwrap_or_else(|| t!("value-empty"))} }
                                }
                            }
                        }
                    }
                }
                PublicPager {
                    page: current_stats_page,
                    page_size: stats_page_size,
                    total: Some(stats_total),
                    has_more: false,
                    label: t!("equipment-stats"),
                    on_page_change: move |next_page| stats_page.set(next_page),
                }
            }
            if !equipment.game_equipment_url.is_empty() {
                a {
                    class: "public-auction-equipment__link",
                    href: equipment.game_equipment_url,
                    target: "_blank",
                    rel: "noreferrer",
                    {t!("equipment-view-in-game")}
                }
            }
        }
    }
}
