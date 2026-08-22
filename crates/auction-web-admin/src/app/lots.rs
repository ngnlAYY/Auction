//! Admin 拍卖 lot 页面。

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api::{
    AdminIntakeResult, AdminLot, AdminLotPage, AdminLotUpdateAction, AdminProxyEvent,
    AdminProxyEventPage, add_admin_intake, fetch_admin_lots, fetch_admin_proxy_events,
    update_admin_lot,
};

use super::AdminPanel;

/// 加载并展示一个拍卖的 Admin lot 安全投影。
pub(super) fn page(auction_id: String) -> Element {
    let reload_generation = use_signal(|| 0_u64);
    let lots = use_resource({
        let auction_id = auction_id.clone();
        move || {
            let _ = reload_generation();
            fetch_admin_lots(auction_id.clone())
        }
    });

    match &*lots.read_unchecked() {
        None => rsx! {
            AdminPanel {
                title: t!("page-auction-lots-title", auction: auction_id.clone()),
                detail: t!("admin-lots-loading"),
            }
        },
        Some(Err(_)) => rsx! {
            AdminPanel {
                title: t!("page-auction-lots-title", auction: auction_id.clone()),
                detail: t!("admin-lots-error"),
            }
        },
        Some(Ok(page)) => rsx! {
            AdminLotTable { page: page.clone(), reload_generation }
        },
    }
}

#[component]
fn AdminLotTable(page: AdminLotPage, reload_generation: Signal<u64>) -> Element {
    let selected_lot_id = use_signal(|| None::<String>);
    let allow_lot_update = page.auction.status == "draft";

    rsx! {
        section { class: "admin-panel", aria_labelledby: "admin-lots-title",
            h1 { id: "admin-lots-title",
                {t!("page-auction-lots-title", auction: page.auction.id.clone())}
            }
            p { class: "admin-panel__note",
                {t!(
                    "admin-lots-summary",
                    world: page.auction.world_id.clone(),
                    status: page.auction.status.clone(),
                    total: page.pagination.total
                )}
            }
            if page.auction.status == "draft" {
                AdminIntakeForm {
                    world_id: page.auction.world_id.clone(),
                    reload_generation,
                }
            }
            div { class: "public-auction-catalog",
                table {
                    thead {
                        tr {
                            th { {t!("admin-lots-number")} }
                            th { {t!("admin-lots-status")} }
                            th { {t!("admin-lots-equipment")} }
                            th { {t!("admin-lots-category")} }
                            th { {t!("admin-lots-price")} }
                            th { {t!("admin-lots-seller")} }
                            th { {t!("admin-lots-source")} }
                            th { {t!("admin-lots-updated")} }
                            th { {t!("admin-lots-actions")} }
                        }
                    }
                    tbody {
                        for lot in page.lots.clone() {
                            AdminLotRow { lot, selected_lot_id, reload_generation, allow_lot_update }
                        }
                    }
                }
            }
            if let Some(lot_id) = selected_lot_id() {
                AdminProxyHistory { lot_id }
            }
        }
    }
}

