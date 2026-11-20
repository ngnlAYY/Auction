//! Admin 管理总览页面。
//! 旧版 Dashboard 只读取一次聚合快照；各项指标、邮件控制和局部降级都从该快照派生，不能再
//! 额外请求列表接口拼装第二套页面状态。

use auction_web_shared::{AsyncState, AsyncStateKind, t, use_app_boot_ready};
use dioxus::prelude::*;

use super::pending::{PendingCommandPanel, use_pending_commands};
use super::poll::{AutoPollControls, use_auto_poll};
use super::query::use_dashboard_query_state;
use super::{AdminReadinessResource, admin_api_error_message};
use crate::api::fetch_dashboard;

mod content;
mod controls;

use content::DashboardContent;
use controls::MailPollerControls;

/// 按旧版单快照请求模型加载管理总览。
pub(super) fn page() -> Element {
    let mut refresh = use_signal(|| 0_u64);
    let pending = use_pending_commands(refresh, |entity| {
        entity == "mail-poll-trigger" || entity.starts_with(super::mail::MAILBOX_ENTITY_PREFIX)
    });
    let dashboard = use_resource(move || {
        let generation = refresh();
        async move {
            #[cfg(test)]
            if let Some(reads) = try_consume_context::<tests::DashboardReads>() {
                return (generation, reads.fetch().await);
            }
            (generation, fetch_dashboard().await)
        }
    });
    // 自动轮询开关与间隔入 URL（`poll` 键），默认 30 秒；页面隐藏时轮询挂起。
    // 轮询只重取当前快照，不额外写地址栏。
    let query = use_dashboard_query_state();
    let query_state = query.state;
    let poll_seconds = use_memo(move || query_state().poll_seconds);
    use_auto_poll(poll_seconds, move || {
        // 手动刷新或命令刚触发的 GET 仍在进行时，轮询不能取消它并重新计时。
        if dashboard.state().cloned() == UseResourceState::Ready {
            refresh.set(refresh().saturating_add(1));
        }
    });
    let mut poll_query = query;
    let readiness = use_context::<AdminReadinessResource>();
    let manual_refresh = use_manual_refresh(refresh, readiness);
    // 默认管理首页的聚合快照无论成功或失败，只要已有结果就一次性揭示真实页面。
    use_app_boot_ready(readiness.read().is_some() && dashboard.read().is_some());
    let state = dashboard.read_unchecked();

    rsx! {
        section {
            class: "admin-dashboard-page",
            aria_labelledby: "dashboard-title",
            div { class: "admin-dashboard-heading",
                h1 { id: "dashboard-title", {t!("page-dashboard-title")} }
                div { class: "admin-dashboard-heading__actions",
                    AutoPollControls {
                        poll_seconds: poll_seconds(),
                        on_change: move |seconds| {
                            // 开关与间隔变更立即生效并写回地址栏。
                            let mut next = query_state();
                            next.poll_seconds = seconds;
                            poll_query.push(next);
                        },
                    }
                    button {
                        class: "admin-btn-secondary",
                        r#type: "button",
                        onclick: move |_| manual_refresh.call(()),
                        {t!("dashboard-refresh")}
                    }
                }
            }
            PendingCommandPanel { owner: pending }
            if state.is_none() {
                AsyncState {
                    kind: AsyncStateKind::Loading,
                    title: t!("dashboard-legacy-loading"),
                    detail: "",
                }
            }
            if let Some((_, Err(error))) = state.as_ref() {
                AsyncState {
                    kind: AsyncStateKind::Error,
                    title: admin_api_error_message(error),
                    detail: "",
                    retry_label: t!("admin-retry"),
                    on_retry: move |_| manual_refresh.call(()),
                }
            }
            if let Some((loaded_generation, Ok(snapshot))) = state.as_ref() {
                DashboardContent { snapshot: snapshot.clone(),
                    MailPollerControls {
                        pollers: snapshot
                            .mail
                            .data
                            .as_ref()
                            .map_or_default(|data| data.pollers.clone()),
                        refresh,
                        loaded_generation: *loaded_generation,
                    }
                }
            }
        }
    }
}

