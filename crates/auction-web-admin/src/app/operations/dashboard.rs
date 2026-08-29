//! Admin 管理总览页面。
//!
//! 旧版 Dashboard 只读取一次聚合快照；各项指标、邮件控制和局部降级都从该快照派生，不能再
//! 额外请求列表接口拼装第二套页面状态。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use crate::api::fetch_dashboard;

use super::admin_api_error_message;

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
    let state = dashboard.read_unchecked();

    rsx! {
        section {
            class: "admin-dashboard-page",
            aria_labelledby: "dashboard-title",
            h1 { id: "dashboard-title", {t!("page-dashboard-title")} }
            if state.is_none() {
                p {
                    class: "admin-dashboard-status",
                    role: "status",
                    {t!("dashboard-legacy-loading")}
                }
            }
            if let Some(Err(error)) = state.as_ref() {
                div { role: "alert",
                    p { {admin_api_error_message(error)} }
                    button {
                        r#type: "button",
                        onclick: move |_| refresh.set(refresh().saturating_add(1)),
                        {t!("admin-retry")}
                    }
                }
            }
            if let Some(Ok(snapshot)) = state.as_ref() {
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
