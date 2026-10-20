//! Dashboard runtime 详情卡片与状态映射辅助。

use auction_web_shared::{TruncatedText, legacy_t as t};
use dioxus::prelude::*;

use crate::api::AdminCredentialScope;

#[component]
pub(crate) fn DashboardDetailCard(title: String, detail: String, tone: String) -> Element {
    rsx! {
        article { class: "admin-dashboard__detail-card admin-dashboard__detail-card--{tone}",
            h3 { TruncatedText { value: title, size: "wide", portal: true } }
            p { TruncatedText { value: detail, size: "wide", portal: true } }
        }
    }
}

pub(crate) fn runtime_bool(value: bool) -> String {
    if value { t!("dashboard-runtime-yes") } else { t!("dashboard-runtime-no") }
}

pub(crate) fn runtime_status(value: &str) -> String {
    match value {
        "not_applicable" => t!("dashboard-legacy-recovery-balance-not-applicable"),
        "ok" | "ready" | "available" | "fresh" => t!("dashboard-runtime-status-good"),
        "degraded" | "low" | "stale" | "starting" => t!("dashboard-runtime-status-warning"),
        "busy" => t!("dashboard-legacy-recovery-balance-busy"),
        "disabled" => t!("dashboard-runtime-status-disabled"),
        "unknown" | "unavailable" | "stopping" => t!("dashboard-runtime-status-unavailable"),
        _ => value.to_owned(),
    }
}

pub(crate) fn runtime_tone(value: &str) -> String {
    match value {
        "ok" | "ready" | "available" | "fresh" => "positive".to_owned(),
        "degraded" | "low" | "stale" | "starting" | "busy" => "warning".to_owned(),
        "unknown" | "unavailable" | "stopping" => "danger".to_owned(),
        _ => "neutral".to_owned(),
    }
}

pub(crate) fn credential_scope_status(scope: &AdminCredentialScope) -> &'static str {
    if scope.invalid_count > 0 {
        "unavailable"
    } else if scope.active_count > 0 {
        "ok"
    } else {
        "degraded"
    }
}

pub(crate) fn credential_scope_tone(scope: &AdminCredentialScope) -> String {
    match credential_scope_status(scope) {
        "ok" => "positive".to_owned(),
        "degraded" => "warning".to_owned(),
        _ => "danger".to_owned(),
    }
}

pub(crate) fn section_unavailable(error_code: Option<&str>) -> String {
    match error_code {
        Some("no_active_credentials") => t!("dashboard-runtime-no-active-credentials"),
        Some("invalid_credentials") => t!("dashboard-runtime-invalid-credentials"),
        Some("credentials_restart_required") => {
            t!("dashboard-runtime-credentials-restart-required")
        }
        Some("credentials_restart_unknown") => t!("dashboard-runtime-credentials-restart-unknown"),
        Some("unresolved_failure") => t!("dashboard-runtime-unresolved-failure"),
        Some("consumer_lag") => t!("dashboard-runtime-consumer-lag"),
        Some("source_unavailable") | Some("no_pollers") | None => {
            t!("dashboard-runtime-unavailable")
        }
        Some(_) => t!("dashboard-runtime-degraded"),
    }
}