fn use_manual_refresh(
    mut refresh: Signal<u64>,
    mut readiness: AdminReadinessResource,
) -> Callback<()> {
    use_callback(move |()| {
        let next = refresh.peek().saturating_add(1);
        refresh.set(next);
        // 用外壳注入的同一份就绪资源复核；正在进行的检查继续完成，不重启请求。
        if readiness.state().cloned() != UseResourceState::Pending {
            readiness.restart();
        }
    })
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use auction_web_shared::t;
    use dioxus::core::{AttributeValue, Mutation, ScopeId};
    use dioxus::prelude::{
        Callback, Element, ReadableExt, Routable, Router, Signal, VNode, VirtualDom, WritableExt,
        component, consume_context, dioxus_core, dioxus_router, dioxus_signals, rsx, use_context,
        use_context_provider, use_hook, use_resource, use_signal,
    };
    use gloo_net::http::Method;

    use super::{page, use_manual_refresh};
    use crate::api::{AdminApiFailure, AdminDashboardSnapshot, AdminMailPolling};
    use crate::surface::AdminReadinessResource;
    use crate::surface::auctions::draft_page_tests::{
        RecordRead, button, click_button, has_live_text, init_i18n, mount_writer_at, settle_writer,
    };
    use crate::surface::pending::{PendingCommandHost, PendingCommandNotices};
    use crate::surface::realtime::TestPage;

    pub(super) type DashboardReads = Rc<RecordRead<AdminDashboardSnapshot>>;

    #[derive(Clone, Debug, PartialEq, Routable)]
    enum TestRoute {
        #[route("/")]
        Fixture {},
    }

    #[component]
    fn Fixture() -> Element {
        page()
    }

    type RefreshHandles = (Signal<u64>, AdminReadinessResource, Callback<()>);

    #[derive(Default)]
    struct RefreshControl {
        ready: Cell<bool>,
        handles: Cell<Option<RefreshHandles>>,
    }

    fn refresh_harness() -> Element {
        let control = consume_context::<Rc<RefreshControl>>();
        let probe = control.clone();
        let readiness = use_resource(move || {
            let ready = probe.ready.get();
            async move { Ok(ready) }
        });
        let refresh = use_signal(|| 0_u64);
        let manual_refresh = use_manual_refresh(refresh, readiness);
        control.handles.set(Some((refresh, readiness, manual_refresh)));
        rsx! { "{refresh()}" }
    }

    #[test]
    fn manual_refresh_rechecks_the_shared_readiness_while_polling_only_reloads_the_snapshot() {
        let control = Rc::new(RefreshControl::default());
        control.ready.set(true);
        let mut dom = VirtualDom::new(refresh_harness).with_root_context(control.clone());
        dom.rebuild_in_place();
        dom.render_immediate_to_vec();
        let (mut refresh, readiness, manual) = control.handles.get().unwrap();
        assert_eq!(readiness.peek().as_ref(), Some(&Ok(true)));

        control.ready.set(false);
        dom.in_runtime(|| refresh.set(1));
        dom.render_immediate_to_vec();
        assert_eq!(readiness.peek().as_ref(), Some(&Ok(true)));

        manual.call(());
        dom.render_immediate_to_vec();
        assert_eq!(*refresh.peek(), 2);
        assert_eq!(readiness.peek().as_ref(), Some(&Ok(false)));
    }

    fn polling_fixture(paused: bool) -> AdminMailPolling {
        AdminMailPolling {
            mailbox_key: "main".to_owned(),
            paused,
            state_version: 1,
            consecutive_failures: 0,
            last_poll_at: "2026-11-18T00:00:00Z".to_owned(),
            next_poll_at: "2026-11-18T00:01:00Z".to_owned(),
            updated_at: "2026-11-18T00:00:00Z".to_owned(),
            diagnostics: None,
        }
    }

    fn dashboard(polling: &AdminMailPolling) -> AdminDashboardSnapshot {
        AdminDashboardSnapshot::test_fixture(|snapshot| {
            snapshot["mail"] = serde_json::json!({
                "status": "ok",
                "observed_at": polling.updated_at,
                "data": { "observed_at": polling.updated_at, "pollers": [polling] },
            });
        })
    }

    fn click_and_check_disabled(dom: &mut TestPage, label: &str) {
        // 登记后未决面板的同名工具提示也带点击处理器；沿用点击前控件的节点 ID。
        let (id, _) = button(dom, label).expect("操作控件存在");
        let mut edits = click_button(dom, label);
        edits.extend(settle_writer(dom));
        let observed = edits.iter().rev().find_map(|edit| match edit {
            Mutation::SetAttribute {
                id: target,
                name: "disabled",
                value: AttributeValue::Bool(disabled),
                ..
            } if *target == id => Some(*disabled),
            _ => None,
        });
        assert_eq!(observed, Some(true), "按钮 {label} 应禁用");
    }

    #[test]
    fn dashboard_pause_resume_refresh_only_for_a_complete_write_409() {
        for (status, paused, reads_after_write) in [(409, false, 2), (503, true, 1)] {
            let reads: DashboardReads = Rc::new(RecordRead::new());
            let (mut dom, _) = mount_writer_at("poll=0", {
                let reads = reads.clone();
                move || {
                    init_i18n();
                    use_context_provider(|| reads.clone());
                    use_hook(move || {
                        AdminApiFailure::stub_write_response(
                            "/api/v1/admin/mail/polling",
                            Method::PUT,
                            status,
                            r#"{"code":"#,
                        );
                    });
                    let readiness: AdminReadinessResource = use_resource(|| async { Ok(true) });
                    use_context_provider(|| readiness);
                    let host = use_context::<PendingCommandHost>();
                    rsx! {
                        PendingCommandNotices { host }
                        Router::<TestRoute> {}
                    }
                }
            });
            reads.respond(Ok(dashboard(&polling_fixture(paused))));
            settle_writer(&mut dom);
            let action =
                if paused { "恢复定时轮询 (main)" } else { "暂停定时轮询 (main)" };
            click_and_check_disabled(&mut dom, action);
            assert_eq!(reads.started(), reads_after_write);
            assert!(button(&dom, "原样重试").is_some());
            assert!(button(&dom, "关闭回执").is_none());
            assert!(!has_live_text(&dom, "轮询状态已更新。"));
        }
    }

    #[test]
    fn dashboard_poll_trigger_keeps_unknown_on_unavailable_write() {
        let reads: DashboardReads = Rc::new(RecordRead::new());
        let (mut dom, _) = mount_writer_at("poll=0", {
            let reads = reads.clone();
            move || {
                init_i18n();
                use_context_provider(|| reads.clone());
                use_hook(|| {
                    AdminApiFailure::stub_write_response(
                        "/api/v1/admin/mail/polls:trigger",
                        Method::POST,
                        503,
                        r#"{"code":"storage_unavailable","message":"暂不可用","data":null,"request_id":null}"#,
                    );
                });
                let readiness: AdminReadinessResource = use_resource(|| async { Ok(true) });
                use_context_provider(|| readiness);
                let host = use_context::<PendingCommandHost>();
                rsx! {
                    PendingCommandNotices { host }
                    Router::<TestRoute> {}
                }
            }
        });
        reads.respond(Ok(dashboard(&polling_fixture(false))));
        settle_writer(&mut dom);
        let action = "手动触发收件轮询";
        click_and_check_disabled(&mut dom, action);
        assert_eq!(reads.started(), 1);
        assert!(button(&dom, "原样重试").is_some());
        assert!(button(&dom, "关闭回执").is_none());
    }

    #[test]
    fn dashboard_poll_receipt_survives_the_failed_aggregate_read() {
        let reads: DashboardReads = Rc::new(RecordRead::new());
        let write = Rc::new(RecordRead::<AdminMailPolling>::new());
        let (mut dom, _) = mount_writer_at("poll=0", {
            let reads = reads.clone();
            let write = write.clone();
            move || {
                init_i18n();
                use_context_provider(|| reads.clone());
                use_context_provider(|| write.clone());
                let readiness: AdminReadinessResource = use_resource(|| async { Ok(true) });
                use_context_provider(|| readiness);
                let host = use_context::<PendingCommandHost>();
                rsx! {
                    PendingCommandNotices { host }
                    Router::<TestRoute> {}
                }
            }
        });
        let mut polling = polling_fixture(false);
        reads.respond(Ok(dashboard(&polling)));
        settle_writer(&mut dom);
        let label = |action_key| {
            dom.dom().in_scope(
                ScopeId::APP,
                || t!("dashboard-legacy-poller-action", action: t!(action_key), mailbox: "main"),
            )
        };
        let pause = label("dashboard-legacy-pause-poll");
        let resume = label("dashboard-legacy-resume-poll");
        click_button(&mut dom, &pause);
        polling.paused = true;
        polling.state_version = 2;
        write.respond(Ok(polling.clone()));
        settle_writer(&mut dom);
        let receipt = "轮询状态已更新。";
        assert!(has_live_text(&dom, receipt));
        reads.respond_unavailable();
        settle_writer(&mut dom);
        assert!(button(&dom, &pause).is_none());
        assert!(has_live_text(&dom, "所需服务暂时不可用。请稍后再试。"));
        assert!(has_live_text(&dom, receipt));
        assert!(button(&dom, "关闭回执").is_some());
        click_button(&mut dom, "重试");
        assert_eq!(reads.started(), 3);
        assert_eq!(write.started(), 1);
        reads.respond(Ok(dashboard(&polling)));
        settle_writer(&mut dom);
        assert!(button(&dom, &resume).is_some());
        assert!(has_live_text(&dom, receipt));
    }
}