#[component]
fn AdminIntakeForm(world_id: String, reload_generation: Signal<u64>) -> Element {
    let mut source_kind = use_signal(|| "url".to_owned());
    let mut equipment_url = use_signal(String::new);
    let mut equipment_json = use_signal(String::new);
    let mut start_price = use_signal(String::new);
    let mut player_id = use_signal(String::new);
    let busy = use_signal(|| false);
    let result = use_signal(|| None::<AdminIntakeResult>);
    let mut failed = use_signal(|| false);
    let empty_value = t!("value-empty");

    rsx! {
        section { class: "admin-panel", aria_labelledby: "admin-intake-title",
            h2 { id: "admin-intake-title", {t!("admin-intake-title")} }
            p { class: "admin-panel__note", {t!("admin-intake-description")} }
            div { class: "admin-panel__actions",
                label {
                    {t!("admin-intake-source")}
                    select {
                        value: source_kind(),
                        onchange: move |event| {
                            source_kind.set(event.value());
                            failed.set(false);
                        },
                        option { value: "url", {t!("admin-intake-source-url")} }
                        option { value: "json", {t!("admin-intake-source-json")} }
                    }
                }
                if source_kind() == "url" {
                    label {
                        {t!("admin-intake-equipment-url")}
                        input {
                            r#type: "url",
                            maxlength: "2048",
                            value: equipment_url(),
                            oninput: move |event| equipment_url.set(event.value()),
                        }
                    }
                } else {
                    label { class: "admin-panel__wide-field",
                        {t!("admin-intake-equipment-json")}
                        textarea {
                            rows: "8",
                            maxlength: "65536",
                            value: equipment_json(),
                            oninput: move |event| equipment_json.set(event.value()),
                        }
                        span { class: "admin-panel__note", {t!("admin-intake-json-hint")} }
                    }
                }
                label {
                    {t!("admin-intake-start-price")}
                    input {
                        r#type: "number",
                        min: "1",
                        value: start_price(),
                        oninput: move |event| start_price.set(event.value()),
                    }
                }
                label {
                    {t!("admin-intake-player-id")}
                    input {
                        maxlength: "256",
                        value: player_id(),
                        oninput: move |event| player_id.set(event.value()),
                    }
                }
                button {
                    r#type: "button",
                    disabled: busy(),
                    onclick: move |_| {
                        let Ok(start_price) = start_price().parse::<i64>() else {
                            failed.set(true);
                            return;
                        };
                        let world_id = world_id.clone();
                        let source_kind = source_kind();
                        let equipment_url = equipment_url();
                        let equipment_json = equipment_json();
                        let player_id = player_id();
                        let mut busy = busy;
                        let mut result = result;
                        let mut failed = failed;
                        let mut reload_generation = reload_generation;
                        spawn(async move {
                            busy.set(true);
                            match add_admin_intake(
                                world_id,
                                source_kind,
                                equipment_url,
                                equipment_json,
                                start_price,
                                player_id,
                            )
                            .await
                            {
                                Ok(value) => {
                                    result.set(Some(value));
                                    failed.set(false);
                                    reload_generation.set(reload_generation().saturating_add(1));
                                }
                                Err(_) => {
                                    result.set(None);
                                    failed.set(true);
                                }
                            }
                            busy.set(false);
                        });
                    },
                    {t!("admin-intake-submit")}
                }
            }
            if failed() {
                p { role: "alert", class: "admin-panel__note", {t!("admin-intake-error")} }
            }
            if let Some(value) = result() {
                p { class: "admin-panel__note",
                    {t!(
                        "admin-intake-result",
                        lot: value.lot_number,
                        category: value.equipment_category,
                        level: value
                            .equipment_level
                            .map(|level| level.to_string())
                            .unwrap_or_else(|| empty_value.clone())
                    )}
                }
            }
        }
    }
}

#[component]
fn AdminLotRow(
    lot: AdminLot,
    selected_lot_id: Signal<Option<String>>,
    reload_generation: Signal<u64>,
    allow_lot_update: bool,
) -> Element {
    let lot_id = lot.id.clone();
    let can_update = allow_lot_update && lot.status == "pending";
    let mut start_price = use_signal(|| lot.start_price.to_string());
    let busy = use_signal(|| false);
    let mut failed = use_signal(|| false);
    let lot_id_for_price = lot.id.clone();
    let lot_id_for_withdraw = lot.id.clone();
    let expected_state_version = lot.state_version;
    rsx! {
        tr {
            td { "{lot.lot_number}" }
            td { "{lot.status}" }
            td { "{lot.equipment_name}" }
            td { "{lot.equipment_category}" }
            td { "{lot.current_price}" }
            td { {lot.seller.unwrap_or_else(|| t!("value-empty"))} }
            td {
                {lot.submission_source_kind.unwrap_or_else(|| t!("value-empty"))}
            }
            td { "{lot.updated_at}" }
            td {
                button {
                    r#type: "button",
                    onclick: move |_| selected_lot_id.set(Some(lot_id.clone())),
                    {t!("admin-lots-proxy-open")}
                }
                if can_update {
                    label {
                        {t!("admin-lots-start-price-label")}
                        input {
                            r#type: "number",
                            min: "1",
                            value: start_price(),
                            oninput: move |event| start_price.set(event.value()),
                        }
                    }
                    button {
                        r#type: "button",
                        disabled: busy(),
                        onclick: move |_| {
                            let Ok(price) = start_price().parse::<i64>() else {
                                failed.set(true);
                                return;
                            };
                            let lot_id = lot_id_for_price.clone();
                            let mut busy = busy;
                            let mut failed = failed;
                            let mut reload_generation = reload_generation;
                            spawn(async move {
                                busy.set(true);
                                match update_admin_lot(
                                    lot_id,
                                    expected_state_version,
                                    AdminLotUpdateAction::SetStartPrice,
                                    Some(price),
                                )
                                .await
                                {
                                    Ok(_) => {
                                        failed.set(false);
                                        reload_generation
                                            .set(reload_generation().saturating_add(1));
                                    }
                                    Err(_) => failed.set(true),
                                }
                                busy.set(false);
                            });
                        },
                        {t!("admin-lots-reprice")}
                    }
                    button {
                        r#type: "button",
                        disabled: busy(),
                        onclick: move |_| {
                            let lot_id = lot_id_for_withdraw.clone();
                            let mut busy = busy;
                            let mut failed = failed;
                            let mut reload_generation = reload_generation;
                            spawn(async move {
                                busy.set(true);
                                match update_admin_lot(
                                    lot_id,
                                    expected_state_version,
                                    AdminLotUpdateAction::Withdraw,
                                    None,
                                )
                                .await
                                {
                                    Ok(_) => {
                                        failed.set(false);
                                        reload_generation
                                            .set(reload_generation().saturating_add(1));
                                    }
                                    Err(_) => failed.set(true),
                                }
                                busy.set(false);
                            });
                        },
                        {t!("admin-lots-withdraw")}
                    }
                    if failed() {
                        span { role: "alert", {t!("admin-lots-update-error")} }
                    }
                }
            }
        }
    }
}

