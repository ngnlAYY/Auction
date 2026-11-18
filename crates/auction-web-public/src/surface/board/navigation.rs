//! Public 拍卖的预览与历史视图切换。

use auction_web_shared::t;
use dioxus::prelude::*;

use crate::api::{CurrentAuction, fetch_auction_list, fetch_current_auction};
use crate::surface::{
    PreviewHistoryLoad, normalize_world_query, public_href_for_auction, read_public_query,
    use_content_loading,
};

pub(super) fn use_preview_history_switch(
    auction: &CurrentAuction,
    historical: bool,
    refresh_generation: Signal<u64>,
) -> Element {
    let eligible_view = historical || auction.presentation_mode != "live";
    let world = use_memo(use_reactive((&auction.world, &eligible_view), |(world, eligible)| {
        eligible.then_some(world)
    }));
    let history_switch = use_resource(move || {
        let _ = refresh_generation();
        let requested = world();
        async move {
            let result = async {
                let Some(world) = requested.as_ref() else { return Ok(None) };
                let Some(current) = fetch_current_auction(world.clone())
                    .await?
                    .filter(|current| current.presentation_mode != "live")
                else {
                    return Ok(None);
                };
                let rounds = fetch_auction_list(world.clone()).await?;
                Ok(Some((current, rounds)))
            }
            .await;
            (requested, result)
        }
    });
    // 换世界时旧辅助结果不能冒充本次读取。当前场次为空或在竞价时也不展示切换器。
    let snapshot = history_switch.read();
    let active_world = world();
    let current =
        snapshot.as_ref().filter(|(world, _)| world == &active_world).map(|(_, result)| result);
    use_content_loading(eligible_view && current.is_none());
    preview_history_switch(current, auction, historical)
}

fn preview_history_switch(
    state: Option<&PreviewHistoryLoad>,
    auction: &CurrentAuction,
    historical: bool,
) -> Element {
    if !historical && auction.presentation_mode == "live" {
        return rsx! {};
    }

    // 读取期间不占位，加载提示由调用方登记到顶栏。
    let Some(state) = state else {
        return rsx! {};
    };
    let (current, rounds) = match state {
        Ok(Some(value)) => value,
        Ok(None) => return rsx! {},
        Err(_error) => {
            return rsx! {
                nav { class: "public-auction-view-switch", aria_label: t!("auction-history-view-label"),
                    span { class: "public-auction-view-switch__options",
                        span { class: "public-auction-view-switch__status", {t!("data-error")} }
                    }
                }
            };
        }
    };
    let latest_settled = rounds
        .iter()
        .filter(|round| round.world_id == current.world)
        .max_by_key(|round| round.sequence);
    if historical && latest_settled.is_none_or(|round| round.id != auction.id) {
        return rsx! {};
    }

    let mut current_query = read_public_query();
    current_query.world = normalize_world_query(&current.world).into();
    let current_href = public_href_for_auction("/auction", current_query, None);
    let previous = rounds
        .iter()
        .filter(|round| round.world_id == current.world && round.sequence < current.number)
        .max_by_key(|round| round.sequence);
    let previous_href = previous.map(|round| {
        let mut query = read_public_query();
        query.world = normalize_world_query(&round.world_id).into();
        public_href_for_auction("/auction", query, Some(round.id.clone()))
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
