//! Public 拍卖论坛镜像展示。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;
use gloo_timers::future::TimeoutFuture;

use crate::api::{ForumThread, fetch_forum_thread};

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
            p { class: "public-page-status", role: "status", {t!("forum-loading")} }
        },
        Some(Err(_)) => rsx! {
            p { class: "public-page-status", role: "status", {t!("forum-unavailable")} }
        },
        Some(Ok(thread)) => rsx! {
            ForumThreadPanel { thread: thread.clone() }
        },
    }
}

#[component]
fn ForumThreadPanel(thread: ForumThread) -> Element {
    if thread.status != "published" {
        let message = match thread.status.as_str() {
            "pending" => t!("forum-syncing-pending"),
            "publishing" => t!("forum-syncing-publishing"),
            "failed" => t!("forum-syncing-failed"),
            _ => t!("forum-unavailable"),
        };
        return rsx! {
            p { class: "public-page-status", role: "status",
                "{message}"
            }
        };
    }
    rsx! {
        p { class: "public-page-status", role: "status", aria_live: "polite",
            {t!("forum-published")}
            " "
            if let Some(url) = thread.thread_url {
                a { href: url, target: "_blank", rel: "noreferrer", {t!("forum-link")} }
            }
        }
    }
}
