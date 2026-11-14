//! 玩家支持视图：按用户名/用户 ID 或参考编号只读汇总。
//!
//! 页面只发一次有界只读查询，不提供任何写动作；输入与结果都不缓存到浏览器本地。
//! 检索入口互斥，提交前由 URL 状态与页面共同校验，服务端仍会再次失败关闭。

use auction_web_shared::{AsyncState, AsyncStateKind, t};
use dioxus::prelude::*;

use super::super::query::{SupportQuery, use_support_query_state};
use super::sections;
use crate::api::fetch_player_support;
use crate::surface::admin_api_error_message;

/// 路由入口。
pub(crate) fn page() -> Element {
    // 已应用的检索入口只由地址栏持有：提交 push、前进/后退由 hook 恢复，资源直接订阅它。
    let query = use_support_query_state();
    let query_state = query.state;
    let mut push_query = query;
    let mut user_input = use_signal(|| query_state.peek().user.clone());
    let mut reference_input = use_signal(|| query_state.peek().reference.clone());
    let mut invalid = use_signal(|| false);
    // 输入框是提交前草稿；已应用入口变化（提交或前进/后退）时重置为当前值，与地址栏一致。
    use_effect(move || {
        let applied = query_state.read();
        if *user_input.peek() != applied.user {
            user_input.set(applied.user.clone());
        }
        if *reference_input.peek() != applied.reference {
            reference_input.set(applied.reference.clone());
        }
        if *invalid.peek() {
            invalid.set(false);
        }
    });

    // 只接受恰好一个入口；参考编号必须是规范 UUID，用户 ID 或用户名都交给服务端解析。
    let submit = move |event: FormEvent| {
        event.prevent_default();
        let next = SupportQuery {
            user: user_input().trim().to_owned(),
            reference: reference_input().trim().to_owned(),
        };
        if !next.valid() {
            invalid.set(true);
            return;
        }
        invalid.set(false);
        push_query.push(next);
    };

    let resource = use_resource(move || {
        let current = query_state();
        async move {
            if !current.valid() {
                return None;
            }
            let result =
                fetch_player_support(current.user.clone(), current.reference.clone()).await;
            Some((current, result))
        }
    });
    let loading = resource.state().cloned() != UseResourceState::Ready;
    // 结果必须与当前入口一致；地址栏变化后不展示上一轮汇总。
    let current = query_state();
    let result = if loading {
        None
    } else {
        resource
            .read()
            .as_ref()
            .and_then(|entry| entry.as_ref())
            .filter(|(loaded, _)| *loaded == current)
            .map(|(_, result)| result.clone())
    };

    rsx! {
        section { class: "admin-support-page", aria_labelledby: "support-title",
            h1 { id: "support-title", {t!("page-support-title")} }
            form { class: "admin-delivery-lookup", onsubmit: submit,
                label { r#for: "support-user", {t!("support-user")} }
                input {
                    id: "support-user",
                    r#type: "text",
                    maxlength: "256",
                    value: "{user_input()}",
                    oninput: move |event| user_input.set(event.value()),
                }
                label { r#for: "support-reference", {t!("support-search-reference")} }
                input {
                    id: "support-reference",
                    r#type: "text",
                    maxlength: "36",
                    value: "{reference_input()}",
                    oninput: move |event| reference_input.set(event.value()),
                }
                button { r#type: "submit", {t!("support-search")} }
                if invalid() {
                    p { class: "action-feedback action-feedback--error", role: "alert", {t!("support-invalid-query")} }
                }
            }
            if result.is_none() && loading {
                AsyncState {
                    kind: AsyncStateKind::Loading,
                    title: t!("data-loading"),
                    detail: "",
                }
            } else if result.is_none() {
                p { class: "admin-panel__note", {t!("support-idle")} }
            } else if let Some(Err(error)) = result {
                AsyncState {
                    kind: AsyncStateKind::Error,
                    title: admin_api_error_message(&error),
                    detail: "",
                }
            } else if let Some(Ok(bundle)) = result {
                {sections::summary(&bundle)}
                {sections::bids(bundle.bids.clone())}
                {sections::deals(bundle.deals.clone())}
                {sections::obligations(bundle.obligations.clone())}
                {sections::deliveries(bundle.deliveries.clone())}
                {sections::notifications(bundle.notifications)}
            }
        }
    }
}
