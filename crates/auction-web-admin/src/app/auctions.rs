//! Admin 拍卖列表页面。
//!
//! 当前页面展示 PostgreSQL 完整档的只读生命周期快照；写命令仍通过独立的版本化 API 接入。

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api::{
    AdminAuction, AdminAuctionLifecycleAction, AdminAuctionPage, create_admin_auction,
    execute_admin_auction_lifecycle, fetch_admin_auctions,
};

use super::{AdminPanel, Route};

/// 加载并展示 Admin 拍卖列表。
pub(super) fn page() -> Element {
    let world_filter = use_signal(|| "all".to_owned());
    let status_filter = use_signal(|| "all".to_owned());
    let page_number = use_signal(|| 1_i64);
    let reload_generation = use_signal(|| 0_u64);
    let auctions = use_resource(move || {
        let _ = reload_generation();
        fetch_admin_auctions(world_filter(), status_filter(), page_number())
    });

    match &*auctions.read_unchecked() {
        None => rsx! {
            AdminPanel { title: t!("page-auctions-title"), detail: t!("admin-auctions-loading") }
        },
        Some(Err(_)) => rsx! {
            AdminPanel { title: t!("page-auctions-title"), detail: t!("admin-auctions-error") }
        },
        Some(Ok(page)) => rsx! {
            div { class: "admin-auctions",
                AdminAuctionCreateForm { reload_generation }
                AdminAuctionTable {
                    page: page.clone(),
                    world_filter,
                    status_filter,
                    page_number,
                    reload_generation,
                }
            }
        },
    }
}

#[component]
fn AdminAuctionCreateForm(reload_generation: Signal<u64>) -> Element {
    let mut world_id = use_signal(|| "main".to_owned());
    let mut sequence = use_signal(String::new);
    let mut preview_scheduled_at = use_signal(String::new);
    let mut scheduled_open_at = use_signal(String::new);
    let mut scheduled_end_at = use_signal(String::new);
    let busy = use_signal(|| false);
    let mut feedback = use_signal(|| None::<bool>);

    rsx! {
        section { class: "admin-panel", aria_labelledby: "admin-auctions-create-title",
            h2 { id: "admin-auctions-create-title", {t!("admin-auctions-create-title")} }
            p { class: "admin-panel__note", {t!("admin-auctions-create-timezone")} }
            div { class: "admin-panel__actions",
                label {
                    {t!("admin-auctions-create-world")}
                    select {
                        value: world_id(),
                        onchange: move |event| world_id.set(event.value()),
                        option { value: "main", {t!("admin-auctions-world-main")} }
                        option { value: "isekai", {t!("admin-auctions-world-isekai")} }
                    }
                }
                label {
                    {t!("admin-auctions-create-sequence")}
                    input {
                        r#type: "number",
                        min: "1",
                        value: sequence(),
                        oninput: move |event| sequence.set(event.value()),
                    }
                }
                label {
                    {t!("admin-auctions-create-preview")}
                    input {
                        r#type: "datetime-local",
                        value: preview_scheduled_at(),
                        oninput: move |event| preview_scheduled_at.set(event.value()),
                    }
                }
                label {
                    {t!("admin-auctions-create-open")}
                    input {
                        r#type: "datetime-local",
                        value: scheduled_open_at(),
                        oninput: move |event| scheduled_open_at.set(event.value()),
                    }
                }
                label {
                    {t!("admin-auctions-create-end")}
                    input {
                        r#type: "datetime-local",
                        value: scheduled_end_at(),
                        oninput: move |event| scheduled_end_at.set(event.value()),
                    }
                }
                button {
                    r#type: "button",
                    disabled: busy(),
                    onclick: move |_| {
                        let Ok(sequence_value) = sequence().parse::<i64>() else {
                            feedback.set(Some(false));
                            return;
                        };
                        let world_id = world_id();
                        let preview_scheduled_at = preview_scheduled_at();
                        let scheduled_open_at = scheduled_open_at();
                        let scheduled_end_at = scheduled_end_at();
                        let mut busy = busy;
                        let mut feedback = feedback;
                        let mut reload_generation = reload_generation;
                        spawn(async move {
                            busy.set(true);
                            let result = create_admin_auction(
                                world_id,
                                sequence_value,
                                preview_scheduled_at,
                                scheduled_open_at,
                                scheduled_end_at,
                            )
                            .await;
                            match result {
                                Ok(_) => {
                                    feedback.set(Some(true));
                                    reload_generation.set(reload_generation().saturating_add(1));
                                }
                                Err(_) => feedback.set(Some(false)),
                            }
                            busy.set(false);
                        });
                    },
                    {t!("admin-auctions-create-submit")}
                }
            }
            if let Some(success) = feedback() {
                p { class: "admin-panel__note",
                    {if success {
                        t!("admin-auctions-create-success")
                    } else {
                        t!("admin-auctions-create-error")
                    }}
                }
            }
        }
    }
}

