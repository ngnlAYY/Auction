//! Public 论坛同步摘要；正文和诊断细节由 Admin 展示。

use auction_web_shared::t;
use dioxus::prelude::*;

use crate::api::{CurrentAuction, ForumThread, fetch_forum_sync_enabled, fetch_forum_thread};

use super::{
    CurrentAuctionResource, PageVisibility, format_auction_time, sleep_while_visible,
    use_content_loading, world_label,
};

/// 先读取独立的开关投影，明确启用后才挂载当前拍卖的论坛进度。
pub(super) fn page() -> Element {
    let mut status_generation = use_signal(|| 0_u64);
    let refresh_generation = use_context::<Signal<u64>>();
    let visibility = use_context::<PageVisibility>();
    let sync_enabled = use_resource(move || {
        let _ = status_generation();
        let _ = refresh_generation();
        async move { fetch_forum_sync_enabled().await }
    });
    // 关闭和短暂不可用时也定期重读，以便 Worker 重启后自动恢复，不能继续显示旧楼层。
    let _poll = use_resource(move || {
        let _ = status_generation();
        let visibility = visibility.clone();
        async move {
            sleep_while_visible(&visibility, 30_000).await;
            status_generation.set(status_generation().saturating_add(1));
        }
    });
    let sync_snapshot = sync_enabled.read();
    use_content_loading(sync_snapshot.is_none());
    let content = match &*sync_snapshot {
        None => rsx! {},
        Some(Ok(false)) => forum_page_state(t!("forum-sync-disabled"), false),
        Some(Ok(true)) => rsx! { ForumCurrentAuction {} },
        Some(Err(_)) => forum_page_state(t!("forum-unavailable"), true),
    };

    rsx! {
        section { class: "public-forum-page", aria_label: t!("page-forum-title"),
            {content}
        }
    }
}

#[component]
fn ForumCurrentAuction() -> Element {
    let selected_world = use_context::<Signal<String>>();
    let current_auction = use_context::<CurrentAuctionResource>();
    let selected_world_value = selected_world();
    let loaded = current_auction.read();
    let current = loaded
        .as_ref()
        .filter(|(loaded_world, _)| loaded_world == &selected_world_value)
        .map(|(_, result)| result);

    use_content_loading(current.is_none());
    match current {
        None => rsx! {},
        Some(Err(_)) => forum_page_state(t!("forum-unavailable-note"), true),
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
        Some(Ok(Some(auction))) => rsx! {
            ForumAuctionPanel { auction: auction.clone() }
        },
    }
}

fn forum_page_state(message: String, alert: bool) -> Element {
    rsx! {
        section {
            class: "public-forum-state",
            role: if alert { "alert" } else { "status" },
            aria_live: "polite",
            p { "{message}" }
        }
    }
}

#[component]
fn ForumAuctionPanel(auction: CurrentAuction) -> Element {
    let mut poll_generation = use_signal(|| 0_u64);
    let visibility = use_context::<PageVisibility>();
    // 此组件只拥有读取和轮询任务；拍卖 UUID 是显式依赖，不用静态 key 假装重挂载。
    let thread = use_resource(use_reactive((&auction.id,), move |(auction_id,)| {
        let _ = poll_generation();
        async move {
            let result = fetch_forum_thread(auction_id.clone()).await;
            (auction_id, result)
        }
    }));
    // 每次读取完成后才开始轮询间隔，不能用两秒节拍取消尚未完成的慢请求。
    // 发布前两秒、发布后三十秒；明确失败时暂停，重新进入页面后重读。手动刷新入口只在 Admin。
    let _poll = use_resource(use_reactive((&auction.id,), move |(auction_id,)| {
        let visibility = visibility.clone();
        let poll_delay = if thread.state().cloned() != UseResourceState::Ready {
            None
        } else {
            let current = thread.read();
            match current.as_ref().filter(|(loaded_id, _)| loaded_id == &auction_id) {
                Some((_, Ok(thread))) if thread.status == "published" => Some(30_000),
                Some((_, Ok(thread))) if thread.status == "failed" => None,
                Some((_, Ok(_))) => Some(2_000),
                Some((_, Err(_))) | None => None,
            }
        };
        async move {
            let Some(poll_delay) = poll_delay else {
                return;
            };
            sleep_while_visible(&visibility, poll_delay).await;
            let next_generation = poll_generation.peek().saturating_add(1);
            poll_generation.set(next_generation);
        }
    }));
    // Resource 会保留旧结果；切换场次后不能把旧成功或错误显示在新标题下面。
    let thread_state = thread
        .read()
        .as_ref()
        .filter(|(loaded_id, _)| loaded_id == &auction.id)
        .map(|(_, result)| result.clone());
    use_content_loading(thread_state.is_none());
    let content = match thread_state {
        None => rsx! {},
        Some(Err(_)) => forum_page_state(t!("forum-unavailable-note"), true),
        Some(Ok(thread)) => forum_thread_details(thread),
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
                    "{world_label(&auction.world)}"
                }
            }
        }
    }
}

fn forum_thread_details(thread: ForumThread) -> Element {
    let status_label = forum_status_label(&thread.status);
    let status_message = forum_status_message(&thread.status);
    let status_class = forum_status_class(&thread.status);
    let updated_at = format_auction_time(&thread.updated_at);
    let status_accessible_label = format!("{}: {status_label}", t!("forum-status"));

    rsx! {
        div { class: "public-forum-card__content",
            div { class: "public-forum-card__status-line",
                span {
                    class: "public-forum-status public-forum-status--{status_class}",
                    aria_label: status_accessible_label,
                    "{status_label}"
                }
                p { role: "status", aria_live: "polite", "{status_message}" }
                if thread.status == "published" {
                    if let Some(thread_url) = thread.thread_url {
                        a {
                            class: "public-forum-card__link",
                            href: thread_url,
                            target: "_blank",
                            rel: "noopener noreferrer",
                            {t!("forum-link")}
                        }
                    }
                }
            }
            if thread.content_last_error_code.is_some() {
                p { class: "public-forum-delay", role: "alert",
                    {t!("forum-sync-delayed")}
                }
            }
            dl { class: "public-forum-metadata",
                div {
                    dt { {t!("forum-progress")} }
                    dd { {t!("forum-updated-through", post_number: thread.processed_through_post_number)} }
                }
                div {
                    dt { {t!("forum-updated-at")} }
                    dd { time { datetime: thread.updated_at, "{updated_at}" } }
                }
            }
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
