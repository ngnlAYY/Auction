//! Admin 管理总览页面。
//!
//! 总览只聚合各个已冻结 HTTP 投影的安全摘要，不创建新的后端状态，也不把失败的请求伪装
//! 成零值。页面因此可以在某个能力尚未部署或暂时不可用时，仍然明确呈现该能力的状态。

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api::{
    AdminMailIntakeQuery, fetch_admin_auctions, fetch_dashboard, fetch_deliveries,
    fetch_mail_intakes,
};

use super::Route;

#[path = "dashboard/controls.rs"]
mod controls;
#[path = "dashboard/metrics.rs"]
mod metrics;
#[path = "dashboard/runtime/mod.rs"]
mod runtime;

use controls::MailPollerControls;
use metrics::{
    DashboardStatusRow, MetricCard, alerting_metric, auction_metric, credential_metric,
    delivery_metric, mail_metric, realtime_metric,
};
use runtime::{DashboardRuntimeDetails, section_unavailable};

/// 加载管理总览所需的各类安全摘要。
pub(super) fn page() -> Element {
    let mut refresh = use_signal(|| 0_u64);
    let dashboard = use_resource(move || {
        let _ = refresh();
        fetch_dashboard()
    });
    let auctions = use_resource(move || {
        let _ = refresh();
        fetch_admin_auctions("all".to_owned(), "all".to_owned(), 1)
    });
    let deliveries = use_resource(move || {
        let _ = refresh();
        fetch_deliveries(String::new(), 1)
    });
    let mail_intakes = use_resource(move || {
        let _ = refresh();
        // dashboard 统计所有已配置邮箱，不能固定读取 main，否则 isekai 等邮箱的入站事实会被漏算。
        fetch_mail_intakes(AdminMailIntakeQuery::default(), 1)
    });

    let dashboard_state = dashboard.read_unchecked();
    let auction_state = auctions.read_unchecked();
    let delivery_state = deliveries.read_unchecked();
    let mail_intake_state = mail_intakes.read_unchecked();
    let mail_dashboard_state = dashboard_state.as_ref().map(|result| match result {
        Ok(snapshot) => Ok(snapshot.mail.clone()),
        Err(error) => Err(error.clone()),
    });
    let realtime_state = dashboard_state.as_ref().map(|result| match result {
        Ok(snapshot) => snapshot
            .realtime
            .data
            .clone()
            .ok_or_else(|| section_unavailable(snapshot.realtime.error_code.as_deref())),
        Err(error) => Err(error.clone()),
    });
    let alerting_state = dashboard_state.as_ref().map(|result| match result {
        Ok(snapshot) => Ok(snapshot.alerting.clone()),
        Err(error) => Err(error.clone()),
    });
    let credentials_state = dashboard_state.as_ref().map(|result| match result {
        Ok(snapshot) => Ok(snapshot.credentials.clone()),
        Err(error) => Err(error.clone()),
    });

    let auction_metric = auction_metric(&auction_state);
    let delivery_metric = delivery_metric(&delivery_state);
    let realtime_metric = realtime_metric(&realtime_state);
    let mail_metric = mail_metric(&mail_dashboard_state, &mail_intake_state);
    let alerting_metric = alerting_metric(&alerting_state);
    let credentials_metric = credential_metric(&credentials_state);
    let mail_pollers = mail_dashboard_state
        .as_ref()
        .and_then(|result| result.as_ref().ok())
        .and_then(|section| section.data.as_ref())
        .map(|data| data.pollers.clone())
        .unwrap_or_default();

    rsx! {
        div { class: "admin-dashboard",
            header { class: "admin-dashboard__hero",
                div { class: "admin-dashboard__hero-copy",
                    h1 { {t!("page-dashboard-title")} }
                    p { class: "admin-dashboard__subtitle", {t!("dashboard-subtitle")} }
                }
                div { class: "admin-dashboard__hero-actions",
                    span { class: "admin-dashboard__live-state",
                        span { class: "admin-dashboard__live-dot", aria_hidden: "true" }
                        {t!("dashboard-connection")}
                    }
                    button {
                        class: "admin-dashboard__refresh",
                        r#type: "button",
                        onclick: move |_| refresh.set(refresh().saturating_add(1)),
                        span { class: "admin-dashboard__refresh-icon", aria_hidden: "true" }
                        {t!("dashboard-refresh")}
                    }
                }
            }

            section { class: "admin-metric-grid", aria_labelledby: "dashboard-metrics-title",
                h2 { id: "dashboard-metrics-title", class: "admin-visually-hidden", {t!("dashboard-metrics-title")} }
                MetricCard { label: t!("dashboard-auctions-label"), metric: auction_metric.clone() }
                MetricCard { label: t!("dashboard-deliveries-label"), metric: delivery_metric.clone() }
                MetricCard { label: t!("dashboard-realtime-label"), metric: realtime_metric.clone() }
                MetricCard { label: t!("dashboard-mail-label"), metric: mail_metric.clone() }
                MetricCard { label: t!("dashboard-credentials-label"), metric: credentials_metric.clone() }
                MetricCard { label: t!("dashboard-alerting-label"), metric: alerting_metric.clone() }
            }

            div { class: "admin-dashboard__layout",
                section { class: "admin-dashboard__card", aria_labelledby: "dashboard-signals-title",
                    header { class: "admin-dashboard__card-header",
                        div {
                            p { class: "admin-dashboard__card-kicker", {t!("dashboard-signal-kicker")} }
                            h2 { id: "dashboard-signals-title", {t!("dashboard-signals-title")} }
                        }
                        span { class: "admin-dashboard__readonly", {t!("dashboard-readonly")} }
                    }
                    p { class: "admin-dashboard__card-note", {t!("dashboard-signals-note")} }
                    div { class: "admin-dashboard__status-list",
                        DashboardStatusRow { label: t!("dashboard-auctions-label"), metric: auction_metric }
                        DashboardStatusRow { label: t!("dashboard-deliveries-label"), metric: delivery_metric }
                        DashboardStatusRow { label: t!("dashboard-realtime-label"), metric: realtime_metric }
                        DashboardStatusRow { label: t!("dashboard-mail-label"), metric: mail_metric }
                        DashboardStatusRow { label: t!("dashboard-credentials-label"), metric: credentials_metric }
                        DashboardStatusRow { label: t!("dashboard-alerting-label"), metric: alerting_metric }
                    }
                }

                section { class: "admin-dashboard__card", aria_labelledby: "dashboard-shortcuts-title",
                    header { class: "admin-dashboard__card-header",
                        div {
                            p { class: "admin-dashboard__card-kicker", {t!("dashboard-shortcuts-kicker")} }
                            h2 { id: "dashboard-shortcuts-title", {t!("dashboard-shortcuts-title")} }
                        }
                    }
                    p { class: "admin-dashboard__card-note", {t!("dashboard-shortcuts-note")} }
                    nav { class: "admin-quick-links", aria_label: t!("dashboard-shortcuts-title"),
                        Link { class: "admin-quick-link", to: Route::Auctions {},
                            span { class: "admin-quick-link__icon admin-quick-link__icon--auction", aria_hidden: "true" }
                            span { class: "admin-quick-link__copy",
                                strong { {t!("dashboard-open-auctions")} }
                                small { {t!("dashboard-open-auctions-note")} }
                            }
                        }
                        Link { class: "admin-quick-link", to: Route::Mails {},
                            span { class: "admin-quick-link__icon admin-quick-link__icon--mail", aria_hidden: "true" }
                            span { class: "admin-quick-link__copy",
                                strong { {t!("dashboard-open-mail")} }
                                small { {t!("dashboard-open-mail-note")} }
                            }
                        }
                        Link { class: "admin-quick-link", to: Route::Deliveries {},
                            span { class: "admin-quick-link__icon admin-quick-link__icon--delivery", aria_hidden: "true" }
                            span { class: "admin-quick-link__copy",
                                strong { {t!("dashboard-open-deliveries")} }
                                small { {t!("dashboard-open-deliveries-note")} }
                            }
                        }
                        Link { class: "admin-quick-link", to: Route::Status {},
                            span { class: "admin-quick-link__icon admin-quick-link__icon--status", aria_hidden: "true" }
                            span { class: "admin-quick-link__copy",
                                strong { {t!("dashboard-open-status")} }
                                small { {t!("dashboard-open-status-note")} }
                            }
                        }
                    }
                }
            }

            if !mail_pollers.is_empty() {
                MailPollerControls { pollers: mail_pollers, refresh }
            }

            if let Some(Ok(snapshot)) = dashboard_state.as_ref() {
                DashboardRuntimeDetails { snapshot: snapshot.clone() }
            }

            p { class: "admin-dashboard__source-note", {t!("dashboard-source-note")} }
        }
    }
}
