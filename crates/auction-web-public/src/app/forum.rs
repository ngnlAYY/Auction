//! Public 拍卖论坛镜像展示。

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api::{ForumThread, fetch_forum_thread};

pub(super) fn panel(auction_id: String) -> Element {
    let thread = use_resource({
        let auction_id = auction_id.clone();
        move || {
            let auction_id = auction_id.clone();
            async move { fetch_forum_thread(auction_id).await }
        }
    });
    match &*thread.read_unchecked() {
        None => rsx! {
            p { class: "public-auction-page__status", role: "status", {t!("data-loading")} }
        },
        Some(Err(_)) => rsx! {
            p { class: "public-auction-page__status", role: "status", {t!("data-error")} }
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
                {t!("forum-updated", updated_at: thread.updated_at)}
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
