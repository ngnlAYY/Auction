//! 外部账号分区：进程内忙碌/暂停、请求门、维护与论坛安全摘要。
//! 只渲染 Worker 快照与既有 owner 投影；子来源不可用时显示明确的不可用文案，不用零值或
//! 空列表冒充“没有暂停”或“凭据齐全”。

use auction_web_shared::t;
use dioxus::prelude::*;

use crate::api::{AdminDashboardSnapshot, AdminExternalAccountsData};

use super::shared::{badge, card, text};

pub(super) fn external_accounts_panel(snapshot: &AdminDashboardSnapshot) -> Element {
    let section = &snapshot.external_accounts;
    let accounts = section.data.as_ref();
    rsx! {
    section { class: "admin-dashboard-panel", aria_labelledby: "dashboard-external-accounts-title",
    h2 { id: "dashboard-external-accounts-title", {t!("dashboard-external-accounts-title")} }
    if let Some(accounts) = accounts {
        {mail_section(snapshot, accounts)}
        {poller_section(accounts)}
        {forum_section(accounts)}
        {maintenance_section(accounts)}
        {credentials_section(accounts)}
    } else {
        p { class: "admin-dashboard-status", {t!("dashboard-external-unavailable")} }
    }

    }
    }
}

fn mail_section(
    snapshot: &AdminDashboardSnapshot,
    accounts: &AdminExternalAccountsData,
) -> Element {
    let mail = &accounts.mail;
    let health = health_badge(&snapshot.external_accounts.status);
    let mail_tone = if mail.busy { "warn" } else { "ok" };
    let mail_label = if mail.busy {
        t!("dashboard-external-mail-busy")
    } else {
        t!("dashboard-external-mail-idle")
    };
    let busy_age = mail.busy_age_seconds;
    rsx! {
        h3 { {t!("dashboard-external-health")} }
        div { class: "admin-dashboard-cards",
            {card(t!("dashboard-external-health"), health)}
            {card(t!("dashboard-external-mail-state"), badge(mail_tone, mail_label))}
            if let Some(seconds) = busy_age {
                {card(
                    t!("dashboard-external-mail-busy-age"),
                    text(t!("dashboard-legacy-duration-seconds", seconds: seconds)),
                )}
            }
            {card(
                t!("dashboard-external-mail-gate-waits"),
                text(mail.gate_wait_count.to_string()),
            )}
            {card(
                t!("dashboard-external-mail-gate-seconds"),
                text(t!(
                    "dashboard-legacy-duration-seconds",
                    seconds: mail.gate_wait_seconds_total
                )),
            )}
            {card(
                t!("dashboard-external-mail-requests"),
                text(mail.request_count.to_string()),
            )}
        }
        details { class: "admin-dashboard-details",
            summary { {t!("dashboard-metrics-details")} }
            div { class: "admin-dashboard-cards admin-dashboard-cards--timestamps",
                {card(
                    t!("dashboard-external-mail-last-request"),
                    text(observed(mail.last_request_at.as_deref())),
                )}
                {card(
                    t!("dashboard-external-mail-last-success"),
                    text(observed(mail.last_success_at.as_deref())),
                )}
                {card(
                    t!("dashboard-external-mail-last-failure"),
                    text(observed(mail.last_failure_at.as_deref())),
                )}
            }
        }
    }
}

