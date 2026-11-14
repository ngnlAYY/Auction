//! Cookie 就绪度面板：按世界展示可用/失效凭据与历史可用数量。

use auction_web_shared::t;
use dioxus::prelude::*;

use super::widgets::badge;
use crate::api::AdminDashboardSnapshot;
use crate::surface::admin_world_label;

pub(super) fn credentials_panel(snapshot: &AdminDashboardSnapshot) -> Element {
    let credentials = snapshot.credentials.data.as_ref();
    let mut sorted_availability =
        credentials.map(|data| data.availability.clone()).unwrap_or_default();
    sorted_availability.sort_by(|left, right| match (left.world.as_str(), right.world.as_str()) {
        ("main", "main") => std::cmp::Ordering::Equal,
        ("main", _) => std::cmp::Ordering::Less,
        (_, "main") => std::cmp::Ordering::Greater,
        _ => left.world.cmp(&right.world),
    });
    rsx! {
    section { class: "admin-dashboard-panel", aria_labelledby: "dashboard-credentials-title",
    h2 { id: "dashboard-credentials-title", {t!("dashboard-legacy-cookie-readiness-by-world")} }
    if credentials.is_none_or(|data| data.availability.is_empty()) {
        p { class: "admin-dashboard-status", {t!("dashboard-legacy-no-data")} }
    } else if let Some(credentials) = credentials {
        div { class: "admin-dashboard-cards",
            for item in credentials.availability.iter() {
                div { class: "admin-dashboard-card", key: "{item.world}",
                    div { class: "admin-dashboard-value", {readiness_badge(item.available, item.invalid)} }
                    div { class: "admin-dashboard-label", {admin_world_label(&item.world)} }
                }
            }
        }
    }

    h3 { {t!("dashboard-legacy-cookies-by-world")} }
    if sorted_availability.is_empty() {
        p { class: "admin-dashboard-status", {t!("dashboard-legacy-no-data")} }
    } else {
        div { class: "admin-dashboard-cards",
            for item in sorted_availability.iter() {
                div { class: "admin-dashboard-card", key: "{item.world}",
                    div { class: "admin-dashboard-value", "{item.available}" }
                    div { class: "admin-dashboard-label", {admin_world_label(&item.world)} }
                }
            }
        }
    }

    }
    }
}

/// 有可用 Cookie 即就绪；否则区分存在无法解密的凭据与没有生效凭据。
fn readiness_badge(available: i64, invalid: i64) -> Element {
    let (tone, status) = if available > 0 {
        ("ok", t!("dashboard-legacy-cookie-ready"))
    } else if invalid > 0 {
        ("error", t!("dashboard-legacy-cookie-undecryptable"))
    } else {
        ("warn", t!("dashboard-legacy-cookie-no-active"))
    };
    badge(tone, status)
}
