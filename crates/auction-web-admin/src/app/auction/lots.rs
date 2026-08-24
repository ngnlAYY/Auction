//! Admin 拍卖 lot 页面。

use dioxus::{html::Key, prelude::*};
use dioxus_i18n::t;
use uuid::Uuid;

use auction_web_shared::{TruncatedText, format_credit_amount as format_amount};

use crate::api::{
    AdminLot, AdminLotPage, AdminLotUpdateAction, AdminProxyEvent, AdminProxyEventPage,
    fetch_admin_lots, fetch_admin_proxy_events, update_admin_lot,
};

use super::intake::AdminIntakeForm;
use super::pagination::AdminPager;
use super::{AdminPanel, Route, format_admin_time};

#[derive(Clone, Copy, PartialEq, Eq)]
enum LotDialog {
    Reprice,
    Withdraw,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LotUpdateNotice {
    Repriced,
    Withdrawn,
}

/// 加载并展示一个拍卖的 Admin lot 安全投影。
pub(super) fn page(auction_id: String) -> Element {
    let page_number = use_signal(|| 1_i64);
    let reload_generation = use_signal(|| 0_u64);
    let lots = use_resource({
        let auction_id = auction_id.clone();
        move || {
            let _ = reload_generation();
            fetch_admin_lots(auction_id.clone(), page_number())
        }
    });

    match &*lots.read_unchecked() {
        None => rsx! {
            AdminPanel {
                title: t!("page-auction-lots-title", auction: "…"),
                detail: t!("admin-lots-loading"),
            }
        },
        Some(Err(_)) => rsx! {
            AdminLotLoadError { reload_generation }
        },
        Some(Ok(page)) => rsx! {
            AdminLotTable { page: page.clone(), page_number, reload_generation }
        },
    }
}

#[component]
fn AdminLotTable(
    page: AdminLotPage,
    mut page_number: Signal<i64>,
    reload_generation: Signal<u64>,
) -> Element {
    let selected_lot = use_signal(|| None::<(String, i32)>);
    let notice = use_signal(|| None::<LotUpdateNotice>);
    let allow_lot_update = page.auction.status == "draft";

    rsx! {
        section { class: "admin-panel", aria_labelledby: "admin-lots-title",
            h1 { id: "admin-lots-title",
                {t!("page-auction-lots-title", auction: page.auction.sequence)}
            }
            p { class: "admin-lots__back",
                Link { to: Route::Auctions {}, {t!("admin-lots-back")} }
            }
            if let Some(notice) = notice() {
                p { class: "action-feedback action-feedback--success", role: "status",
                    {lot_update_notice_label(notice)}
                }
            }
            if page.auction.status == "draft" && page.capabilities.manual_intake {
                AdminIntakeForm {
                    world_id: page.auction.world_id.clone(),
                    reload_generation,
                }
            }
            if page.lots.is_empty() {
                p { class: "admin-panel__note", role: "status", {t!("admin-lots-empty")} }
            } else {
                div { class: "public-auction-catalog admin-lots__table-wrap",
                    table {
                        thead {
                            tr {
                                th { class: "admin-lots__col-code", {t!("admin-lots-code")} }
                                th { class: "admin-lots__col-equipment", {t!("admin-lots-equipment")} }
                                th { class: "admin-lots__col-seller", {t!("admin-lots-seller")} }
                                th { class: "admin-lots__col-submission", {t!("admin-lots-submission")} }
                                th { class: "admin-lots__col-status", {t!("admin-lots-status")} }
                                th { class: "admin-lots__col-start", {t!("admin-lots-start-price")} }
                                th { class: "admin-lots__col-current", {t!("admin-lots-price")} }
                                th { {t!("admin-lots-actions")} }
                            }
                        }
                        tbody {
                            for lot in page.lots.clone() {
                                AdminLotRow { lot, selected_lot, notice, reload_generation, allow_lot_update }
                            }
                        }
                    }
                }
            }
            AdminPager {
                page: page.pagination.page,
                page_size: page.pagination.page_size,
                total: page.pagination.total,
                label: t!("admin-lots-pagination"),
                on_page_change: move |next_page| page_number.set(next_page),
            }
            if let Some((lot_id, lot_number)) = selected_lot() {
                AdminProxyHistory { lot_id, lot_number, selected_lot, reload_generation }
            }
        }
    }
}

#[component]
fn AdminLotLoadError(mut reload_generation: Signal<u64>) -> Element {
    rsx! {
        section { class: "admin-panel", role: "alert", aria_labelledby: "admin-lots-error-title",
            h1 { id: "admin-lots-error-title", {t!("page-auction-lots-title", auction: "…")} }
            p { class: "admin-lots__back",
                Link { to: Route::Auctions {}, {t!("admin-lots-back")} }
            }
            p { class: "admin-panel__note", {t!("admin-lots-error")} }
            button {
                r#type: "button",
                onclick: move |_| reload_generation.set(reload_generation().saturating_add(1)),
                {t!("admin-lots-retry")}
            }
        }
    }
}

#[component]
fn AdminLotRow(
    lot: AdminLot,
    selected_lot: Signal<Option<(String, i32)>>,
    mut notice: Signal<Option<LotUpdateNotice>>,
    reload_generation: Signal<u64>,
    allow_lot_update: bool,
) -> Element {
    let lot_id = lot.id.clone();
    let can_update = allow_lot_update && lot.status == "pending";
    let mut start_price = use_signal(|| lot.start_price.to_string());
    let mut dialog = use_signal(|| None::<LotDialog>);
    let busy = use_signal(|| false);
    let mut failed = use_signal(|| false);
    let mut invalid_price = use_signal(|| false);
    let mut command_state = use_signal(|| None::<(String, Uuid)>);
    let lot_id_for_price = lot.id.clone();
    let lot_id_for_withdraw = lot.id.clone();
    let lot_code = admin_lot_code_for(&lot.equipment_category, lot.lot_number);
    let expected_state_version = lot.state_version;
    rsx! {
        tr { class: "admin-lots__row",
            td { "data-label": t!("admin-lots-code"),
                h4 { class: "admin-visually-hidden",
                    {t!(
                        "admin-lots-row-heading",
                        number: lot.lot_number,
                        name: lot.equipment_name.clone()
                    )}
                }
                "{lot_code}"
            }
            td { "data-label": t!("admin-lots-equipment"),
                TruncatedText {
                    value: equipment_details(&lot.equipment_name, &lot.equipment_attributes),
                    display: lot.equipment_name.clone(),
                }
            }
            td { "data-label": t!("admin-lots-seller"),
                TruncatedText { value: lot.seller.clone().unwrap_or_default() }
            }
            td { "data-label": t!("admin-lots-submission"),
                if let Some(submission_id) = lot.submission_id.clone() {
                    TruncatedText {
                        value: submission_id.clone(),
                        display: submission_id_display(&submission_id),
                        size: "compact",
                    }
                }
            }
            td { "data-label": t!("admin-lots-status"),
                span { class: "admin-lots__status admin-lots__status--{lot.status}",
                    {lot_status_label(&lot.status)}
                }
                if let Some(slow_due_at) = lot.slow_due_at.clone() {
                    span { class: "admin-lots__slow-schedule",
                        title: lot.slow_authorization_id.clone().unwrap_or_default(),
                        {t!("admin-lots-slow-schedule", generation: lot.slow_generation, time: format_admin_time(&slow_due_at))}
                    }
                }
            }
            td { class: "public-auction-catalog__amount public-auction-catalog__number", "data-label": t!("admin-lots-start-price"), "{format_amount(lot.start_price)}" }
            td { class: "public-auction-catalog__amount public-auction-catalog__number", "data-label": t!("admin-lots-price"), "{format_amount(lot.current_price)}" }
            td { "data-label": t!("admin-lots-actions"),
                div { class: "admin-lots__actions",
                    if can_update {
                        button {
                            class: "admin-btn-primary",
                            r#type: "button",
                            disabled: busy(),
                            onclick: move |_| {
                                start_price.set(lot.start_price.to_string());
                                failed.set(false);
                                invalid_price.set(false);
                                notice.set(None);
                                dialog.set(Some(LotDialog::Reprice));
                            },
                            {t!("admin-lots-reprice")}
                        }
                        button {
                            class: "admin-btn-danger",
                            r#type: "button",
                            disabled: busy(),
                            onclick: move |_| {
                                failed.set(false);
                                invalid_price.set(false);
                                notice.set(None);
                                dialog.set(Some(LotDialog::Withdraw));
                            },
                            {t!("admin-lots-withdraw")}
                        }
                        if failed() {
                            span {
                                role: "alert",
                                class: "action-feedback action-feedback--error",
                                {t!("admin-lots-update-error")}
                            }
                        }
                    }
                    button {
                        r#type: "button",
                        disabled: busy(),
                        onclick: move |_| selected_lot.set(Some((lot_id.clone(), lot.lot_number))),
                        {t!("admin-lots-proxy-open")}
                    }
                }
            }
        }
        if let Some(current_dialog) = dialog() {
            tr { class: "admin-lots__dialog-row",
                td { colspan: "8",
                    div { class: "admin-lots__dialog-backdrop", role: "presentation",
                        section {
                            class: "admin-lots__dialog",
                            role: "dialog",
                            aria_modal: "true",
                            aria_labelledby: "admin-lots-dialog-title",
                            tabindex: "-1",
                            onkeydown: move |event| {
                                if event.key() == Key::Escape && !busy() {
                                    dialog.set(None);
                                    failed.set(false);
                                    invalid_price.set(false);
                                }
                            },
                            h2 { id: "admin-lots-dialog-title",
                                if current_dialog == LotDialog::Reprice {
                                    {t!("admin-lots-reprice-title", code: lot_code.clone())}
                                } else {
                                    {t!("admin-lots-withdraw-title", code: lot_code.clone())}
                                }
                            }
                            form {
                                class: "admin-lots__dialog-form",
                                onsubmit: move |event| {
                                    event.prevent_default();
                                    if busy() {
                                        return;
                                    }
                                    let price = if current_dialog == LotDialog::Reprice {
                                        let Ok(price) = start_price().parse::<i64>() else {
                                            invalid_price.set(true);
                                            return;
                                        };
                                        if price <= 0 {
                                            invalid_price.set(true);
                                            return;
                                        }
                                        Some(price)
                                    } else {
                                        None
                                    };
                                    invalid_price.set(false);
                                    notice.set(None);
                                    let lot_id = if current_dialog == LotDialog::Reprice {
                                        lot_id_for_price.clone()
                                    } else {
                                        lot_id_for_withdraw.clone()
                                    };
                                    let action = if current_dialog == LotDialog::Reprice {
                                        AdminLotUpdateAction::SetStartPrice
                                    } else {
                                        AdminLotUpdateAction::Withdraw
                                    };
                                    let action_key = if current_dialog == LotDialog::Reprice {
                                        "set_start_price"
                                    } else {
                                        "withdraw"
                                    };
                                    let signature = format!(
                                        "{action_key}:{expected_state_version}:{}",
                                        price.map_or_else(String::new, |value| value.to_string())
                                    );
                                    let command_id = match command_state() {
                                        Some((current, command_id)) if current == signature => command_id,
                                        _ => {
                                            let command_id = Uuid::new_v4();
                                            command_state.set(Some((signature, command_id)));
                                            command_id
                                        }
                                    };
                                    let mut busy = busy;
                                    let mut failed = failed;
                                    let mut dialog = dialog;
                                    let mut notice = notice;
                                    let mut reload_generation = reload_generation;
                                    let success_notice = if current_dialog == LotDialog::Reprice {
                                        LotUpdateNotice::Repriced
                                    } else {
                                        LotUpdateNotice::Withdrawn
                                    };
                                    busy.set(true);
                                    spawn(async move {
                                        match update_admin_lot(
                                            command_id,
                                            lot_id,
                                            expected_state_version,
                                            action,
                                            price,
                                        )
                                        .await
                                        {
                                            Ok(_) => {
                                                failed.set(false);
                                                dialog.set(None);
                                                command_state.set(None);
                                                notice.set(Some(success_notice));
                                                reload_generation
                                                    .set(reload_generation().saturating_add(1));
                                            }
                                            Err(_) => failed.set(true),
                                        }
                                        busy.set(false);
                                    });
                                },
                                if current_dialog == LotDialog::Reprice {
                                    label {
                                        {t!("admin-lots-start-price-label")}
                                        input {
                                            r#type: "number",
                                            min: "1",
                                            step: "1",
                                            required: true,
                                            disabled: busy(),
                                            autofocus: true,
                                            value: start_price(),
                                            oninput: move |event| {
                                                start_price.set(event.value());
                                                failed.set(false);
                                                invalid_price.set(false);
                                            },
                                        }
                                    }
                                } else {
                                    p {
                                        {t!(
                                            "admin-lots-withdraw-warning",
                                            seller: lot.seller.clone().unwrap_or_else(|| t!("value-empty"))
                                        )}
                                    }
                                }
                                if invalid_price() {
                                    p { class: "action-feedback action-feedback--error", role: "alert",
                                        {t!("admin-lots-invalid-price")}
                                    }
                                }
                                if failed() {
                                    p { class: "action-feedback action-feedback--error", role: "alert",
                                        {t!("admin-lots-update-error")}
                                    }
                                }
                                div { class: "admin-lots__dialog-actions",
                                    button {
                                        r#type: "button",
                                        disabled: busy(),
                                        autofocus: current_dialog == LotDialog::Withdraw,
                                        onclick: move |_| {
                                            dialog.set(None);
                                            failed.set(false);
                                            invalid_price.set(false);
                                        },
                                        {t!("admin-lots-action-cancel")}
                                    }
                                    button {
                                        class: if current_dialog == LotDialog::Withdraw { "admin-btn-danger" } else { "admin-btn-primary" },
                                        r#type: "submit",
                                        disabled: busy(),
                                        if busy() {
                                            {t!("admin-action-working")}
                                        } else if current_dialog == LotDialog::Reprice {
                                            {t!("admin-lots-reprice-save")}
                                        } else {
                                            {t!("admin-lots-withdraw")}
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
}

#[component]
fn AdminProxyHistory(
    lot_id: String,
    lot_number: i32,
    mut selected_lot: Signal<Option<(String, i32)>>,
    mut reload_generation: Signal<u64>,
) -> Element {
    let page_number = use_signal(|| 1_i64);
    let events = use_resource({
        let lot_id = lot_id.clone();
        move || {
            let _ = reload_generation();
            fetch_admin_proxy_events(lot_id.clone(), page_number())
        }
    });

    let content = match &*events.read_unchecked() {
        None => rsx! {
            p { class: "admin-panel__note", role: "status", {t!("admin-lots-proxy-loading")} }
        },
        Some(Err(_)) => rsx! {
            div {
                p { class: "admin-panel__note", role: "alert", {t!("admin-lots-proxy-error")} }
                button {
                    r#type: "button",
                    onclick: move |_| reload_generation.set(reload_generation().saturating_add(1)),
                    {t!("admin-lots-retry")}
                }
            }
        },
        Some(Ok(page)) => rsx! {
            AdminProxyHistoryTable { page: page.clone(), page_number }
        },
    };

    rsx! {
        div { class: "admin-lots__dialog-backdrop", role: "presentation",
            section {
                class: "admin-lots__dialog admin-lots__proxy-dialog",
                role: "dialog",
                aria_modal: "true",
                aria_labelledby: "admin-proxy-history-title",
                tabindex: "-1",
                onkeydown: move |event| {
                    if event.key() == Key::Escape {
                        selected_lot.set(None);
                    }
                },
                h2 { id: "admin-proxy-history-title",
                    {t!("admin-lots-proxy-title", lot: lot_number)}
                }
                {content}
                div { class: "admin-lots__dialog-actions",
                    button {
                        r#type: "button",
                        autofocus: true,
                        onclick: move |_| selected_lot.set(None),
                        {t!("admin-lots-proxy-close")}
                    }
                }
            }
        }
    }
}

#[component]
fn AdminProxyHistoryTable(page: AdminProxyEventPage, page_number: Signal<i64>) -> Element {
    let current_page = page.pagination.page.unwrap_or(page_number()).max(1);
    let page_size = page.pagination.page_size.max(1);
    let total = proxy_event_total(&page, current_page);

    rsx! {
        div { class: "admin-lots__proxy-content",
            if page.proxy_events.is_empty() {
                p { class: "admin-panel__note", {t!("admin-lots-proxy-empty")} }
            } else {
                div { class: "admin-lots__proxy-table-wrap",
                    table { class: "admin-lots__proxy-table",
                        tbody {
                            for event in page.proxy_events.clone() {
                                AdminProxyHistoryRow { event }
                            }
                        }
                    }
                }
            }
            AdminPager {
                page: current_page,
                page_size,
                total,
                label: t!("admin-lots-proxy-pagination"),
                on_page_change: move |next_page| page_number.set(next_page),
            }
        }
    }
}

fn proxy_event_total(page: &AdminProxyEventPage, current_page: i64) -> i64 {
    let page_size = page.pagination.page_size.max(1);
    if let Some(total) = page.pagination.total {
        return total;
    }

    let has_more =
        page.pagination.has_more.unwrap_or_else(|| page.pagination.next_cursor.is_some());
    let loaded = current_page.saturating_mul(page_size);
    if has_more {
        loaded.saturating_add(1)
    } else {
        current_page
            .saturating_sub(1)
            .saturating_mul(page_size)
            .saturating_add(page.proxy_events.len().try_into().unwrap_or(i64::MAX))
    }
}

#[component]
fn AdminProxyHistoryRow(event: AdminProxyEvent) -> Element {
    rsx! {
        tr {
            td { strong { TruncatedText { value: event.username.clone() } } }
            td {
                {t!("admin-lots-proxy-amount", amount: format_amount(event.maximum_amount))}
            }
            td { "{proxy_mode_label(&event.mode)}" }
            td { "{proxy_status_label(&event.status)}" }
            td { title: event.authorization_id.clone(),
                {t!(
                    "admin-lots-proxy-identity",
                    authorization_sequence: event.authorization_sequence,
                    usage_number: event.usage_number
                )}
            }
            td { time { datetime: event.created_at.clone(), "{format_admin_time(&event.created_at)}" } }
        }
    }
}

fn lot_status_label(status: &str) -> String {
    match status {
        "pending" => t!("admin-lots-status-pending"),
        "live" => t!("admin-lots-status-live"),
        "sold" => t!("admin-lots-status-sold"),
        "unsold" => t!("admin-lots-status-unsold"),
        "withdrawn" => t!("admin-lots-status-withdrawn"),
        _ => status.to_owned(),
    }
}

fn lot_update_notice_label(notice: LotUpdateNotice) -> String {
    match notice {
        LotUpdateNotice::Repriced => t!("admin-lots-reprice-done"),
        LotUpdateNotice::Withdrawn => t!("admin-lots-withdraw-done"),
    }
}

/// 按旧版拍卖页面的约定生成稳定的拍品编码。
fn admin_lot_code_for(category: &str, lot_number: i32) -> String {
    let prefix = match category {
        "One-Handed" => "One",
        "Two-Handed" => "Two",
        "Staff" => "Sta",
        "Shield" => "Shd",
        "Cloth" => "Clo",
        "Light" => "Lig",
        "Heavy" => "Hea",
        _ => "Lot",
    };
    format!("{prefix}{lot_number:02}")
}

/// 将提交编号压缩为旧版列表中的可读后缀，同时保留完整值供悬浮提示查看。
fn submission_id_display(value: &str) -> String {
    let suffix = value
        .rsplit_once(|character: char| !character.is_ascii_digit())
        .map(|(_, digits)| digits)
        .filter(|digits| !digits.is_empty())
        .unwrap_or(value)
        .trim_start_matches('0');
    let display = if suffix.is_empty() {
        value.chars().rev().take(8).collect::<String>().chars().rev().collect()
    } else {
        suffix.to_owned()
    };
    format!("#{display}")
}

fn proxy_mode_label(mode: &str) -> String {
    match mode {
        "fast" => t!("admin-lots-proxy-mode-fast"),
        "slow" => t!("admin-lots-proxy-mode-slow"),
        _ => mode.to_owned(),
    }
}

fn proxy_status_label(status: &str) -> String {
    match status {
        "active" => t!("admin-lots-proxy-status-active"),
        "final_fast" => t!("admin-lots-proxy-status-final-fast"),
        "exhausted" => t!("admin-lots-proxy-status-exhausted"),
        "frozen" => t!("admin-lots-proxy-status-frozen"),
        "closed" => t!("admin-lots-proxy-status-closed"),
        _ => status.to_owned(),
    }
}

fn equipment_details(name: &str, value: &serde_json::Value) -> String {
    let details = serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
    format!("{name}\n{details}")
}
