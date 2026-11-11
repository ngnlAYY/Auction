//! Dashboard 快捷入口与顶部摘要卡。
//! 快捷入口顺序、语义卡片 id 与轮询状态徽标沿用旧版 Dashboard。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use crate::api::AdminDashboardSnapshot;
use crate::surface::Route;

use super::derived::DashboardDerived;

pub(super) fn summary_section(
    snapshot: &AdminDashboardSnapshot,
    derived: &DashboardDerived,
) -> Element {
    let worker_up = snapshot.worker.condition == "available";
    let active_alerts = snapshot.alerting.data.as_ref().map_or(0, |data| data.active_signal_count);

    rsx! {
        div { class: "console-kpi console-kpi--6",
            div { class: "console-kpi__cell",
                div { class: "console-kpi__label", {t!("dashboard-legacy-worker-channel")} }
                div { class: "console-kpi__value", "{snapshot.worker.condition}" }
                div { class: "console-kpi__sub",
                    if worker_up {
                        {t!("dashboard-legacy-channel-up")}
                    } else {
                        {t!("dashboard-legacy-channel-down")}
                    }
                }
            }
            div { class: "console-kpi__cell",
                div { class: "console-kpi__label", {t!("dashboard-legacy-serving")} }
                div { class: "console-kpi__value",
                    if snapshot.worker.ready {
                        {t!("dashboard-legacy-yes")}
                    } else {
                        {t!("dashboard-legacy-no")}
                    }
                }
            }
            div { class: "console-kpi__cell", "data-state": derived.poll_state,
                div { class: "console-kpi__label", {t!("dashboard-legacy-poll-status")} }
                div { class: "console-kpi__value", "{derived.poll_status}" }
            }
            div { class: "console-kpi__cell",
                div { class: "console-kpi__label", {t!("dashboard-legacy-audit-health")} }
                div { class: "console-kpi__value", "{derived.audit_status}" }
            }
            div { class: "console-kpi__cell",
                div { class: "console-kpi__label", {t!("dashboard-legacy-outbox-pending")} }
                div { class: "console-kpi__value", "{derived.pending}" }
            }
            div { class: "console-kpi__cell",
                div { class: "console-kpi__label", {t!("dashboard-legacy-alerting-active")} }
                div { class: "console-kpi__value", "{active_alerts}" }
            }
        }
        section {
            class: "admin-dashboard-overview",
            aria_labelledby: "dashboard-shortcuts-title",
            div { class: "admin-dashboard-overview__head",
                h2 { id: "dashboard-shortcuts-title", {t!("dashboard-shortcuts-title")} }
            }
            nav { class: "admin-quick-links", aria_label: t!("dashboard-shortcuts-title"),
                Link { class: "admin-quick-link", to: Route::Auctions {}, title: t!("dashboard-open-auctions-note"),
                    span { class: "admin-quick-link__icon", aria_hidden: "true" }
                    span { class: "admin-quick-link__body",
                        strong { {t!("dashboard-open-auctions")} }
                        small { {t!("dashboard-open-auctions-note")} }
                    }
                }
                Link { class: "admin-quick-link", to: Route::Mails {}, title: t!("dashboard-open-mail-note"),
                    span { class: "admin-quick-link__icon admin-quick-link__icon--mail", aria_hidden: "true" }
                    span { class: "admin-quick-link__body",
                        strong { {t!("dashboard-open-mail")} }
                        small { {t!("dashboard-open-mail-note")} }
                    }
                }
                Link { class: "admin-quick-link", to: Route::Deliveries {}, title: t!("dashboard-open-deliveries-note"),
                    span { class: "admin-quick-link__icon admin-quick-link__icon--delivery", aria_hidden: "true" }
                    span { class: "admin-quick-link__body",
                        strong { {t!("dashboard-open-deliveries")} }
                        small { {t!("dashboard-open-deliveries-note")} }
                    }
                }
                Link { class: "admin-quick-link", to: Route::Settlement {}, title: t!("dashboard-open-settlement-note"),
                    span { class: "admin-quick-link__icon admin-quick-link__icon--settlement", aria_hidden: "true" }
                    span { class: "admin-quick-link__body",
                        strong { {t!("nav-settlement")} }
                        small { {t!("dashboard-open-settlement-note")} }
                    }
                }
                Link { class: "admin-quick-link", to: Route::Outbox {}, title: t!("dashboard-open-status-note"),
                    span { class: "admin-quick-link__icon admin-quick-link__icon--status", aria_hidden: "true" }
                    span { class: "admin-quick-link__body",
                        strong { {t!("dashboard-open-status")} }
                        small { {t!("dashboard-open-status-note")} }
                    }
                }
                Link { class: "admin-quick-link", to: Route::Config {},
                    span { class: "admin-quick-link__icon admin-quick-link__icon--config", aria_hidden: "true" }
                    span { class: "admin-quick-link__body",
                        strong { {t!("nav-config")} }
                    }
                }
            }
        }
    }
}
