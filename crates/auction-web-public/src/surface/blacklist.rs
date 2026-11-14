//! Public 黑名单页面。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use auction_web_shared::TruncatedText;

use crate::api::{BlacklistEntry, BlacklistPage, fetch_blacklist};

use super::{
    AsyncState, AsyncStateKind, PublicPager, QueryWriteMode, Time, public_page_matches,
    update_public_page_size, update_public_query_state, use_content_loading,
    use_public_query_state,
};

pub(super) fn page() -> Element {
    let mut refresh_generation = use_context::<Signal<u64>>();
    // 页码与地址栏双向同步：初值取自 URL，浏览器前进/后退由 hook 恢复。
    let query_state = use_public_query_state();
    let query_state_signal = query_state.state;
    let page_number = use_memo(move || query_state_signal.read().page.max(1));
    let page_size = use_memo(move || query_state_signal.read().page_size);
    let blacklist = use_resource(move || {
        let _ = refresh_generation();
        let requested = (page_number(), page_size());
        async move {
            let result = fetch_blacklist(requested.0, requested.1).await;
            (requested, result)
        }
    });
    let loaded = blacklist.read_unchecked();
    let current = loaded
        .as_ref()
        .filter(|(requested, _)| *requested == (page_number(), page_size()))
        .map(|(_, result)| result);
    // 没有当前页码的结果（含等待页码纠偏后的重读）时由顶栏提示加载，正文留空。
    use_content_loading(match current {
        None => true,
        Some(Ok(page)) => {
            !public_page_matches(page.page, page.page_size, page_number(), page_size())
        }
        Some(Err(_)) => false,
    });
    match current {
        None => rsx! {},
        Some(Err(_)) => rsx! {
            AsyncState {
                kind: AsyncStateKind::Error,
                title: t!("page-blacklist-title"),
                detail: t!("data-error"),
                retry_label: t!("data-retry"),
                on_retry: move |_| {
                    refresh_generation.set(refresh_generation().saturating_add(1));
                },
            }
        },
        Some(Ok(page))
            if !public_page_matches(page.page, page.page_size, page_number(), page_size()) =>
        {
            rsx! {}
        }
        Some(Ok(page)) => rsx! {
            BlacklistTable {
                page: page.clone(),
                on_page_size_change: move |size| update_public_page_size(query_state_signal, size),
                on_page_change: move |next_page| {
                    update_public_query_state(
                        query_state_signal,
                        |query| query.page = next_page,
                        QueryWriteMode::Push,
                    );
                },
                on_page_clamp: move |next_page| {
                    update_public_query_state(
                        query_state_signal,
                        |query| query.page = next_page,
                        QueryWriteMode::Replace,
                    );
                },
            }
        },
    }
}

#[component]
fn BlacklistTable(
    page: BlacklistPage,
    on_page_change: EventHandler<i64>,
    on_page_clamp: EventHandler<i64>,
    on_page_size_change: EventHandler<i64>,
) -> Element {
    rsx! {
        section { class: "public-blacklist-page", aria_label: t!("page-blacklist-title"),
            if page.users.is_empty() {
                p { class: "public-blacklist-empty", {t!("blacklist-empty")} }
            } else {
                div { class: "public-blacklist-table-wrap",
                    table { class: "public-blacklist-table",
                        auction_web_shared::TableColumns { layout: "public-blacklist", widths: vec![22, 54, 24] }
                        thead {
                            tr {
                                th { {t!("blacklist-player")} }
                                th { {t!("blacklist-reason")} }
                                th { {t!("blacklist-time")} }
                            }
                        }
                        tbody {
                            for entry in page.users {
                                BlacklistRow { entry }
                            }
                        }
                    }
                }
            }
            // 没有任何记录时分页器只会显示“共 0 条”，不渲染；有记录但页码越界时仍需它纠偏。
            if page.total > 0 {
                PublicPager {
                    page: page.page,
                    page_size: page.page_size,
                    total: Some(page.total),
                    has_more: false,
                    label: t!("page-blacklist-title"),
                    max_offset: Some(i64::from(i32::MAX)),
                    on_page_change,
                    on_page_clamp,
                    on_page_size_change,
                }
            }
        }
    }
}

#[component]
fn BlacklistRow(entry: BlacklistEntry) -> Element {
    rsx! {
        tr {
            td { "data-label": t!("blacklist-player"),
                TruncatedText { value: entry.username, portal: true }
            }
            td { "data-label": t!("blacklist-reason"),
                TruncatedText { value: entry.reason, size: "wide", portal: true }
            }
            td {
                "data-label": t!("blacklist-time"),
                Time { value: entry.blacklisted_at }
            }
        }
    }
}
