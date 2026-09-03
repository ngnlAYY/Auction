//! Public 拍卖的预览与历史视图切换。

use super::*;

pub(crate) fn preview_history_switch(
    state: &Option<PreviewHistoryLoad>,
    auction: &CurrentAuction,
    historical: bool,
) -> Element {
    if !historical && auction.presentation_mode == "live" {
        return rsx! {};
    }

    let Some(state) = state else {
        return rsx! {
            nav { class: "public-auction-view-switch", aria_label: t!("auction-history-view-label"),
                span { class: "public-auction-view-switch__options",
                    span { class: "public-auction-view-switch__status", {t!("data-loading")} }
                }
            }
        };
    };
    let Ok(Some((current, rounds))) = state else {
        return rsx! {
            nav { class: "public-auction-view-switch", aria_label: t!("auction-history-view-label"),
                span { class: "public-auction-view-switch__options",
                    span { class: "public-auction-view-switch__status", {t!("data-error")} }
                }
            }
        };
    };
    let Some(current) = current else {
        return rsx! {};
    };
    if current.presentation_mode == "live" {
        return rsx! {};
    }

    let latest_settled = rounds
        .iter()
        .filter(|round| round.world_id == current.world)
        .max_by_key(|round| round.sequence);
    if historical && latest_settled.is_none_or(|round| round.id != auction.id) {
        return rsx! {};
    }

    let mut current_query = read_public_query();
    current_query.auction_id = None;
    current_query.world = normalize_world_query(&current.world);
    let current_href = public_href("/auction", &current_query);
    let previous = rounds
        .iter()
        .filter(|round| round.world_id == current.world && round.sequence < current.number)
        .max_by_key(|round| round.sequence);
    let previous_href = previous.map(|round| {
        let mut query = read_public_query();
        query.auction_id = Some(round.id.clone());
        query.world = normalize_world_query(&round.world_id);
        public_href("/auction", &query)
    });

    rsx! {
        nav { class: "public-auction-view-switch", aria_label: t!("auction-history-view-label"),
            span { class: "public-auction-view-switch__options",
                if historical {
                    Link { to: current_href, {t!("auction-history-current")} }
                } else if let Some(previous_href) = previous_href {
                    Link { to: previous_href, {t!("auction-history-previous-view")} }
                } else {
                    span { {t!("auction-history-no-previous")} }
                }
            }
        }
    }
}
