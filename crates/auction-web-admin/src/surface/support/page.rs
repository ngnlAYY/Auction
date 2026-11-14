//! 玩家支持视图：按用户名/用户 ID 或参考编号只读汇总。
//!
//! 页面只发一次有界只读查询，不提供任何写动作；输入与结果都不缓存到浏览器本地。
//! 检索入口互斥，提交前由 URL 状态与页面共同校验，服务端仍会再次失败关闭。

use auction_web_shared::{AsyncState, AsyncStateKind, legacy_t as t};
use dioxus::prelude::*;

use crate::api::{fetch_player_support, valid_uuid};
use crate::surface::admin_api_error_message;

use super::super::query::use_support_query_state;
use super::sections;

/// 路由入口。
pub(crate) fn page() -> Element {
    let query = use_support_query_state();
    let query_state = query.state;
    let mut push_query = query;
    let mut user_input = use_signal(|| query_state().user);
    let mut reference_input = use_signal(|| query_state().reference);
    let mut invalid = use_signal(|| false);
    // `Signal` 本身不是 `Fn`，保留显式闭包以复制当前查询快照。
    #[allow(clippy::redundant_closure)]
    let mut lookup = use_signal(|| query_state());

    // 只接受恰好一个入口；参考编号必须是规范 UUID，用户 ID 或用户名都交给服务端解析。
    let submit = move |event: FormEvent| {
        event.prevent_default();
        let user = user_input().trim().to_owned();
        let reference = reference_input().trim().to_owned();
        let valid = match (user.is_empty(), reference.is_empty()) {
            (false, true) => true,
            (true, false) => valid_uuid(&reference),
            _ => false,
        };
        if !valid {
            invalid.set(true);
            return;
        }
        invalid.set(false);
        let next = super::super::query::SupportQuery { user, reference };
        lookup.set(next.clone());
        push_query.push(next);
    };

    let resource = use_resource(move || {
        let current = lookup();
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
    // 结果必须与当前入口一致；地址栏或表单变化后不展示上一轮汇总。
    let current = lookup();
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
                    p { class: "admin-delivery-error", role: "alert", {t!("support-invalid-query")} }
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
