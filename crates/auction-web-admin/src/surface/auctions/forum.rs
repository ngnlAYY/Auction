//! 拍卖管理中的论坛状态与正文诊断；只读 GET，不代替人工写入恢复授权。

use auction_contracts::PublicForumResponse;
use auction_web_shared::{Time, t};
use dioxus::prelude::*;

use super::super::admin_api_error_message;
use super::presentation::admin_auction_world_label;
use crate::api::{AdminAuction, AdminForumStatus, fetch_admin_forum_status};

/// 从当前列表页选择拍卖，保留列表筛选和表格列宽。
#[component]
pub(super) fn AdminAuctionForumStatus(auctions: Vec<AdminAuction>) -> Element {
    let mut selected = use_signal(String::new);
    let selected_id = selected();
    let Some(auction) =
        auctions.iter().find(|auction| auction.id == selected_id).or_else(|| auctions.first())
    else {
        return rsx! {};
    };
    let auction_id = auction.id.clone();
    let readout_auction_id = auction_id.clone();
    rsx! {
        section { class: "admin-forum-status", aria_labelledby: "admin-forum-title",
            h2 { id: "admin-forum-title", {t!("admin-forum-title")} }
            p { {t!("admin-forum-readonly-note")} }
            label { r#for: "admin-forum-auction", {t!("admin-forum-auction")} }
            select {
                id: "admin-forum-auction",
                value: auction_id,
                onchange: move |event| selected.set(event.value()),
                for auction in auctions {
                    option { value: auction.id,
                        {t!("admin-auctions-row-heading", number: auction.sequence, world: admin_auction_world_label(&auction.world_id))}
                    }
                }
            }
            ForumStatusReadout { auction_id: readout_auction_id }
        }
    }
}

#[component]
fn ForumStatusReadout(auction_id: String) -> Element {
    let mut generation = use_signal(|| 0_u64);
    // 普通 prop 不会自动重启资源；显式订阅拍卖身份，不依赖静态组件的 key。
    let status = use_resource(use_reactive((&auction_id,), move |(auction_id,)| {
        let _ = generation();
        async move {
            let result = fetch_admin_forum_status(auction_id.clone()).await;
            (auction_id, result)
        }
    }));
    let busy = status.state().cloned() != UseResourceState::Ready;
    // 正在刷新或切换拍卖时不显示上一轮结果，避免把旧观测展示在新拍卖下。
    let result = if busy {
        None
    } else {
        status
            .read()
            .as_ref()
            .filter(|(loaded_auction, _)| loaded_auction == &auction_id)
            .map(|(_, result)| result.clone())
    };
    rsx! {
        button {
            r#type: "button",
            disabled: busy,
            onclick: move |_| generation.set(generation().saturating_add(1)),
            {t!("admin-forum-refresh")}
        }
        match result {
            None => rsx! { p { {t!("admin-forum-loading")} } },
            Some(Err(error)) => rsx! { p { role: "alert", {admin_api_error_message(&error)} } },
            Some(Ok(status)) => forum_details(status),
        }
    }
}

fn forum_details(status: AdminForumStatus) -> Element {
    if !status.enabled {
        return rsx! { p { role: "status", {t!("admin-forum-disabled")} } };
    }
    let Some(thread) = status.thread else {
        return rsx! { p { role: "status", {t!("admin-forum-missing")} } };
    };
    let status_label = match thread.status.as_str() {
        "pending" => t!("admin-forum-pending"),
        "publishing" => t!("admin-forum-publishing"),
        "published" => t!("admin-forum-published"),
        _ => t!("admin-forum-failed"),
    };
    let content_details = forum_content_details(&thread);
    rsx! {
        p { role: "status", {t!("admin-forum-status", status: status_label)} }
        dl {
            div {
                dt { {t!("admin-forum-progress")} }
                dd { "#{thread.processed_through_post_number}" }
            }
            div {
                dt { {t!("admin-forum-updated-at")} }
                dd { Time { value: thread.updated_at } }
            }
        }
        if let Some(url) = thread.thread_url {
            a { href: url, target: "_blank", rel: "noopener noreferrer", {t!("admin-forum-link")} }
        }
        {content_details}
    }
}

/// 正文只是本地目标镜像；使用原生折叠和转义文本，不将 BBCode 当作 HTML 执行。
fn forum_content_details(thread: &PublicForumResponse) -> Element {
    rsx! {
        section { class: "admin-forum-content", aria_labelledby: "admin-forum-content-title",
            h3 { id: "admin-forum-content-title", {t!("admin-forum-content-title")} }
            dl {
                div {
                    dt { {t!("admin-forum-source-revision")} }
                    dd { "{thread.source_revision}" }
                }
                div {
                    dt { {t!("admin-forum-content-revision")} }
                    dd { "{thread.content_revision}" }
                }
                if let Some(at) = &thread.content_synced_at {
                    div {
                        dt { {t!("admin-forum-content-synced-at")} }
                        dd { Time { value: at.clone() } }
                    }
                }
                if let Some(at) = &thread.content_last_success_at {
                    div {
                        dt { {t!("admin-forum-last-success-at")} }
                        dd { Time { value: at.clone() } }
                    }
                }
                if let Some(at) = &thread.content_last_error_at {
                    div {
                        dt { {t!("admin-forum-error-at")} }
                        dd { Time { value: at.clone() } }
                    }
                }
            }
            if let Some(code) = &thread.content_last_error_code {
                p { class: "admin-forum-content__error", role: "alert",
                    {t!("admin-forum-content-error", code: code.clone())}
                }
            }
            if thread.status == "published" {
                if let (Some(title), Some(body)) = (&thread.title, &thread.body) {
                    h4 { "{title}" }
                    p { class: "admin-forum-content__note", {t!("admin-forum-body-note")} }
                    details { class: "admin-forum-content__body",
                        summary { {t!("admin-forum-body-summary")} }
                        pre { tabindex: "0", aria_label: t!("admin-forum-body-summary"),
                            code { "{body}" }
                        }
                    }
                }
            }
        }
    }
}
