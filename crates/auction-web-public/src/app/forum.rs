//! Public 拍卖论坛镜像展示。

use dioxus::prelude::*;
use dioxus_i18n::t;
use gloo_timers::future::TimeoutFuture;

use crate::api::{ForumThread, fetch_forum_thread};

use super::{RetryPlaceholder, format_auction_time};

pub(super) fn panel(auction_id: String) -> Element {
    let refresh_generation = use_context::<Signal<u64>>();
    let poll_generation = use_signal(|| 0_u64);
    let thread = use_resource({
        let auction_id = auction_id.clone();
        move || {
            let _ = refresh_generation();
            let _ = poll_generation();
            let auction_id = auction_id.clone();
            async move { fetch_forum_thread(auction_id).await }
        }
    });
    // 旧版在论坛主题尚未发布时每两秒重读一次；发布成功后停止，避免无意义请求。
    let _poll = use_future({
        let mut poll_generation = poll_generation;
        move || async move {
            loop {
                TimeoutFuture::new(2_000).await;
                let published = {
                    let current = thread.read_unchecked();
                    current
                        .as_ref()
                        .and_then(|result| result.as_ref().ok())
                        .is_some_and(|thread| thread.status == "published")
                };
                if published {
                    break;
                }
                poll_generation.set(poll_generation().saturating_add(1));
            }
        }
    });
    match &*thread.read_unchecked() {
        None => rsx! {
            p { class: "public-auction-page__status", role: "status", {t!("data-loading")} }
        },
        Some(Err(_)) => rsx! {
            RetryPlaceholder {
                title: t!("forum-title"),
                detail: t!("data-error"),
                refresh_generation,
            }
        },
        Some(Ok(thread)) => rsx! {
            ForumThreadPanel { thread: thread.clone() }
        },
    }
}

#[component]
fn ForumThreadPanel(thread: ForumThread) -> Element {
    rsx! {
        section { class: "public-auction-category", aria_labelledby: "forum-title",
            h2 { id: "forum-title", class: "public-auction-category__title",
                {t!("forum-title")}
            }
            p { class: "public-auction-page__status",
                {t!("forum-status", status: thread.status, revision: thread.source_revision)}
            }
            p { class: "public-auction-page__status",
                {t!("forum-updated", updated_at: format_auction_time(&thread.updated_at))}
            }
            if let Some(title) = thread.title {
                h3 { "{title}" }
            }
            if let Some(url) = thread.thread_url {
                p {
                    a { href: url, target: "_blank", rel: "noreferrer", {t!("forum-link")} }
                }
            }
            if let Some(body) = thread.body {
                pre { class: "public-auction-page__document", "{body}" }
            }
        }
    }
}
