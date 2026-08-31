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

#[component]
pub(crate) fn PreviewCatalogRow(row: CatalogRow) -> Element {
    let status_label = lot_status_label(&row.status);
    let status_class = lot_status_class(&row.status);
    let level = row.level.map(|value| value.to_string()).unwrap_or_else(|| t!("value-empty"));
    let avg_roll = row.avg_roll.map(|value| format!("{value:.1}%"));
    let seller = row.seller_name.clone();
    let start_price = format_amount(row.start_price);

    rsx! {
        tr {
            td { "data-label": t!("catalog-code"),
                Link {
                    class: "public-auction-catalog__code",
                    to: lot_detail_href(&row.lot_id),
                    "{row.code}"
                }
            }
            td { "data-label": t!("catalog-item"),
                TruncatedText {
                    value: row.name.clone(),
                    class_name: "ui-equipment-name",
                    portal: true,
                    legacy_dom: true,
                }
            }
            td {
                class: "public-auction-catalog__avg-roll public-auction-catalog__number",
                "data-label": t!("equipment-avg-roll"),
                if let Some(avg_roll) = avg_roll {
                    "{avg_roll}"
                } else {
                    span { class: "public-auction-catalog__none", {t!("value-empty")} }
                }
            }
            td { "data-label": t!("catalog-level"), "{level}" }
            td { "data-label": t!("catalog-seller"),
                TruncatedText {
                    value: seller,
                    portal: true,
                    legacy_dom: true,
                }
            }
            td {
                class: "public-auction-catalog__amount public-auction-catalog__number",
                "data-label": t!("catalog-start-price"),
                "{start_price}"
            }
            td { class: "public-auction-catalog__participant-cell", "data-label": t!("catalog-leader"),
                span { class: "public-auction-catalog__none", {t!("value-empty")} }
            }
            td { "data-label": t!("catalog-remaining"),
                span { class: "public-auction-countdown--none", {t!("value-empty")} }
            }
            td { "data-label": t!("catalog-status"),
                span { class: "public-auction-status public-auction-status--{status_class}", "{status_label}" }
            }
            td { class: "public-auction-catalog__actions", "data-label": t!("catalog-actions"),
                span { class: "public-auction-catalog__actions-list",
                    Link { to: lot_detail_href(&row.lot_id), {t!("catalog-view")} }
                }
            }
        }
    }
}
