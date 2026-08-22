//! Public 持久化实时事件页面。
//!
//! 页面只推进服务端返回的 `next_after` 游标，不在浏览器端应用竞价或生命周期状态机。
//! `refetch_required` 会停止继续消费，并要求重新读取权威快照。

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api::{
    HealthStatus, RealtimeEvent, RealtimePage, fetch_health, fetch_public_session,
    fetch_realtime_events, subscribe_realtime,
};

use super::Placeholder;

#[derive(Clone, Copy, PartialEq, Eq)]
enum LiveConnectionState {
    Checking,
    Connecting,
    Connected,
    Anonymous,
    Stopped,
    Error,
}

pub(super) fn page() -> Element {
    let mut cursor = use_signal(|| 0_i64);
    let mut refresh = use_signal(|| 0_u64);
    let mut live_page = use_signal(|| None::<RealtimePage>);
    let live_state = use_signal(|| LiveConnectionState::Checking);
    let health = use_resource(fetch_health);
    let events = use_resource(move || {
        let after = cursor();
        let _ = refresh();
        fetch_realtime_events(after)
    });
    let _subscription = use_coroutine({
        let live_page = live_page;
        let mut live_state = live_state;
        move |_rx: UnboundedReceiver<()>| async move {
            match fetch_public_session().await {
                Ok(Some(session)) if session.authenticated => {
                    live_state.set(LiveConnectionState::Connecting);
                    let mut callback_page = live_page;
                    let mut callback_state = live_state;
                    match subscribe_realtime(0, move |page| {
                        callback_state.set(LiveConnectionState::Connected);
                        callback_page.set(Some(page));
                    })
                    .await
                    {
                        Ok(()) => live_state.set(LiveConnectionState::Stopped),
                        Err(_) => live_state.set(LiveConnectionState::Error),
                    }
                }
                Ok(_) => live_state.set(LiveConnectionState::Anonymous),
                Err(_) => live_state.set(LiveConnectionState::Error),
            }
        }
    });
    let live_snapshot = live_page();
    let connection_state = live_state();

    match (&*health.read_unchecked(), &*events.read_unchecked()) {
        (None, _) | (_, None) => rsx! {
            Placeholder { title: t!("page-status-title"), detail: t!("realtime-loading") }
        },
        (Some(Err(_)), _) | (_, Some(Err(_))) => rsx! {
            Placeholder { title: t!("page-status-title"), detail: t!("realtime-error") }
        },
        (Some(Ok(health_status)), Some(Ok(event_page))) => {
            let event_page = live_snapshot.unwrap_or_else(|| event_page.clone());
            let next_after = event_page.next_after;
            let can_load_more = event_page.has_more && !event_page.refetch_required;
            rsx! {
                HealthSummary { status: health_status.clone() }
                section { class: "public-auction-category", aria_labelledby: "realtime-title",
                    div { class: "public-auction-category__header",
                        h1 { id: "realtime-title", class: "public-auction-category__title",
                            {t!("page-status-title")}
                        }
                        button {
                            r#type: "button",
                            onclick: move |_| {
                                cursor.set(0);
                                refresh.set(refresh() + 1);
                            },
                            {t!("realtime-refresh")}
                        }
                    }
                    p { class: "public-auction-page__status", role: "status",
                        {live_connection_message(connection_state)}
                    }
                    p { class: "public-auction-page__status",
                        {t!("realtime-cursor", sequence: event_page.next_after)}
                    }
                    if event_page.refetch_required {
                        p { class: "public-auction-page__status", role: "alert",
                            {t!("realtime-refetch-required")}
                        }
                    }
                    if event_page.events.is_empty() {
                        p { class: "public-auction-page__status", {t!("realtime-empty")} }
                    } else {
                        RealtimeEventTable { events: event_page.events.clone() }
                    }
                    if can_load_more {
                        button {
                            r#type: "button",
                            onclick: move |_| {
                                live_page.set(None);
                                cursor.set(next_after);
                            },
                            {t!("realtime-load-more")}
                        }
                    }
                }
            }
        }
    }
}

fn live_connection_message(state: LiveConnectionState) -> String {
    match state {
        LiveConnectionState::Checking => t!("realtime-connection-checking"),
        LiveConnectionState::Connecting => t!("realtime-connection-connecting"),
        LiveConnectionState::Connected => t!("realtime-connection-connected"),
        LiveConnectionState::Anonymous => t!("realtime-connection-anonymous"),
        LiveConnectionState::Stopped => t!("realtime-connection-stopped"),
        LiveConnectionState::Error => t!("realtime-connection-error"),
    }
}

#[component]
fn HealthSummary(status: HealthStatus) -> Element {
    rsx! {
        section { class: "public-auction-category", aria_labelledby: "health-title",
            h1 { id: "health-title", class: "public-auction-category__title",
                {t!("page-status-title")}
            }
            dl { class: "public-auction-preview-stats", aria_label: t!("page-status-title"),
                div {
                    dt { {t!("health-service")} }
                    dd { "{status.service}" }
                }
                div {
                    dt { {t!("health-status")} }
                    dd { "{status.status}" }
                }
                div {
                    dt { {t!("health-version")} }
                    dd { "{status.version}" }
                }
            }
        }
    }
}

#[component]
fn RealtimeEventTable(events: Vec<RealtimeEvent>) -> Element {
    rsx! {
        div { class: "public-auction-catalog",
            table {
                thead {
                    tr {
                        th { {t!("realtime-event-sequence")} }
                        th { {t!("realtime-event-kind")} }
                        th { {t!("realtime-event-schema")} }
                        th { {t!("realtime-event-payload")} }
                        th { {t!("realtime-event-time")} }
                    }
                }
                tbody {
                    for event in events {
                        RealtimeEventRow { event }
                    }
                }
            }
        }
    }
}

#[component]
fn RealtimeEventRow(event: RealtimeEvent) -> Element {
    let payload = event.payload.to_string();
    rsx! {
        tr {
            td { class: "public-auction-catalog__number", "{event.stream_sequence}" }
            td { "{event.kind}" }
            td { class: "public-auction-catalog__number", "{event.schema_version}" }
            td { "{payload}" }
            td {
                time { datetime: event.created_at.clone(), "{event.created_at}" }
            }
        }
    }
}
