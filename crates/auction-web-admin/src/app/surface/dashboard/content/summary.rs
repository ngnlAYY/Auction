//! Dashboard 快捷入口与顶部摘要卡。
//! 快捷入口顺序、语义卡片 id 与轮询状态徽标沿用旧版 Dashboard。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use crate::api::AdminDashboardSnapshot;
use crate::app::surface::Route;

use super::derived::DashboardDerived;
use super::shared::{badge, card, semantic_card};

pub(super) fn summary_section(
    snapshot: &AdminDashboardSnapshot,
    derived: &DashboardDerived,
) -> Element {
    let worker_up = snapshot.worker.condition == "available";
    let poll_state = derived.poll_state;
    let poll_status = derived.poll_status.clone();

    rsx! {
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
            div { class: "admin-dashboard-cards admin-dashboard-cards--summary",
                {semantic_card(
                    "dashboard-worker-channel",
                    t!("dashboard-legacy-worker-channel"),
                    snapshot.worker.condition.clone(),
                    badge(
                        if worker_up { "ok" } else { "error" },
                        if worker_up {
                            t!("dashboard-legacy-channel-up")
                        } else {
                            t!("dashboard-legacy-channel-down")
                        },
                    ),
                )}
                {semantic_card(
                    "dashboard-worker-serving",
                    t!("dashboard-legacy-serving"),
                    if snapshot.worker.ready { "ready" } else { "not-ready" }.to_owned(),
                    badge(
                        if snapshot.worker.ready { "ok" } else { "error" },
                        if snapshot.worker.ready {
                            t!("dashboard-legacy-yes")
                        } else {
                            t!("dashboard-legacy-no")
                        },
                    ),
                )}
                {card(
                    t!("dashboard-legacy-poll-status"),
                    badge(poll_state, poll_status),
                )}
            }
        }
    }
}