#[component]
fn AdminAuctionTable(
    page: AdminAuctionPage,
    mut world_filter: Signal<String>,
    mut status_filter: Signal<String>,
    mut page_number: Signal<i64>,
    reload_generation: Signal<u64>,
) -> Element {
    let can_previous = page.pagination.page > 1;
    let can_next = page.pagination.page * page.pagination.page_size < page.pagination.total;
    rsx! {
        section { class: "admin-panel", aria_labelledby: "admin-auctions-title",
            h1 { id: "admin-auctions-title", {t!("page-auctions-title")} }
            p { class: "admin-panel__note",
                {t!("admin-auctions-summary", total: page.pagination.total)}
            }
            div { class: "admin-panel__actions",
                label {
                    {t!("admin-auctions-world")}
                    select {
                        value: world_filter(),
                        onchange: move |event| {
                            world_filter.set(event.value());
                            page_number.set(1);
                        },
                        option { value: "all", {t!("admin-auctions-world-all")} }
                        option { value: "main", {t!("admin-auctions-world-main")} }
                        option { value: "isekai", {t!("admin-auctions-world-isekai")} }
                    }
                }
                label {
                    {t!("admin-auctions-status")}
                    select {
                        value: status_filter(),
                        onchange: move |event| {
                            status_filter.set(event.value());
                            page_number.set(1);
                        },
                        option { value: "all", {t!("admin-auctions-status-all")} }
                        for status in ["draft", "preview", "live", "final", "settling", "settled", "cancelled"] {
                            option { value: status, {admin_auction_status_label(status)} }
                        }
                    }
                }
            }
            div { class: "public-auction-catalog",
                table {
                    thead {
                        tr {
                            th { {t!("admin-auctions-world")} }
                            th { {t!("admin-auctions-sequence")} }
                            th { {t!("admin-auctions-status")} }
                            th { {t!("admin-auctions-lots")} }
                            th { {t!("admin-auctions-updated")} }
                            th { {t!("catalog-actions")} }
                            th { {t!("admin-auctions-actions")} }
                        }
                    }
                    tbody {
                        for auction in page.auctions {
                            AdminAuctionRow { auction, reload_generation }
                        }
                    }
                }
            }
            div { class: "admin-panel__actions",
                button {
                    r#type: "button",
                    disabled: !can_previous,
                    onclick: move |_| page_number.set(page.pagination.page - 1),
                    {t!("admin-auctions-previous")}
                }
                span { {t!("admin-auctions-page", page: page.pagination.page)} }
                button {
                    r#type: "button",
                    disabled: !can_next,
                    onclick: move |_| page_number.set(page.pagination.page + 1),
                    {t!("admin-auctions-next")}
                }
            }
        }
    }
}

fn admin_auction_status_label(status: &str) -> String {
    match status {
        "draft" => t!("admin-auctions-status-draft"),
        "preview" => t!("admin-auctions-status-preview"),
        "live" => t!("admin-auctions-status-live"),
        "final" => t!("admin-auctions-status-final"),
        "settling" => t!("admin-auctions-status-settling"),
        "settled" => t!("admin-auctions-status-settled"),
        "cancelled" => t!("admin-auctions-status-cancelled"),
        _ => status.to_owned(),
    }
}