#[component]
fn AdminProxyHistory(lot_id: String) -> Element {
    let page_number = use_signal(|| 1_i64);
    let events = use_resource({
        let lot_id = lot_id.clone();
        move || fetch_admin_proxy_events(lot_id.clone(), page_number())
    });

    match &*events.read_unchecked() {
        None => rsx! {
            section { class: "admin-panel", aria_labelledby: "admin-proxy-history-title",
                h2 { id: "admin-proxy-history-title", {t!("admin-lots-proxy-title")} }
                p { class: "admin-panel__note", {t!("admin-lots-proxy-loading")} }
            }
        },
        Some(Err(_)) => rsx! {
            section { class: "admin-panel", aria_labelledby: "admin-proxy-history-title",
                h2 { id: "admin-proxy-history-title", {t!("admin-lots-proxy-title")} }
                p { class: "admin-panel__note", {t!("admin-lots-proxy-error")} }
            }
        },
        Some(Ok(page)) => rsx! {
            AdminProxyHistoryTable { page: page.clone(), page_number }
        },
    }
}

#[component]
fn AdminProxyHistoryTable(page: AdminProxyEventPage, mut page_number: Signal<i64>) -> Element {
    let current_page = page.pagination.page.unwrap_or_else(&*page_number);
    let page_size = page.pagination.page_size.max(1);
    let total = page.pagination.total.unwrap_or(0);
    let has_next = current_page.checked_mul(page_size).is_some_and(|end| end < total);

    rsx! {
        section { class: "admin-panel", aria_labelledby: "admin-proxy-history-title",
            h2 { id: "admin-proxy-history-title", {t!("admin-lots-proxy-title")} }
            p { class: "admin-panel__note",
                {t!("admin-lots-proxy-summary", total: total)}
            }
            if page.proxy_events.is_empty() {
                p { class: "admin-panel__note", {t!("admin-lots-proxy-empty")} }
            } else {
                div { class: "public-auction-catalog",
                    table {
                        thead {
                            tr {
                                th { {t!("admin-lots-proxy-user")} }
                                th { {t!("admin-lots-proxy-mode")} }
                                th { {t!("admin-lots-proxy-maximum")} }
                                th { {t!("admin-lots-proxy-status")} }
                                th { {t!("admin-lots-proxy-usage")} }
                                th { {t!("admin-lots-proxy-version")} }
                                th { {t!("admin-lots-proxy-command")} }
                                th { {t!("admin-lots-proxy-created")} }
                            }
                        }
                        tbody {
                            for event in page.proxy_events.clone() {
                                AdminProxyHistoryRow { event }
                            }
                        }
                    }
                }
            }
            nav { class: "admin-panel__actions", aria_label: t!("admin-lots-proxy-pagination"),
                button {
                    r#type: "button",
                    disabled: current_page <= 1,
                    onclick: move |_| {
                        if current_page > 1 {
                            page_number.set(current_page - 1);
                        }
                    },
                    {t!("admin-lots-proxy-previous")}
                }
                span { {t!("admin-lots-proxy-page", page: current_page)} }
                button {
                    r#type: "button",
                    disabled: !has_next,
                    onclick: move |_| {
                        if has_next {
                            page_number.set(current_page + 1);
                        }
                    },
                    {t!("admin-lots-proxy-next")}
                }
            }
        }
    }
}

#[component]
fn AdminProxyHistoryRow(event: AdminProxyEvent) -> Element {
    rsx! {
        tr {
            td { "{event.username}" }
            td { "{event.mode}" }
            td { class: "public-auction-catalog__amount public-auction-catalog__number",
                "{event.maximum_amount}"
            }
            td { "{event.status}" }
            td { "{event.usage_number}/{event.authorization_sequence}" }
            td { "{event.state_version}" }
            td { "{event.command_id}" }
            td { time { datetime: event.created_at.clone(), "{event.created_at}" } }
        }
    }
}
