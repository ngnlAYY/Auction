//! Public 论坛同步独立页面。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;
use gloo_timers::future::TimeoutFuture;

use crate::api::{CurrentAuction, ForumThread, fetch_forum_sync_enabled, fetch_forum_thread};

use super::{CurrentAuctionResource, auction_world_label, format_auction_time};

/// 先读取独立的开关投影，明确启用后才挂载当前拍卖的论坛进度。
pub(super) fn page() -> Element {
    let mut status_generation = use_signal(|| 0_u64);
    let refresh_generation = use_context::<Signal<u64>>();
    let sync_enabled = use_resource(move || {
        let _ = status_generation();
        let _ = refresh_generation();
        async move { fetch_forum_sync_enabled().await }
    });
    // 关闭和短暂不可用时也定期重读，以便 Worker 重启后自动恢复，不能继续显示旧楼层。
    let _poll = use_resource(move || {
        let _ = status_generation();
        async move {
            TimeoutFuture::new(30_000).await;
            status_generation.set(status_generation().saturating_add(1));
        }
    });
    let content = match sync_enabled.read().clone() {
        None => forum_page_state(t!("forum-loading"), false, None),
        Some(Ok(false)) => forum_page_state(t!("forum-sync-disabled"), false, None),
        Some(Ok(true)) => rsx! { ForumCurrentAuction {} },
        Some(Err(_)) => forum_page_state(
            t!("forum-unavailable"),
            true,
            Some(rsx! {
                button {
                    class: "public-state-retry",
                    r#type: "button",
                    onclick: move |_| {
                        status_generation.set(status_generation().saturating_add(1));
                    },
                    {t!("forum-retry")}
                }
            }),
        ),
    };

    rsx! {
        section { class: "public-forum-page", aria_labelledby: "public-forum-title",
            header { class: "public-forum-page__intro",
                h1 { id: "public-forum-title", {t!("page-forum-title")} }
                p { {t!("page-forum-note")} }
            }
            {content}
        }
    }
}

#[component]
fn ForumCurrentAuction() -> Element {
    let selected_world = use_context::<Signal<String>>();
    let mut refresh_generation = use_context::<Signal<u64>>();
    let current_auction = use_context::<CurrentAuctionResource>();
    let selected_world_value = selected_world();
    let current = match current_auction.read().clone() {
        Some((loaded_world, result)) if loaded_world == selected_world_value => Some(result),
        _ => None,
    };

    match current {
        None => forum_page_state(t!("forum-loading"), false, None),
        Some(Err(_)) => forum_page_state(
            t!("forum-unavailable-note"),
            true,
            Some(rsx! {
                button {
                    class: "public-state-retry",
                    r#type: "button",
                    onclick: move |_| {
                        refresh_generation.set(refresh_generation().saturating_add(1));
                    },
                    {t!("forum-retry")}
                }
            }),
        ),
        Some(Ok(None)) => rsx! {
            section {
                class: "public-forum-state",
                role: "status",
                aria_live: "polite",
                h2 { {t!("forum-no-auction")} }
                p { {t!("forum-no-auction-note")} }
            }
        },
        Some(Ok(Some(auction))) if auction.presentation_mode != "live" => rsx! {
            article { class: "public-forum-card",
                ForumAuctionHeading { auction: auction.clone() }
                section {
                    class: "public-forum-state",
                    role: "status",
                    aria_live: "polite",
                    h3 { {t!("forum-not-open")} }
                    p { {t!("forum-not-open-note")} }
                }
            }
        },
        Some(Ok(Some(auction))) => {
            let forum_key = format!("{}:{}", auction.world, auction.id);
            // Dioxus 的 key 特殊属性会消费该值，但 WASM release 展开后仍可能产生误报。
            let _ = forum_key.as_str();
            rsx! {
                ForumAuctionPanel { key: "{forum_key}", auction }
            }
        }
    }
}

