//! 玩家支持视图：按用户名/用户 ID 或参考编号只读汇总。
//!
//! 页面只发一次有界只读查询，不提供任何写动作；输入与结果都不缓存到浏览器本地。
//! 检索入口互斥，提交前由 URL 状态与页面共同校验，服务端仍会再次失败关闭。

use auction_web_shared::{AsyncState, AsyncStateKind, t};
use dioxus::prelude::*;

use super::super::query::{SupportQuery, use_support_query_state};
use super::sections;
use crate::api::{
    MAX_PLAYER_SUPPORT_USER_BYTES, fetch_player_support, player_support_user_too_long,
};
use crate::surface::admin_api_error_message;
use crate::surface::validation::{FieldError, focus_field};

/// 路由入口。
pub(crate) fn page() -> Element {
    // 已应用的检索入口只由地址栏持有：提交 push、前进/后退由 hook 恢复，资源直接订阅它。
    let query = use_support_query_state();
    let query_state = query.state;
    let mut push_query = query;
    let mut user_input = use_signal(|| query_state.peek().user.clone());
    let mut reference_input = use_signal(|| query_state.peek().reference.clone());
    let mut invalid = use_signal(|| false);
    let mut invalid_user = use_signal(|| player_support_user_too_long(&query_state.peek().user));
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
        let user_too_long = player_support_user_too_long(&applied.user);
        if *invalid_user.peek() != user_too_long {
            invalid_user.set(user_too_long);
        }
    });

    // 只接受恰好一个入口；参考编号必须是规范 UUID，用户 ID 或用户名都交给服务端解析。
    let submit = move |event: FormEvent| {
        event.prevent_default();
        let next = SupportQuery {
            user: user_input.peek().trim().to_owned(),
            reference: reference_input.peek().trim().to_owned(),
        };
        let user_too_long = player_support_user_too_long(&next.user);
        invalid_user.set(user_too_long);
        if user_too_long {
            invalid.set(false);
            focus_field("support-user");
            return;
        }
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
    // 结果必须与当前入口一致；地址栏变化后不展示上一轮汇总。
    let current = query_state();
    let address_user_too_long = player_support_user_too_long(&current.user);
    let loading = !address_user_too_long && resource.state().cloned() != UseResourceState::Ready;
    let result = if address_user_too_long || loading {
        None
    } else {
        resource
            .read()
            .as_ref()
            .and_then(|entry| entry.as_ref())
            .filter(|(loaded, _)| *loaded == current)
            .map(|(_, result)| result.clone())
    };
    let show_user_error = invalid_user();

    rsx! {
        section { class: "admin-support-page", aria_labelledby: "support-title",
            h1 { id: "support-title", {t!("page-support-title")} }
            form { class: "admin-delivery-lookup", onsubmit: submit,
                label { r#for: "support-user", {t!("support-user")} }
                input {
                    id: "support-user",
                    r#type: "text",
                    aria_invalid: show_user_error.to_string(),
                    aria_describedby: show_user_error.then_some("support-user-error"),
                    value: "{user_input()}",
                    oninput: move |event| {
                        let value = event.value();
                        if *invalid_user.peek() && !player_support_user_too_long(value.trim()) {
                            invalid_user.set(false);
                        }
                        user_input.set(value);
                    },
                }
                FieldError {
                    id: "support-user-error",
                    message: show_user_error.then(|| t!("admin-field-max-utf8-bytes", max: MAX_PLAYER_SUPPORT_USER_BYTES)),
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
            } else if result.is_none() && !show_user_error && !address_user_too_long {
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

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::future::Future;
    use std::pin::pin;
    use std::rc::Rc;
    use std::task::{Context, Poll, Waker};

    use auction_web_shared::query::{QueryLocation, QueryWriteMode, use_query_router};
    use dioxus::prelude::{ReadableExt, Signal, WritableExt, use_context_provider, use_signal};

    use super::page;
    use crate::api::fetch_player_support;
    use crate::surface::auctions::draft_page_tests::{
        assert_field, has_live_text, mount_writer, settle_writer, writer_form,
    };
    use crate::surface::query::SupportQuery;
    use crate::surface::realtime::text_attributes;
    use crate::surface::test_events::dispatch_form_event;

    const ERROR: &str = "请输入不超过 256 个 UTF-8 字节的内容。";
    const REFERENCE: &str = "0b5f1c2e-8a7d-4c3b-9e1f-2a3b4c5d6e7f";

    #[derive(Default)]
    struct DraftAddress(RefCell<Vec<(String, QueryWriteMode)>>);

    impl QueryLocation for DraftAddress {
        fn current(&self) -> String {
            self.0
                .borrow()
                .last()
                .map_or_else(|| "/player-support".to_owned(), |(location, _)| location.clone())
        }

        fn write(&self, location: &str, mode: QueryWriteMode) -> bool {
            self.0.borrow_mut().push((location.to_owned(), mode));
            true
        }
    }

    #[test]
    fn support_rejects_oversized_drafts_and_clears_error_at_256_utf8_bytes() {
        let address = Rc::new(DraftAddress::default());
        let location: Rc<dyn QueryLocation> = address.clone();
        let (mut mounted, edits) = mount_writer(move || {
            use_context_provider(|| location.clone());
            page()
        });
        let form = writer_form(&edits);
        let field = text_attributes(&edits, "aria-invalid").next().unwrap().0;
        let reference = text_attributes(&edits, "value")
            .find_map(|(id, _)| (id != field).then_some(id))
            .expect("参考编号输入框");
        let limit = format!("{}a", "漢".repeat(85));
        for (index, oversized) in [format!("{limit}b"), "漢".repeat(86)].into_iter().enumerate() {
            mounted.dispatch("input", field, &oversized);
            let changes = mounted.dispatch("submit", form, "");
            if index == 0 {
                assert_eq!(
                    text_attributes(&changes, "aria-describedby").collect::<Vec<_>>(),
                    [(field, "support-user-error")]
                );
            }
            assert_field(&mounted, field, &oversized, false);
            assert!(has_live_text(&mounted, ERROR));
            assert!(address.0.borrow().is_empty(), "超长提交不得写 URL");
        }
        mounted.dispatch("input", field, &format!(" {limit} "));
        assert!(!has_live_text(&mounted, ERROR));
        // 提交同步写入地址；宿主测试在资源轮询浏览器 GET 前检查地址替身。
        dispatch_form_event(mounted.dom(), form, "submit", "");
        assert_eq!(address.0.borrow().len(), 1, "合法用户名应写入地址");

        dispatch_form_event(mounted.dom(), field, "input", "");
        dispatch_form_event(mounted.dom(), reference, "input", REFERENCE);
        dispatch_form_event(mounted.dom(), form, "submit", "");
        let writes = address.0.borrow();
        assert_eq!(writes.len(), 2, "合法参考编号应独立写入地址");
        for ((location, mode), expected) in
            writes.iter().zip([("user", limit.as_str()), ("reference", REFERENCE)])
        {
            assert_eq!(*mode, QueryWriteMode::Push);
            let url = url::Url::parse(&format!("https://auction.test{location}")).unwrap();
            assert_eq!(url.path(), "/player-support");
            let pairs: Vec<_> = url.query_pairs().collect();
            assert_eq!(pairs, [(expected.0.into(), expected.1.into())]);
        }
    }

    #[test]
    fn support_restores_oversized_bookmarks_and_history_with_a_field_error() {
        let first = "漢".repeat(86);
        let second = format!("{}ab", "漢".repeat(85));
        let url = |user: &str, reference: &str| {
            let query = url::form_urlencoded::Serializer::new(String::new())
                .append_pair("user", user)
                .append_pair("reference", reference)
                .finish();
            format!("/player-support?{query}")
        };
        let initial = url(&first, REFERENCE);
        let route = Rc::new(Cell::new(None::<Signal<String>>));
        let (mut mounted, edits) = mount_writer({
            let route = route.clone();
            move || {
                let address = use_signal(|| initial.clone());
                route.set(Some(address));
                use_query_router(move || address.read().clone());
                page()
            }
        });
        let field = text_attributes(&edits, "aria-invalid").next().unwrap().0;
        assert_field(&mounted, field, &first, false);
        assert!(has_live_text(&mounted, ERROR));

        let mut address = route.get().unwrap();
        for (next, expected) in [
            ("/player-support".to_owned(), ""),
            (url(&second, ""), second.as_str()),
            (url(&first, REFERENCE), first.as_str()),
        ] {
            address.set(next);
            settle_writer(&mut mounted);
            assert_field(&mounted, field, expected, false);
            assert_eq!(has_live_text(&mounted, ERROR), !expected.is_empty());
        }
    }

    #[test]
    fn support_client_rejects_oversized_terms_before_fetch() {
        let limit = format!("{}a", "漢".repeat(85));
        for oversized in [format!("{limit}b"), "漢".repeat(86)] {
            assert!(!SupportQuery { user: oversized.clone(), reference: String::new() }.valid());
            for reference in ["", REFERENCE] {
                let mut fetch =
                    pin!(fetch_player_support(format!(" {oversized} "), reference.to_owned(),));
                assert!(matches!(
                    fetch.as_mut().poll(&mut Context::from_waker(Waker::noop())),
                    Poll::Ready(Err(code)) if code == "invalid_player_support_query"
                ));
            }
        }
    }
}
