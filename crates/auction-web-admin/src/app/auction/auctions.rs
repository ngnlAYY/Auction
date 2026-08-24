//! Admin 拍卖列表页面。
//!
//! 当前页面展示 PostgreSQL 完整档的只读生命周期快照；写命令仍通过独立的版本化 API 接入。

use dioxus::{html::Key, prelude::*};
use dioxus_i18n::t;
use uuid::Uuid;

#[cfg(target_arch = "wasm32")]
use gloo_timers::future::TimeoutFuture;

use crate::api::{
    AdminAuction, AdminAuctionCapabilities, AdminAuctionLifecycleAction, AdminAuctionPage,
    create_admin_auction, execute_admin_auction_lifecycle, fetch_admin_auctions,
};

use super::intake::AdminIntakeForm;
use super::pagination::AdminPager;
use super::{AdminPanel, Route, format_admin_time};

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
            AdminAuctionLoadError { reload_generation }
        },
        Some(Ok(page)) => rsx! {
            div { class: "admin-auctions",
                AdminAuctionTable {
                    page: page.clone(),
                    capabilities: page.capabilities.clone(),
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
fn AdminAuctionLoadError(mut reload_generation: Signal<u64>) -> Element {
    rsx! {
        section {
            class: "admin-panel",
            role: "alert",
            aria_labelledby: "admin-auctions-error-title",
            h1 { id: "admin-auctions-error-title", {t!("page-auctions-title")} }
            p { class: "admin-panel__note", {t!("admin-auctions-error")} }
            button {
                r#type: "button",
                onclick: move |_| reload_generation.set(reload_generation().saturating_add(1)),
                {t!("admin-auctions-retry")}
            }
        }
    }
}

#[component]
fn AdminAuctionCreateForm(reload_generation: Signal<u64>) -> Element {
    let mut open = use_signal(|| false);

    rsx! {
        if open() {
            div { class: "admin-auctions__dialog-backdrop", role: "presentation",
                section {
                    class: "admin-auctions__dialog",
                    role: "dialog",
                    aria_modal: "true",
                    aria_labelledby: "admin-auctions-create-title",
                    AdminAuctionCreateFields { reload_generation, open }
                }
            }
        } else {
            button {
                class: "admin-auctions__create-opener admin-btn-primary",
                r#type: "button",
                onclick: move |_| open.set(true),
                {t!("admin-auctions-create-submit")}
            }
        }
    }
}

#[component]
fn AdminAuctionCreateFields(reload_generation: Signal<u64>, mut open: Signal<bool>) -> Element {
    let mut world_id = use_signal(|| "main".to_owned());
    let mut sequence = use_signal(String::new);
    let mut preview_scheduled_at = use_signal(String::new);
    let mut scheduled_open_at = use_signal(String::new);
    let mut scheduled_end_at = use_signal(String::new);
    let mut command_id_state = use_signal(|| None::<Uuid>);
    let mut auction_id_state = use_signal(|| None::<Uuid>);
    let busy = use_signal(|| false);
    let mut feedback = use_signal(|| None::<bool>);
    let mut validation_error = use_signal(|| false);

    rsx! {
        section {
            class: "admin-panel",
            aria_labelledby: "admin-auctions-create-title",
            aria_busy: busy().to_string(),
            tabindex: "-1",
            onkeydown: move |event| {
                if event.key() == Key::Escape && !busy() {
                    open.set(false);
                }
            },
            h2 { id: "admin-auctions-create-title", {t!("admin-auctions-create-title")} }
            p { class: "admin-panel__note", {t!("admin-auctions-create-timezone")} }
            form {
                class: "admin-panel__actions admin-auctions__create-form",
                onsubmit: move |event| {
                    event.prevent_default();
                    if busy() {
                        return;
                    }
                    let Ok(sequence_value) = sequence().trim().parse::<i64>() else {
                        validation_error.set(true);
                        feedback.set(Some(false));
                        return;
                    };
                    let world_id = world_id();
                    let preview_scheduled_at = preview_scheduled_at();
                    let scheduled_open_at = scheduled_open_at();
                    let scheduled_end_at = scheduled_end_at();
                    if sequence_value <= 0
                        || !valid_auction_schedule(
                            &preview_scheduled_at,
                            &scheduled_open_at,
                            &scheduled_end_at,
                        )
                    {
                        validation_error.set(true);
                        feedback.set(Some(false));
                        return;
                    }
                    let command_id = command_id_state().unwrap_or_else(|| {
                        let value = Uuid::new_v4();
                        command_id_state.set(Some(value));
                        value
                    });
                    let auction_id = auction_id_state().unwrap_or_else(|| {
                        let value = Uuid::new_v4();
                        auction_id_state.set(Some(value));
                        value
                    });
                    let mut busy = busy;
                    let mut feedback = feedback;
                    let mut validation_error = validation_error;
                    let mut reload_generation = reload_generation;
                    let mut open = open;
                    busy.set(true);
                    spawn(async move {
                        let result = create_admin_auction(
                            command_id,
                            auction_id,
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
                                validation_error.set(false);
                                reload_generation.set(reload_generation().saturating_add(1));
                                open.set(false);
                            }
                            Err(_) => feedback.set(Some(false)),
                        }
                        busy.set(false);
                    });
                },
                label {
                    r#for: "admin-auctions-create-world",
                    {t!("admin-auctions-create-world")}
                    select {
                        id: "admin-auctions-create-world",
                        disabled: busy(),
                        autofocus: true,
                        value: world_id(),
                        onchange: move |event| {
                            world_id.set(event.value());
                            command_id_state.set(None);
                            auction_id_state.set(None);
                            validation_error.set(false);
                            feedback.set(None);
                        },
                        option { value: "main", {t!("admin-auctions-world-main")} }
                        option { value: "isekai", {t!("admin-auctions-world-isekai")} }
                    }
                }
                label {
                    r#for: "admin-auctions-create-sequence",
                    {t!("admin-auctions-create-sequence")}
                    input {
                        id: "admin-auctions-create-sequence",
                        r#type: "number",
                        min: "1",
                        required: true,
                        disabled: busy(),
                        value: sequence(),
                        oninput: move |event| {
                            sequence.set(event.value());
                            command_id_state.set(None);
                            auction_id_state.set(None);
                            validation_error.set(false);
                            feedback.set(None);
                        },
                    }
                }
                label {
                    r#for: "admin-auctions-create-preview",
                    {t!("admin-auctions-create-preview")}
                    input {
                        id: "admin-auctions-create-preview",
                        r#type: "datetime-local",
                        required: true,
                        disabled: busy(),
                        value: preview_scheduled_at(),
                        oninput: move |event| {
                            preview_scheduled_at.set(event.value());
                            command_id_state.set(None);
                            auction_id_state.set(None);
                            validation_error.set(false);
                            feedback.set(None);
                        },
                    }
                }
                label {
                    r#for: "admin-auctions-create-open",
                    {t!("admin-auctions-create-open")}
                    input {
                        id: "admin-auctions-create-open",
                        r#type: "datetime-local",
                        required: true,
                        disabled: busy(),
                        value: scheduled_open_at(),
                        oninput: move |event| {
                            scheduled_open_at.set(event.value());
                            command_id_state.set(None);
                            auction_id_state.set(None);
                            validation_error.set(false);
                            feedback.set(None);
                        },
                    }
                }
                label {
                    r#for: "admin-auctions-create-end",
                    {t!("admin-auctions-create-end")}
                    input {
                        id: "admin-auctions-create-end",
                        r#type: "datetime-local",
                        required: true,
                        disabled: busy(),
                        value: scheduled_end_at(),
                        oninput: move |event| {
                            scheduled_end_at.set(event.value());
                            command_id_state.set(None);
                            auction_id_state.set(None);
                            validation_error.set(false);
                            feedback.set(None);
                        },
                    }
                }
                button {
                    r#type: "submit",
                    disabled: busy(),
                    if busy() { {t!("admin-action-working")} } else { {t!("admin-auctions-create-submit")} }
                }
                button {
                    r#type: "button",
                    disabled: busy(),
                    onclick: move |_| open.set(false),
                    {t!("admin-auctions-create-close")}
                }
            }
            if let Some(success) = feedback() {
                p {
                    class: if success {
                        "admin-panel__note action-feedback action-feedback--success"
                    } else {
                        "admin-panel__note action-feedback action-feedback--error"
                    },
                    role: if success { "status" } else { "alert" },
                    {if success {
                        t!("admin-auctions-create-success")
                    } else if validation_error() {
                        t!("admin-auctions-create-invalid")
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
    capabilities: AdminAuctionCapabilities,
    mut world_filter: Signal<String>,
    mut status_filter: Signal<String>,
    mut page_number: Signal<i64>,
    reload_generation: Signal<u64>,
) -> Element {
    let clock_millis = use_signal(current_time_millis);
    let notice = use_signal(|| None::<AdminAuctionLifecycleAction>);
    #[cfg(target_arch = "wasm32")]
    let _clock_task = use_future(move || {
        let mut clock_millis = clock_millis;
        async move {
            loop {
                TimeoutFuture::new(1_000).await;
                clock_millis.set(current_time_millis());
            }
        }
    });
    rsx! {
        section { class: "admin-panel", aria_labelledby: "admin-auctions-title",
            h1 { id: "admin-auctions-title", {t!("page-auctions-title")} }
            div { class: "admin-panel__actions admin-auctions__filters",
                label {
                    class: "admin-auctions__world-filter",
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
                    class: "admin-auctions__status-filter",
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
                AdminAuctionCreateForm { reload_generation }
            }
            if let Some(action) = notice() {
                p { class: "action-feedback action-feedback--success", role: "status",
                    {lifecycle_action_success_label(action)}
                }
            }
            if capabilities.manual_intake && matches!(world_filter().as_str(), "main" | "isekai") {
                AdminIntakeForm {
                    world_id: world_filter(),
                    reload_generation,
                }
            }
            if page.auctions.is_empty() {
                p { class: "admin-panel__note", role: "status", {t!("admin-auctions-empty")} }
            } else {
                div { class: "public-auction-catalog admin-auctions__table-wrap",
                    table {
                        thead {
                            tr {
                                th { {t!("admin-auctions-sequence")} }
                                th { {t!("admin-auctions-world")} }
                                th { {t!("admin-auctions-status")} }
                                th { {t!("admin-auctions-open-at")} }
                                th { {t!("admin-auctions-end-at")} }
                                th { {t!("admin-auctions-readiness")} }
                                th { {t!("admin-auctions-actions")} }
                            }
                        }
                        tbody {
                            for auction in page.auctions {
                                AdminAuctionRow {
                                    auction,
                                    capabilities: capabilities.clone(),
                                    clock_millis,
                                    notice,
                                    reload_generation,
                                }
                            }
                        }
                    }
                }
            }
            AdminPager {
                page: page.pagination.page,
                page_size: page.pagination.page_size,
                total: page.pagination.total,
                label: t!("page-auctions-title"),
                on_page_change: move |next_page| page_number.set(next_page),
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

fn current_time_millis() -> i64 {
    #[cfg(target_arch = "wasm32")]
    {
        return js_sys::Date::new_0().get_time() as i64;
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        chrono::Utc::now().timestamp_millis()
    }
}

fn scheduled_time_millis(value: &str) -> Option<i64> {
    #[cfg(target_arch = "wasm32")]
    {
        let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_str(value));
        let milliseconds = date.get_time();
        return milliseconds.is_finite().then_some(milliseconds as i64);
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        chrono::DateTime::parse_from_rfc3339(value).ok().map(|value| value.timestamp_millis())
    }
}

fn valid_auction_schedule(preview: &str, open: &str, end: &str) -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        let Some(preview) = local_datetime_millis(preview) else {
            return false;
        };
        let Some(open) = local_datetime_millis(open) else {
            return false;
        };
        let Some(end) = local_datetime_millis(end) else {
            return false;
        };
        return preview >= js_sys::Date::new_0().get_time() && open >= preview && end > open;
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let Ok(preview) = chrono::NaiveDateTime::parse_from_str(preview, "%Y-%m-%dT%H:%M") else {
            return false;
        };
        let Ok(open) = chrono::NaiveDateTime::parse_from_str(open, "%Y-%m-%dT%H:%M") else {
            return false;
        };
        let Ok(end) = chrono::NaiveDateTime::parse_from_str(end, "%Y-%m-%dT%H:%M") else {
            return false;
        };
        preview >= chrono::Utc::now().naive_utc() && open >= preview && end > open
    }
}

#[cfg(target_arch = "wasm32")]
fn local_datetime_millis(value: &str) -> Option<f64> {
    // `datetime-local` 按浏览器本地时区解释；这里的时间戳必须与提交时的 UTC 转换使用同一
    // 规则，否则用户在非 UTC 时区创建拍卖时，前端校验与服务端看到的时间会产生偏差。
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_str(value));
    let milliseconds = date.get_time();
    milliseconds.is_finite().then_some(milliseconds)
}

fn admin_auction_world_label(world: &str) -> String {
    match world {
        "main" => t!("admin-auctions-world-main"),
        "isekai" => t!("admin-auctions-world-isekai"),
        _ => world.to_owned(),
    }
}

#[component]
fn AdminAuctionRow(
    auction: AdminAuction,
    capabilities: AdminAuctionCapabilities,
    clock_millis: Signal<i64>,
    notice: Signal<Option<AdminAuctionLifecycleAction>>,
    reload_generation: Signal<u64>,
) -> Element {
    let auction_id = auction.id.clone();
    let lifecycle_status = auction.status.clone();
    let current_time_millis = clock_millis();
    let open_ready = auction.status == "preview"
        && (capabilities.manual_early_open
            || scheduled_time_millis(&auction.scheduled_open_at)
                .is_some_and(|scheduled| scheduled <= current_time_millis));
    rsx! {
        tr { class: "admin-auctions__row",
            td { class: "public-auction-catalog__number", "data-label": t!("admin-auctions-sequence"),
                h3 { class: "admin-visually-hidden",
                    {t!("admin-auctions-row-heading", number: auction.sequence, world: auction.world_id.clone())}
                }
                "#{auction.sequence}"
            }
            td { "data-label": t!("admin-auctions-world"), {admin_auction_world_label(&auction.world_id)} }
            td { "data-label": t!("admin-auctions-status"),
                span { class: "admin-auctions__status admin-auctions__status--{auction.status}",
                    {admin_auction_status_label(&auction.status)}
                }
            }
            td { "data-label": t!("admin-auctions-open-at"), time { datetime: auction.scheduled_open_at.clone(), "{format_admin_time(&auction.scheduled_open_at)}" } }
            td {
                "data-label": t!("admin-auctions-end-at"),
                if let Some(ends_at) = auction.scheduled_ends_at.clone().or(auction.scheduled_end_at.clone()) {
                    time { datetime: ends_at.clone(), "{format_admin_time(&ends_at)}" }
                } else {
                    {t!("value-empty")}
                }
            }
            td { "data-label": t!("admin-auctions-readiness"),
                if open_ready {
                    span { class: "admin-auctions__ready", {t!("admin-auctions-ready")} }
                } else {
                    {t!("value-empty")}
                }
            }
            td { "data-label": t!("admin-auctions-actions"),
                AdminAuctionActions {
                    auction_id,
                    auction_sequence: auction.sequence,
                    status: lifecycle_status,
                    expected_state_version: auction.state_version,
                    capabilities,
                    scheduled_open_at: auction.scheduled_open_at.clone(),
                    current_time_millis,
                    notice,
                    reload_generation,
                }
            }
        }
    }
}

#[component]
fn AdminAuctionActions(
    auction_id: String,
    auction_sequence: i64,
    status: String,
    expected_state_version: i64,
    capabilities: AdminAuctionCapabilities,
    scheduled_open_at: String,
    current_time_millis: i64,
    notice: Signal<Option<AdminAuctionLifecycleAction>>,
    reload_generation: Signal<u64>,
) -> Element {
    let busy = use_signal(|| false);
    let feedback = use_signal(|| None::<bool>);
    let pending_action = use_signal(|| None::<AdminAuctionLifecycleAction>);
    let cancel_reason = use_signal(String::new);
    let command_state = use_signal(|| None::<(String, Uuid)>);
    let actions =
        lifecycle_actions(&status, &capabilities, &scheduled_open_at, current_time_millis);

    rsx! {
        div { class: "admin-auctions__actions",
            Link {
                class: "admin-auctions__open-link",
                to: Route::AuctionLots { auction_id: auction_id.clone() },
                {t!("admin-auctions-open")}
            }
            for action in actions {
                AdminAuctionActionButton {
                    action,
                    busy,
                    notice,
                    pending_action,
                }
            }
        }
        if let Some(action) = pending_action() {
            AdminAuctionActionDialog {
                auction_id,
                auction_sequence,
                expected_state_version,
                action,
                busy,
                feedback,
                notice,
                reload_generation,
                pending_action,
                cancel_reason,
                command_state,
            }
        }
        if feedback() == Some(false) {
            span {
                class: "admin-panel__note action-feedback action-feedback--error",
                role: "alert",
                {t!("admin-auctions-action-error")}
            }
        }
    }
}

#[component]
fn AdminAuctionActionButton(
    action: AdminAuctionLifecycleAction,
    busy: Signal<bool>,
    mut notice: Signal<Option<AdminAuctionLifecycleAction>>,
    mut pending_action: Signal<Option<AdminAuctionLifecycleAction>>,
) -> Element {
    rsx! {
        button {
            class: lifecycle_action_button_class(action),
            r#type: "button",
            disabled: busy(),
            onclick: move |_| {
                if busy() {
                    return;
                }
                notice.set(None);
                pending_action.set(Some(action));
            },
            if busy() { {t!("admin-action-working")} } else { {lifecycle_action_label(action)} }
        }
    }
}

#[component]
fn AdminAuctionActionDialog(
    auction_id: String,
    auction_sequence: i64,
    expected_state_version: i64,
    action: AdminAuctionLifecycleAction,
    busy: Signal<bool>,
    mut feedback: Signal<Option<bool>>,
    mut notice: Signal<Option<AdminAuctionLifecycleAction>>,
    reload_generation: Signal<u64>,
    mut pending_action: Signal<Option<AdminAuctionLifecycleAction>>,
    mut cancel_reason: Signal<String>,
    mut command_state: Signal<Option<(String, Uuid)>>,
) -> Element {
    let is_cancel = action == AdminAuctionLifecycleAction::Cancel;
    let is_destructive = matches!(
        action,
        AdminAuctionLifecycleAction::Cancel | AdminAuctionLifecycleAction::FinalPhase
    );
    let can_submit = !busy() && (!is_cancel || !cancel_reason().trim().is_empty());
    rsx! {
        div { class: "admin-auctions__dialog-backdrop", role: "presentation",
            section {
                class: "admin-auctions__dialog",
                role: "dialog",
                aria_modal: "true",
                aria_labelledby: "admin-auction-action-title",
                tabindex: "-1",
                onkeydown: move |event| {
                    if event.key() == Key::Escape && !busy() {
                        pending_action.set(None);
                        cancel_reason.set(String::new());
                        feedback.set(None);
                    }
                },
                h2 { id: "admin-auction-action-title",
                    {lifecycle_action_confirm_title(action, auction_sequence)}
                }
                form {
                    class: "admin-auctions__dialog-form",
                    onsubmit: move |event| {
                        event.prevent_default();
                        if !can_submit {
                            return;
                        }
                            let reason = is_cancel.then(|| cancel_reason().trim().to_owned());
                            let auction_id = auction_id.clone();
                            let signature = format!(
                                "{action:?}:{expected_state_version}:{}",
                                reason.as_deref().unwrap_or_default()
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
                            let mut feedback = feedback;
                            let mut notice = notice;
                            let mut pending_action = pending_action;
                            let mut cancel_reason = cancel_reason;
                            let mut reload_generation = reload_generation;
                            busy.set(true);
                            spawn(async move {
                                match execute_admin_auction_lifecycle(
                                    command_id,
                                    auction_id,
                                    action,
                                    expected_state_version,
                                    reason,
                                )
                                .await
                                {
                                    Ok(_) => {
                                        feedback.set(None);
                                        notice.set(Some(action));
                                        pending_action.set(None);
                                        cancel_reason.set(String::new());
                                        command_state.set(None);
                                        reload_generation
                                            .set(reload_generation().saturating_add(1));
                                    }
                                    Err(_) => feedback.set(Some(false)),
                                }
                                busy.set(false);
                            });
                    },
                    p { class: "admin-panel__note", {lifecycle_action_confirm_note(action)} }
                    if is_cancel {
                        label {
                            {t!("admin-auctions-cancel-reason")}
                            textarea {
                                maxlength: "1024",
                                required: true,
                                autofocus: true,
                                disabled: busy(),
                                value: cancel_reason(),
                                oninput: move |event| {
                                    cancel_reason.set(event.value());
                                    feedback.set(None);
                                },
                            }
                        }
                        p { class: "admin-panel__note", {t!("admin-auctions-cancel-reason-hint")} }
                    }
                    if is_cancel && cancel_reason().trim().is_empty() {
                        p { class: "admin-panel__note action-feedback action-feedback--error", role: "alert",
                            {t!("admin-auctions-cancel-reason-required")}
                        }
                    }
                    div { class: "admin-auctions__dialog-actions",
                        button {
                            r#type: "button",
                            disabled: busy(),
                            autofocus: !is_cancel,
                            onclick: move |_| {
                                pending_action.set(None);
                                cancel_reason.set(String::new());
                                feedback.set(None);
                            },
                            {t!("admin-auctions-action-close")}
                        }
                        button {
                            class: if is_destructive { "admin-btn-danger" } else { "admin-btn-primary" },
                            r#type: "submit",
                            disabled: !can_submit,
                            if busy() { {t!("admin-action-working")} } else { {lifecycle_action_submit_label(action)} }
                        }
                    }
                }
            }
        }
    }
}

fn lifecycle_actions(
    status: &str,
    capabilities: &AdminAuctionCapabilities,
    scheduled_open_at: &str,
    current_time_millis: i64,
) -> Vec<AdminAuctionLifecycleAction> {
    let mut actions = Vec::new();
    match status {
        "draft" => {
            if capabilities.manual_preview_open {
                actions.push(AdminAuctionLifecycleAction::Preview);
            }
            if capabilities.cancel_auction {
                actions.push(AdminAuctionLifecycleAction::Cancel);
            }
        }
        "preview" => {
            if capabilities.manual_early_open
                || scheduled_time_millis(scheduled_open_at)
                    .is_some_and(|scheduled| scheduled <= current_time_millis)
            {
                actions.push(AdminAuctionLifecycleAction::Open);
            }
            if capabilities.cancel_auction {
                actions.push(AdminAuctionLifecycleAction::Cancel);
            }
        }
        "live" => {
            actions.push(AdminAuctionLifecycleAction::FinalPhase);
        }
        _ => {}
    }
    actions
}

fn lifecycle_action_label(action: AdminAuctionLifecycleAction) -> String {
    match action {
        AdminAuctionLifecycleAction::Preview => t!("admin-auctions-action-preview"),
        AdminAuctionLifecycleAction::Open => t!("admin-auctions-action-open"),
        AdminAuctionLifecycleAction::Cancel => t!("admin-auctions-action-cancel"),
        AdminAuctionLifecycleAction::FinalPhase => t!("admin-auctions-action-final"),
    }
}

fn lifecycle_action_button_class(action: AdminAuctionLifecycleAction) -> &'static str {
    match action {
        AdminAuctionLifecycleAction::Preview | AdminAuctionLifecycleAction::Open => {
            "admin-btn-primary"
        }
        AdminAuctionLifecycleAction::Cancel | AdminAuctionLifecycleAction::FinalPhase => {
            "admin-btn-danger"
        }
    }
}

fn lifecycle_action_submit_label(action: AdminAuctionLifecycleAction) -> String {
    match action {
        AdminAuctionLifecycleAction::FinalPhase => t!("admin-auctions-action-confirm"),
        _ => lifecycle_action_label(action),
    }
}

fn lifecycle_action_confirm_title(
    action: AdminAuctionLifecycleAction,
    auction_sequence: i64,
) -> String {
    match action {
        AdminAuctionLifecycleAction::Preview => {
            t!("admin-auctions-confirm-preview", number: auction_sequence)
        }
        AdminAuctionLifecycleAction::Open => {
            t!("admin-auctions-confirm-open", number: auction_sequence)
        }
        AdminAuctionLifecycleAction::Cancel => {
            t!("admin-auctions-confirm-cancel", number: auction_sequence)
        }
        AdminAuctionLifecycleAction::FinalPhase => {
            t!("admin-auctions-confirm-final", number: auction_sequence)
        }
    }
}

fn lifecycle_action_confirm_note(action: AdminAuctionLifecycleAction) -> String {
    match action {
        AdminAuctionLifecycleAction::Preview => t!("admin-auctions-confirm-preview-note"),
        AdminAuctionLifecycleAction::Open => t!("admin-auctions-confirm-open-note"),
        AdminAuctionLifecycleAction::Cancel => t!("admin-auctions-confirm-cancel-note"),
        AdminAuctionLifecycleAction::FinalPhase => t!("admin-auctions-confirm-final-note"),
    }
}

fn lifecycle_action_success_label(action: AdminAuctionLifecycleAction) -> String {
    match action {
        AdminAuctionLifecycleAction::Preview => t!("admin-auctions-action-preview-success"),
        AdminAuctionLifecycleAction::Open => t!("admin-auctions-action-open-success"),
        AdminAuctionLifecycleAction::Cancel => t!("admin-auctions-action-cancel-success"),
        AdminAuctionLifecycleAction::FinalPhase => t!("admin-auctions-action-final-success"),
    }
}