fn forum_page_state(message: String, alert: bool, action: Option<Element>) -> Element {
    rsx! {
        section {
            class: "public-forum-state",
            role: if alert { "alert" } else { "status" },
            aria_live: "polite",
            p { "{message}" }
            {action}
        }
    }
}

#[component]
fn ForumAuctionPanel(auction: CurrentAuction) -> Element {
    let retry_generation = use_signal(|| 0_u64);
    let poll_generation = use_signal(|| 0_u64);
    let thread = use_resource({
        let auction_id = auction.id.clone();
        move || {
            let _ = retry_generation();
            let _ = poll_generation();
            let auction_id = auction_id.clone();
            async move { fetch_forum_thread(auction_id).await }
        }
    });
    // 每次读取完成后才开始轮询间隔，不能用两秒节拍取消尚未完成的慢请求。
    // 发布前两秒、发布后三十秒；明确失败时暂停，手动重读成功后恢复。
    let _poll = use_resource({
        let mut poll_generation = poll_generation;
        move || {
            let poll_delay = if thread.state().cloned() != UseResourceState::Ready {
                None
            } else {
                let current = thread.read();
                match current.as_ref() {
                    Some(Ok(thread)) if thread.status == "published" => Some(30_000),
                    Some(Ok(thread)) if thread.status == "failed" => None,
                    Some(Ok(_)) => Some(2_000),
                    Some(Err(_)) | None => None,
                }
            };
            async move {
                let Some(poll_delay) = poll_delay else {
                    return;
                };
                TimeoutFuture::new(poll_delay).await;
                let next_generation = poll_generation.peek().saturating_add(1);
                poll_generation.set(next_generation);
            }
        }
    });
    let thread_state = thread.read().clone();
    let content = match thread_state {
        None => forum_page_state(t!("forum-loading"), false, None),
        Some(Err(_)) => {
            let mut retry_generation = retry_generation;
            forum_page_state(
                t!("forum-unavailable-note"),
                true,
                Some(rsx! {
                    button {
                        class: "public-state-retry",
                        r#type: "button",
                        onclick: move |_| {
                            retry_generation.set(retry_generation().saturating_add(1));
                        },
                        {t!("forum-retry")}
                    }
                }),
            )
        }
        Some(Ok(thread)) => forum_thread_details(thread, retry_generation),
    };

    rsx! {
        article { class: "public-forum-card",
            ForumAuctionHeading { auction }
            {content}
        }
    }
}

#[component]
fn ForumAuctionHeading(auction: CurrentAuction) -> Element {
    rsx! {
        header { class: "public-forum-card__header",
            div {
                h2 { {t!("forum-current-auction", number: auction.number)} }
                p { class: "public-forum-card__world",
                    {t!("forum-world")}
                    ": "
                    "{auction_world_label(&auction.world)}"
                }
            }
        }
    }
}