fn admin_auction_world_label(world: &str) -> String {
    match world {
        "main" => t!("admin-auctions-world-main"),
        "isekai" => t!("admin-auctions-world-isekai"),
        _ => world.to_owned(),
    }
}

#[component]
fn AdminAuctionRow(auction: AdminAuction, reload_generation: Signal<u64>) -> Element {
    let auction_id = auction.id.clone();
    let lifecycle_auction_id = auction.id.clone();
    let lifecycle_status = auction.status.clone();
    rsx! {
        tr {
            td { {admin_auction_world_label(&auction.world_id)} }
            td { class: "public-auction-catalog__number", "{auction.sequence}" }
            td { {admin_auction_status_label(&auction.status)} }
            td { class: "public-auction-catalog__number", "{auction.lot_count}" }
            td { "{auction.updated_at}" }
            td {
                Link {
                    to: Route::AuctionLots { auction_id },
                    {t!("admin-auctions-open")}
                }
            }
            td {
                AdminAuctionActions {
                    auction_id: lifecycle_auction_id,
                    status: lifecycle_status,
                    reload_generation,
                }
            }
        }
    }
}

#[component]
fn AdminAuctionActions(
    auction_id: String,
    status: String,
    reload_generation: Signal<u64>,
) -> Element {
    let busy = use_signal(|| false);
    let feedback = use_signal(|| None::<bool>);
    let actions = lifecycle_actions(&status);

    rsx! {
        for action in actions {
            AdminAuctionActionButton {
                auction_id: auction_id.clone(),
                action,
                busy,
                feedback,
                reload_generation,
            }
        }
        if let Some(success) = feedback() {
            span { class: "admin-panel__note",
                {if success { t!("admin-auctions-action-success") } else { t!("admin-auctions-action-error") }}
            }
        }
    }
}

#[component]
fn AdminAuctionActionButton(
    auction_id: String,
    action: AdminAuctionLifecycleAction,
    busy: Signal<bool>,
    feedback: Signal<Option<bool>>,
    reload_generation: Signal<u64>,
) -> Element {
    rsx! {
        button {
            r#type: "button",
            disabled: busy(),
            onclick: move |_| {
                let auction_id = auction_id.clone();
                let mut busy = busy;
                let mut feedback = feedback;
                let mut reload_generation = reload_generation;
                spawn(async move {
                    busy.set(true);
                    let result = execute_admin_auction_lifecycle(auction_id, action).await;
                    match result {
                        Ok(_) => {
                            feedback.set(Some(true));
                            reload_generation.set(reload_generation().saturating_add(1));
                        }
                        Err(_) => feedback.set(Some(false)),
                    }
                    busy.set(false);
                });
            },
            {lifecycle_action_label(action)}
        }
    }
}

fn lifecycle_actions(status: &str) -> Vec<AdminAuctionLifecycleAction> {
    match status {
        "draft" => vec![AdminAuctionLifecycleAction::Preview, AdminAuctionLifecycleAction::Cancel],
        "preview" => vec![AdminAuctionLifecycleAction::Open, AdminAuctionLifecycleAction::Cancel],
        "live" => {
            vec![AdminAuctionLifecycleAction::FinalPhase, AdminAuctionLifecycleAction::Cancel]
        }
        _ => Vec::new(),
    }
}

fn lifecycle_action_label(action: AdminAuctionLifecycleAction) -> String {
    match action {
        AdminAuctionLifecycleAction::Preview => t!("admin-auctions-action-preview"),
        AdminAuctionLifecycleAction::Open => t!("admin-auctions-action-open"),
        AdminAuctionLifecycleAction::Cancel => t!("admin-auctions-action-cancel"),
        AdminAuctionLifecycleAction::FinalPhase => t!("admin-auctions-action-final"),
    }
}