fn poller_section(accounts: &AdminExternalAccountsData) -> Element {
    let pollers = &accounts.mail.pollers;
    let available = pollers.status == "available";
    rsx! {
        h3 { {t!("dashboard-external-pollers")} }
        if !available {
            p { class: "admin-dashboard-status", {t!("dashboard-external-projection-unavailable")} }
        } else if pollers.items.is_empty() {
            p { class: "admin-dashboard-status", {t!("dashboard-legacy-no-data")} }
        } else {
            div { class: "admin-dashboard-cards",
                for poller in pollers.items.iter() {
                    {
                        let mailbox = poller.mailbox_key.clone();
                        let tone = if poller.paused { "warn" } else { "ok" };
                        let state = if poller.paused {
                            t!("dashboard-external-poller-paused")
                        } else {
                            t!("dashboard-external-poller-running")
                        };
                        let paused_since = poller.paused_since.clone();
                        let failures = poller.consecutive_failures;
                        rsx! {
                            div { class: "admin-dashboard-card", key: "{mailbox}",
                                div { class: "admin-dashboard-value", {badge(tone, state)} }
                                div { class: "admin-dashboard-label", "{mailbox}" }
                                if let Some(at) = paused_since {
                                    div { class: "admin-dashboard-label",
                                        {t!("dashboard-external-poller-paused-since", at: at)}
                                    }
                                }
                                if failures > 0 {
                                    div { class: "admin-dashboard-label",
                                        {t!("dashboard-external-poller-failures", count: failures)}
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn forum_section(accounts: &AdminExternalAccountsData) -> Element {
    let forum = &accounts.forum;
    rsx! {
        details { class: "admin-dashboard-details",
            summary { {t!("dashboard-external-forum-state")} }
            div { class: "admin-dashboard-cards admin-dashboard-cards--timestamps",
                {card(
                    t!("dashboard-external-forum-last-request"),
                    text(observed(forum.last_request_at.as_deref())),
                )}
                {card(
                    t!("dashboard-external-forum-last-success"),
                    text(observed(forum.last_success_at.as_deref())),
                )}
                {card(
                    t!("dashboard-external-forum-last-failure"),
                    text(observed(forum.last_failure_at.as_deref())),
                )}
                {card(
                    t!("dashboard-external-forum-write-unknown"),
                    text(forum.write_unknown_count.to_string()),
                )}
            }
        }
    }
}

fn maintenance_section(accounts: &AdminExternalAccountsData) -> Element {
    let maintenance = &accounts.maintenance;
    let (tone, label) = match maintenance.state.as_str() {
        "running" => ("warn", t!("dashboard-external-maintenance-running")),
        "succeeded" => ("ok", t!("dashboard-external-maintenance-succeeded")),
        "failed" => ("error", t!("dashboard-external-maintenance-failed")),
        "canceled" => ("neutral", t!("dashboard-external-maintenance-canceled")),
        _ => ("neutral", t!("dashboard-external-maintenance-idle")),
    };
    let running_age = maintenance.running_age_seconds;
    rsx! {
        h3 { {t!("dashboard-external-maintenance")} }
        div { class: "admin-dashboard-cards",
            {card(t!("dashboard-external-maintenance"), badge(tone, label))}
            {card(
                t!("dashboard-external-maintenance-last-start"),
                text(observed(maintenance.last_started_at.as_deref())),
            )}
            {card(
                t!("dashboard-external-maintenance-last-success"),
                text(observed(maintenance.last_success_at.as_deref())),
            )}
            {card(
                t!("dashboard-external-maintenance-last-failure"),
                text(observed(maintenance.last_failure_at.as_deref())),
            )}
            if let Some(seconds) = running_age {
                {card(
                    t!("dashboard-external-maintenance-running-age"),
                    text(t!("dashboard-legacy-duration-seconds", seconds: seconds)),
                )}
            }
        }
    }
}

fn credentials_section(accounts: &AdminExternalAccountsData) -> Element {
    let credentials = &accounts.credentials;
    let available = credentials.status == "available";
    rsx! {
        h3 { {t!("dashboard-external-credentials")} }
        if !available {
            p { class: "admin-dashboard-status", {t!("dashboard-external-projection-unavailable")} }
        } else if credentials.items.is_empty() {
            p { class: "admin-dashboard-status", {t!("dashboard-legacy-no-data")} }
        } else {
            div { class: "admin-dashboard-cards",
                for credential in credentials.items.iter() {
                    {
                        let key = format!("{}-{}", credential.world, credential.kind);
                        let world = credential.world.clone();
                        let kind = match credential.kind.as_str() {
                            "mail_cookie" => t!("dashboard-external-kind-mail-cookie"),
                            "forum_cookie" => t!("dashboard-external-kind-forum-cookie"),
                            other => other.to_owned(),
                        };
                        let available = credential.available;
                        let invalid = credential.invalid;
                        let tone = if invalid > 0 {
                            "error"
                        } else if available > 0 {
                            "ok"
                        } else {
                            "warn"
                        };
                        rsx! {
                            div { class: "admin-dashboard-card", key: "{key}",
                                div { class: "admin-dashboard-value",
                                    span { class: "admin-dashboard-badge admin-dashboard-badge--{tone}",
                                        "{available}"
                                    }
                                }
                                div { class: "admin-dashboard-label",
                                    {t!(
                                        "dashboard-external-credential-row",
                                        world: world,
                                        kind: kind,
                                        available: available,
                                        invalid: invalid
                                    )}
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn health_badge(status: &str) -> Element {
    match status {
        "ok" => badge("ok", t!("dashboard-external-status-ok")),
        "degraded" => badge("warn", t!("dashboard-external-status-degraded")),
        _ => badge("neutral", t!("dashboard-external-status-unknown")),
    }
}

fn observed(value: Option<&str>) -> String {
    value.map_or_else(|| t!("dashboard-external-never"), str::to_owned)
}
