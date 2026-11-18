//! Public 价格历史页面及其筛选、值规范化和结果表格。

use auction_web_shared::{AsyncState, AsyncStateKind, QueryWriteMode, t};
use dioxus::prelude::*;

use crate::api::{PriceHistoryFetchError, fetch_price_history, fetch_worlds};
use crate::surface::{
    LotDetailLookupHost, PageKey, PublicPageClampError, commit_public_query_pairs, page_pathname,
    public_page_matches, set_public_page_size, supported_worlds, use_content_loading,
    use_lot_detail_lookup, use_page_query, use_public_page_bounds,
};

mod filters;
mod presentation;
mod table;
mod values;

use self::filters::{HISTORY_PATH, PriceHistoryFilters, use_history_form};
use self::presentation::HistoryResultControls;
use self::table::PriceHistoryTable;
use self::values::{history_active_filter_count, history_player_error};
pub(crate) use filters::history_equipment_name_query;

pub(crate) fn history() -> Element {
    let mut refresh_generation = use_context::<Signal<u64>>();
    // 查看按钮使用页内抽屉；查询切换不会卸载抽屉宿主。
    let lookup = use_lot_detail_lookup();
    let query_state = use_page_query();
    let page_size = use_memo(move || query_state.read().page_size);
    let bounds = use_public_page_bounds(query_state, HISTORY_PATH, |query| {
        [(PageKey::Page, query.page_size)]
    });
    let form = use_history_form(query_state);
    let applied = form.applied;
    let request = use_memo(move || applied.read().request(page_size()));
    let worlds = use_resource(fetch_worlds);
    let world_options = match &*worlds.read_unchecked() {
        Some(Ok(worlds)) => supported_worlds(worlds),
        _ => vec!["main".to_owned(), "isekai".to_owned()],
    };
    let has_search = applied.read().has_request();
    let clamp_failed = bounds.failed(PageKey::Page);
    let history = use_resource(move || {
        let _ = refresh_generation();
        let requested = request();
        async move {
            let (query, size) = requested?;
            let result = fetch_price_history(query.clone(), size).await;
            Some((query, size, result))
        }
    });
    // Resource 会保留旧结果；筛选改变或被判非法时不能显示前一次结果。
    let history_snapshot = history.read_unchecked();
    let current = request.read();
    let history_result = history_snapshot
        .as_ref()
        .and_then(Option::as_ref)
        .filter(|(query, size, _)| {
            current.as_ref().is_some_and(|(now, length)| query == now && size == length)
        })
        .map(|(_, _, result)| result);
    let active_filter_count = history_active_filter_count(&applied.read().query);
    let page_number = applied.read().query.page;
    let sort = applied.read().query.sort.clone();
    let grouping = query_state.read().history_group.clone();
    let table_scope_key = format!("{:?}:{}", applied.read().query, page_size());
    use_content_loading(
        has_search
            && !clamp_failed
            && match history_result {
                None => true,
                Some(Ok(page)) => {
                    !public_page_matches(page.page, page.page_size, page_number, page_size())
                }
                Some(Err(PriceHistoryFetchError)) => false,
            },
    );
    let body = match history_result {
        _ if clamp_failed => rsx! { PublicPageClampError { bounds } },
        None => rsx! {},
        Some(Err(PriceHistoryFetchError)) => rsx! {
            AsyncState {
                kind: AsyncStateKind::Error,
                title: t!("page-history-title"),
                detail: t!("data-error"),
                retry_label: t!("data-retry"),
                on_retry: move |_| {
                    refresh_generation.set(refresh_generation().saturating_add(1));
                },
            }
        },
        Some(Ok(page))
            if public_page_matches(page.page, page.page_size, page_number, page_size()) =>
        {
            rsx! {
                // 查询身份隔离折叠状态；同查询后台重读保持稳定分组键。
                for (table_scope_key, page) in [(table_scope_key, page.clone())] {
                    PriceHistoryTable {
                        key: "{table_scope_key}",
                        page,
                        grouping: grouping.clone(),
                        current_page: page_number,
                        on_buyer_filter: move |name| filter_history_player(query_state, "buyer", name),
                        on_seller_filter: move |name| filter_history_player(query_state, "seller", name),
                        on_page_size_change: move |size| page_pathname() == HISTORY_PATH
                            && set_public_page_size(query_state, size),
                        on_page_change: move |page| PageKey::Page.write(query_state, HISTORY_PATH, page, QueryWriteMode::Push),
                        on_page_clamp: move |page| PageKey::Page.write(query_state, HISTORY_PATH, page, QueryWriteMode::Replace),
                    }
                }
            }
        }
        Some(Ok(_)) => rsx! {},
    };
    rsx! {
        section { class: "public-history-page", aria_label: t!("page-history-title"),
            div { class: "public-history-search-region",
                PriceHistoryFilters { form, world_options, active_filter_count }
            }
            if has_search {
                div { class: "public-history-output",
                    HistoryResultControls {
                        sort,
                        grouping,
                        on_sort_change: move |sort| write_history_control(query_state, &["history_sort", "page"], "history_sort", sort),
                        on_group_change: move |group| {
                            // 分组只重排已获取的本页，不重新读取相同成交集合。
                            write_history_control(query_state, &["history_group"], "history_group", group)
                        },
                    }
                    {body}
                }
            } else if clamp_failed {
                {body}
            }
            LotDetailLookupHost { lookup }
        }
    }
}

fn write_history_control(
    state: Signal<crate::surface::PublicBoardQuery>,
    keys: &[&str],
    key: &str,
    value: String,
) -> bool {
    page_pathname() == HISTORY_PATH
        && commit_public_query_pairs(state, keys, &[(key.to_owned(), value)], QueryWriteMode::Push)
}

fn filter_history_player(
    state: Signal<crate::surface::PublicBoardQuery>,
    key: &str,
    value: String,
) {
    if !value.is_empty() && history_player_error(&value).is_none() {
        write_history_control(state, &[key, "page"], key, value);
    }
}
