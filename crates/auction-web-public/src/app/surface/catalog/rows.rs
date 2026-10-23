//! Public 拍卖目录的实时、预览和参与者行。

use super::*;

/// 公开拍卖参与者标识；长名称保持可读的省略样式，并保留旧版的代理/当前用户标记。
#[component]
pub(crate) fn CatalogParticipant(
    name: String,
    marker: Option<String>,
    trailing: Option<String>,
) -> Element {
    rsx! {
        span { class: "public-auction-participant-identity",
            if let Some(marker) = marker {
                span {
                    class: "public-auction-participant-identity__marker public-auction-participant-identity__marker--leading",
                    "{marker}"
                }
            }
            TruncatedText {
                value: name,
                size: "compact",
                class_name: "public-auction-participant-identity__name",
                portal: true,
                legacy_dom: true,
            }
            if let Some(trailing) = trailing {
                span {
                    class: "public-auction-participant-identity__marker public-auction-participant-identity__marker--trailing",
                    "{trailing}"
                }
            }
        }
    }
}

/// 竞价历史明确标记普通/代理来源；论坛楼层紧贴玩家名称并链接真实帖子。
#[component]
pub(crate) fn BidHistoryParticipant(
    name: String,
    source: Option<String>,
    source_post_number: Option<i64>,
    source_post_url: Option<String>,
) -> Element {
    let forum_post_number =
        (source.as_deref() == Some("forum")).then_some(source_post_number).flatten();
    let forum_unknown = source.as_deref() == Some("forum") && forum_post_number.is_none();
    let proxy = source.as_deref() == Some("proxy");
    rsx! {
        span { class: "public-auction-participant-identity public-auction-participant-identity--source",
            TruncatedText {
                value: name,
                size: "compact",
                class_name: "public-auction-participant-identity__name",
                portal: true,
                legacy_dom: true,
            }
            if let Some(post_number) = forum_post_number {
                if let Some(url) = source_post_url {
                    a {
                        class: "public-auction-participant-identity__marker public-auction-participant-identity__marker--trailing",
                        href: url,
                        target: "_blank",
                        rel: "noopener noreferrer",
                        aria_label: t!("bidlog-source-forum-link-label", post_number: post_number),
                        {t!("bidlog-source-forum", post_number: post_number)}
                    }
                } else {
                    span {
                        class: "public-auction-participant-identity__marker public-auction-participant-identity__marker--trailing",
                        {t!("bidlog-source-forum", post_number: post_number)}
                    }
                }
            } else if forum_unknown {
                span {
                    class: "public-auction-participant-identity__marker public-auction-participant-identity__marker--trailing",
                    {t!("bidlog-source-forum-unknown")}
                }
            } else if proxy {
                span {
                    class: "public-auction-participant-identity__marker public-auction-participant-identity__marker--trailing",
                    {t!("bidlog-source-proxy")}
                }
            } else if source.is_none() {
                span {
                    class: "public-auction-participant-identity__marker public-auction-participant-identity__marker--trailing",
                    {t!("bidlog-source-ordinary")}
                }
            }
        }
    }
}

/// 装备详情以独立文字标签表达服务端装备分组。
#[component]
pub(crate) fn EquipmentGroupBadge(equipment_group: String) -> Element {
    let class = if equipment_group == "rare" {
        "public-equipment-group public-equipment-group--rare"
    } else {
        "public-equipment-group"
    };
    rsx! {
        span { class, {equipment_group_label(&equipment_group)} }
    }
}

/// 装备列仅展示名称，保持单行截断并直接导航真实装备 URL。
#[component]
pub(crate) fn CatalogEquipmentName(name: String, game_equipment_url: String) -> Element {
    rsx! {
        span { class: "public-auction-catalog__equipment",
            if game_equipment_url.is_empty() {
                TruncatedText {
                    value: name.clone(),
                    class_name: "ui-equipment-name",
                    portal: true,
                    legacy_dom: true,
                }
            } else {
                a {
                    class: "public-auction-catalog__equipment-link",
                    href: game_equipment_url,
                    target: "_blank",
                    rel: "noopener noreferrer",
                    aria_label: name.clone(),
                    title: name.clone(),
                    TruncatedText {
                        value: name.clone(),
                        class_name: "ui-equipment-name",
                        focusable: false,
                        portal: true,
                        legacy_dom: true,
                    }
                }
            }
        }
    }
}

#[component]
pub(crate) fn PreviewCatalogRow(row: CatalogRow, visibility: CatalogVisibility) -> Element {
    let status_label = lot_status_label(&row.status);
    let status_class = lot_status_class(&row.status);
    let level = row.level.map(|value| value.to_string()).unwrap_or_else(|| t!("value-empty"));
    let primary_attribute_initials =
        row.primary_attribute_initials.clone().unwrap_or_else(|| t!("value-empty"));
    let avg_roll = row.avg_roll.map(|value| format!("{value:.1}%"));
    let seller = row.seller_name.clone();
    let start_price = format_amount(row.start_price);

    rsx! {
        tr {
            td { hidden: !visibility.contains(CatalogColumn::Code), "data-label": t!("catalog-code"),
                Link {
                    class: "public-auction-catalog__code",
                    to: lot_detail_href(&row.lot_id),
                    "{row.code}"
                }
            }
            td { hidden: !visibility.contains(CatalogColumn::Item), class: "public-auction-catalog__name", "data-label": t!("catalog-item"),
                CatalogEquipmentName {
                    name: row.name.clone(),
                    game_equipment_url: row.game_equipment_url.clone(),
                }
            }
            td { hidden: !visibility.contains(CatalogColumn::Group), "data-label": t!("catalog-group"),
                TruncatedText {
                    value: equipment_group_label(&row.equipment_group),
                    portal: true,
                    legacy_dom: true,
                }
            }
            td {
                hidden: !visibility.contains(CatalogColumn::AverageRoll),
                class: "public-auction-catalog__avg-roll public-auction-catalog__number",
                "data-label": t!("equipment-avg-roll"),
                if let Some(avg_roll) = avg_roll {
                    "{avg_roll}"
                } else {
                    span { class: "public-auction-catalog__none", {t!("value-empty")} }
                }
            }
            td { hidden: !visibility.contains(CatalogColumn::Level), "data-label": t!("catalog-level"), "{level}" }
            td {
                hidden: !visibility.contains(CatalogColumn::Attributes),
                class: "public-auction-catalog__primary-attributes",
                "data-label": t!("catalog-primary-attributes"),
                "{primary_attribute_initials}"
            }
            td { hidden: !visibility.contains(CatalogColumn::Seller), "data-label": t!("catalog-seller"),
                TruncatedText {
                    value: seller,
                    portal: true,
                    legacy_dom: true,
                }
            }
            td {
                hidden: !visibility.contains(CatalogColumn::Price),
                class: "public-auction-catalog__amount public-auction-catalog__number",
                "data-label": t!("catalog-start-price"),
                "{start_price}"
            }
            td { hidden: !visibility.contains(CatalogColumn::Leader), class: "public-auction-catalog__participant-cell", "data-label": t!("catalog-leader"),
                span { class: "public-auction-catalog__none", {t!("value-empty")} }
            }
            td { hidden: !visibility.contains(CatalogColumn::Remaining), "data-label": t!("catalog-remaining"),
                span { class: "public-auction-countdown--none", {t!("value-empty")} }
            }
            td { hidden: !visibility.contains(CatalogColumn::Status), "data-label": t!("catalog-status"),
                span { class: "public-auction-status public-auction-status--{status_class}", "{status_label}" }
            }
        }
    }
}
