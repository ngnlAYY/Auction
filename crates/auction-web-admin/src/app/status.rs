//! Admin 服务状态页面。
//!
//! 这里展示进程存活和依赖就绪两个兼容探针；业务 dashboard、Worker 状态和 realtime 水位
//! 继续由各自页面负责，避免把不同语义压成一个不准确的健康标志。

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api::{AdminHealthProbeState, fetch_admin_health};

use super::{AdminLoadError, AdminPanel};

/// 加载并展示 Admin 的兼容健康探针。
pub(super) fn page() -> Element {
    let mut refresh = use_signal(|| 0_u64);
    let health = use_resource(move || {
        let _ = refresh();
        fetch_admin_health()
    });

    match &*health.read_unchecked() {
        None => rsx! {
            AdminPanel { title: t!("page-status-title"), detail: t!("admin-status-loading") }
        },
        Some(Err(_)) => rsx! {
            AdminLoadError {
                title: t!("page-status-title"),
                detail: t!("admin-status-error"),
                refresh,
            }
        },
        Some(Ok(status)) => rsx! {
            section {
                class: "admin-panel admin-status-page",
                aria_labelledby: "admin-status-title",
                h1 { id: "admin-status-title", {t!("page-status-title")} }
                p { class: "admin-panel__note", {t!("page-status-note")} }
                div { class: "admin-status-page__probes",
                    HealthProbeCard {
                        label: t!("admin-status-live"),
                        state: status.live,
                    }
                    HealthProbeCard {
                        label: t!("admin-status-ready"),
                        state: status.ready,
                    }
                }
                div { class: "admin-panel__actions admin-status-page__actions",
                    button {
                        class: "admin-btn-secondary",
                        r#type: "button",
                        onclick: move |_| refresh.set(refresh().saturating_add(1)),
                        {t!("admin-status-refresh")}
                    }
                }
            }
        },
    }
}

#[component]
fn HealthProbeCard(label: String, state: AdminHealthProbeState) -> Element {
    let (class, text) = match state {
        AdminHealthProbeState::Healthy => ("positive", t!("admin-status-healthy")),
        AdminHealthProbeState::NotReady => ("warning", t!("admin-status-not-ready")),
        AdminHealthProbeState::Unavailable => ("danger", t!("admin-status-unavailable")),
    };
    rsx! {
        article { class: "admin-status-page__probe",
            h2 { {label} }
            span { class: "admin-status-pill admin-status-pill--{class}", {text} }
        }
    }
}
