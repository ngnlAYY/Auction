//! Admin 管理总览页面。
//!
//! 旧版 Dashboard 只读取一次聚合快照；各项指标、邮件控制和局部降级都从该快照派生，不能再
//! 额外请求列表接口拼装第二套页面状态。

use auction_web_shared::{AsyncState, AsyncStateKind, legacy_t as t, use_app_boot_ready};
use dioxus::prelude::*;

use crate::api::fetch_dashboard;

use super::poll::{AutoPollControls, use_auto_poll};
use super::query::use_dashboard_query_state;
use super::{AdminReadinessResource, admin_api_error_message};

#[path = "dashboard/content.rs"]
mod content;
#[path = "dashboard/controls.rs"]
mod controls;

use content::DashboardContent;
use controls::MailPollerControls;

/// 按旧版单快照请求模型加载管理总览。
pub(super) fn page() -> Element {
    let mut refresh = use_signal(|| 0_u64);
    let dashboard = use_resource(move || {
        let _ = refresh();
        fetch_dashboard()
    });
    // 自动轮询开关与间隔入 URL（`poll` 键），默认关闭；页面隐藏时轮询挂起。
    // 轮询只重取当前快照，不额外写地址栏。
    let query = use_dashboard_query_state();
    let query_state = query.state;
    let poll_seconds = use_memo(move || query_state().poll_seconds);
    use_auto_poll(poll_seconds, move || refresh.set(refresh().saturating_add(1)));
    let mut poll_query = query.clone();
    let readiness = use_context::<AdminReadinessResource>();
    // 默认管理首页的聚合快照无论成功或失败，只要已有结果就一次性揭示真实页面。
    use_app_boot_ready(readiness.read().is_some() && dashboard.read().is_some());
    let state = dashboard.read_unchecked();

    rsx! {
        section {
            class: "admin-dashboard-page",
            aria_labelledby: "dashboard-title",
            div { class: "admin-dashboard-heading",
                h1 { id: "dashboard-title", {t!("page-dashboard-title")} }
                AutoPollControls {
                    poll_seconds: poll_seconds(),
                    on_change: move |seconds| {
                        // 开关与间隔变更立即生效并写回地址栏。
                        let mut next = query_state();
                        next.poll_seconds = seconds;
                        poll_query.push(next);
                    },
                }
            }
            if state.is_none() {
                AsyncState {
                    kind: AsyncStateKind::Loading,
                    title: t!("dashboard-legacy-loading"),
                    detail: "",
                }
            }
            if let Some(Err(error)) = state.as_ref() {
                AsyncState {
                    kind: AsyncStateKind::Error,
                    title: admin_api_error_message(error),
                    detail: "",
                    retry_label: t!("admin-retry"),
                    on_retry: move |_| refresh.set(refresh().saturating_add(1)),
                }
            }
            if let Some(Ok(snapshot)) = state.as_ref() {
                div { class: "admin-dashboard-actions admin-dashboard-actions--refresh",
                    button {
                        class: "admin-btn-secondary",
                        r#type: "button",
                        onclick: move |_| refresh.set(refresh().saturating_add(1)),
                        {t!("dashboard-refresh")}
                    }
                }
                DashboardContent { snapshot: snapshot.clone() }
                MailPollerControls {
                    pollers: snapshot
                        .mail
                        .data
                        .as_ref()
                        .map(|data| data.pollers.clone())
                        .unwrap_or_default(),
                    refresh,
                }
            }
        }
    }
}
