//! Public 论坛同步独立页面。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;
use gloo_timers::future::TimeoutFuture;

use crate::api::{CurrentAuction, ForumThread, fetch_forum_thread};

use super::{CurrentAuctionResource, auction_world_label};

/// 展示所选世界当前拍卖的论坛同步投影；首页不再承担该数据源的轮询。
pub(super) fn page() -> Element {
    let selected_world = use_context::<Signal<String>>();
    let mut refresh_generation = use_context::<Signal<u64>>();
    let current_auction = use_context::<CurrentAuctionResource>();
    let selected_world_value = selected_world();
    let current = match current_auction.read().clone() {
        Some((loaded_world, result)) if loaded_world == selected_world_value => Some(result),
        _ => None,
    };

    let content = match current {
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
    // 发布前两秒轮询；发布后降为三十秒读取本地处理楼层。发布或请求明确失败时暂停，
    // 手动重读成功后由 thread 资源变化重新启动计时器。
    let _poll = use_resource({
        let mut poll_generation = poll_generation;
        move || {
            let _ = retry_generation();
            let _ = poll_generation();
            let poll_delay = {
                let current = thread.read();
                match current.as_ref() {
                    Some(Ok(thread)) if thread.status == "published" => Some(30_000),
                    Some(Ok(thread)) if thread.status == "failed" => None,
                    Some(Ok(_)) | None => Some(2_000),
                    Some(Err(_)) => None,
                }
            };
            async move {
                let Some(poll_delay) = poll_delay else {
                    return;
                };
                TimeoutFuture::new(poll_delay).await;
                poll_generation.set(poll_generation().saturating_add(1));
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

fn forum_thread_details(thread: ForumThread, mut retry_generation: Signal<u64>) -> Element {
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
    if let Some(thread_url) = thread.thread_url.filter(|_| thread.status == "published") {
        return rsx! {
            div { class: "public-forum-card__content", role: "status", aria_live: "polite",
                dl { class: "public-forum-metadata",
                    div {
                        dt { {t!("forum-location")} }
                        dd {
                            a {
                                class: "public-forum-thread__link",
                                href: thread_url,
                                target: "_blank",
                                rel: "noopener noreferrer",
                                {t!("forum-link")}
                            }
                        }
                    }
                    div {
                        dt { {t!("forum-progress")} }
                        dd {
                            {t!(
                                "forum-updated-through",
                                post_number: thread.processed_through_post_number
                            )}
                        }
                    }
                }
            }
        };
    }

    forum_page_state(forum_status_message(&thread.status), thread.status == "failed", retry_action)
}

fn forum_status_message(status: &str) -> String {
    match status {
        "pending" => t!("forum-syncing-pending"),
        "publishing" => t!("forum-syncing-publishing"),
        "failed" => t!("forum-syncing-failed"),
        _ => t!("forum-unavailable"),
    }
}