fn forum_thread_details(thread: ForumThread, mut retry_generation: Signal<u64>) -> Element {
    let status_label = forum_status_label(&thread.status);
    let status_message = forum_status_message(&thread.status);
    let status_class = forum_status_class(&thread.status);
    let updated_at = format_auction_time(&thread.updated_at);
    let status_accessible_label = format!("{}: {status_label}", t!("forum-status"));
    let content_synced_at = thread.content_synced_at.as_deref().map(format_auction_time);
    let content_last_success_at =
        thread.content_last_success_at.as_deref().map(format_auction_time);
    let content_error = thread
        .content_last_error_at
        .as_deref()
        .zip(thread.content_last_error_code.as_deref())
        .map(|(at, code)| (at.to_owned(), format_auction_time(at), code.to_owned()));
    let retry_action = (thread.status == "failed").then(|| {
        rsx! {
            button {
                class: "public-state-retry",
                r#type: "button",
                onclick: move |_| {
                    retry_generation.set(retry_generation().saturating_add(1));
                },
                {t!("forum-retry")}
            }
        }
    });
    let published_content = match (
        thread.thread_url.clone(),
        thread.title.clone(),
        thread.body.clone(),
    ) {
        (Some(thread_url), Some(title), Some(body)) if thread.status == "published" => rsx! {
            section { class: "public-forum-thread", aria_labelledby: "public-forum-thread-title",
                div { class: "public-forum-thread__heading",
                    h3 { id: "public-forum-thread-title", "{title}" }
                    a {
                        class: "public-forum-thread__link",
                        href: thread_url,
                        target: "_blank",
                        rel: "noopener noreferrer",
                        {t!("forum-link")}
                    }
                }
                article { class: "public-forum-post",
                    header { class: "public-forum-post__meta",
                        time { datetime: thread.updated_at.clone(), "{updated_at}" }
                        span { class: "public-forum-post__number", "#1" }
                    }
                    div { class: "public-forum-post__grid",
                        aside { class: "public-forum-post__author",
                            strong { {t!("app-title")} }
                            span { {t!("forum-state-published")} }
                        }
                        div { class: "public-forum-post__content",
                            details {
                                class: "public-forum-thread__body",
                                open: true,
                                summary { {t!("forum-thread-body-summary")} }
                                pre { "{body}" }
                            }
                        }
                    }
                }
            }
        },
        _ => rsx! {},
    };

    rsx! {
        div { class: "public-forum-card__content",
            div { class: "public-forum-card__status-line",
                span {
                    class: "public-forum-status public-forum-status--{status_class}",
                    aria_label: status_accessible_label,
                    "{status_label}"
                }
                p { role: "status", aria_live: "polite", "{status_message}" }
                {retry_action}
            }
            if let Some((error_at, formatted_error_at, error_code)) = content_error {
                p { class: "public-forum-delay", role: "alert",
                    {t!("forum-sync-delayed", code: error_code, time: formatted_error_at)}
                    time { class: "public-visually-hidden", datetime: error_at }
                }
            }
            dl { class: "public-forum-metadata",
                div {
                    dt { {t!("forum-source-revision")} }
                    dd { "{thread.source_revision}" }
                }
                div {
                    dt { {t!("forum-content-revision")} }
                    dd { "{thread.content_revision}" }
                }
                if let (Some(raw), Some(formatted)) =
                    (thread.content_synced_at.as_ref(), content_synced_at)
                {
                    div {
                        dt { {t!("forum-content-synced-at")} }
                        dd { time { datetime: raw.clone(), "{formatted}" } }
                    }
                }
                if let (Some(raw), Some(formatted)) =
                    (thread.content_last_success_at.as_ref(), content_last_success_at)
                {
                    div {
                        dt { {t!("forum-last-success-at")} }
                        dd { time { datetime: raw.clone(), "{formatted}" } }
                    }
                }
                div {
                    dt { {t!("forum-progress")} }
                    dd { {t!("forum-updated-through", post_number: thread.processed_through_post_number)} }
                }
                div {
                    dt { {t!("forum-updated-at")} }
                    dd { time { datetime: thread.updated_at.clone(), "{updated_at}" } }
                }
            }
            {published_content}
        }
    }
}

fn forum_status_label(status: &str) -> String {
    match status {
        "pending" => t!("forum-state-pending"),
        "publishing" => t!("forum-state-publishing"),
        "failed" => t!("forum-state-failed"),
        "published" => t!("forum-state-published"),
        _ => t!("forum-unavailable"),
    }
}

fn forum_status_message(status: &str) -> String {
    match status {
        "pending" => t!("forum-syncing-pending"),
        "publishing" => t!("forum-syncing-publishing"),
        "failed" => t!("forum-syncing-failed"),
        "published" => t!("forum-published"),
        _ => t!("forum-unavailable"),
    }
}

fn forum_status_class(status: &str) -> &'static str {
    match status {
        "published" => "published",
        "failed" => "failed",
        "publishing" => "publishing",
        _ => "pending",
    }
}
