//! Admin 结算与 recovery 页面。
//!
//! 页面复刻旧版的操作顺序：先选择已结算拍卖，再读取结算 lot、资金池摘要和 recovery
//! 安全投影。recovery 确认始终携带报告版本，不在浏览器端推导或修改业务状态。

use dioxus::{html::Key, prelude::*};
use dioxus_i18n::t;
use uuid::Uuid;

use auction_web_shared::{TruncatedText, format_credit_amount as format_amount};

use crate::api::{
    AdminAuction, AdminAuctionPage, AdminRecovery, AdminRecoveryAction, AdminRecoveryReport,
    AdminSettlementLot, AdminSettlementReport, AdminSettlementSummary, confirm_admin_recovery,
    fetch_admin_auctions, fetch_admin_recovery, fetch_admin_settlement,
};

use super::pagination::AdminPager;
use super::{AdminLoadError, AdminPanel, format_admin_time};

const PAGE_SIZE: usize = 20;

#[derive(Debug, Clone, PartialEq)]
struct RecoveryConfirmationDraft {
    recovery_id: String,
    state_version: i64,
    action: String,
    equipment_name: String,
    lot_number: i32,
}

/// 加载并展示旧版结算页面对应的完整 Rust 页面。
pub(super) fn page() -> Element {
    let selected_auction_id = use_signal(String::new);
    let auction_page = use_signal(|| 1_i64);
    let lot_page = use_signal(|| 1_i64);
    let recovery_page = use_signal(|| 1_i64);
    let mut reload_generation = use_signal(|| 0_u64);
    let confirmation = use_signal(|| None::<RecoveryConfirmationDraft>);
    let reason = use_signal(String::new);
    let action_busy = use_signal(|| false);
    let action_feedback = use_signal(|| None::<bool>);

    let auctions = use_resource(move || {
        let _ = reload_generation();
        fetch_admin_auctions("all".to_owned(), "settled".to_owned(), auction_page())
    });
    let settlement = use_resource(move || {
        let _ = reload_generation();
        let auction_id = selected_auction_id();
        async move {
            if auction_id.is_empty() {
                Ok(None)
            } else {
                fetch_admin_settlement(auction_id).await.map(Some)
            }
        }
    });
    let recovery = use_resource(move || {
        let _ = reload_generation();
        let auction_id = selected_auction_id();
        async move {
            if auction_id.is_empty() {
                Ok(None)
            } else {
                fetch_admin_recovery(auction_id).await.map(Some)
            }
        }
    });

    match &*auctions.read_unchecked() {
        None => rsx! {
            AdminPanel { title: t!("page-settlement-title"), detail: t!("admin-settlement-loading") }
        },
        Some(Err(_)) => rsx! {
            AdminLoadError {
                title: t!("page-settlement-title"),
                detail: t!("admin-settlement-error"),
                refresh: reload_generation,
            }
        },
        Some(Ok(page)) => {
            let settlement_state = settlement.read_unchecked().clone();
            let recovery_state = recovery.read_unchecked().clone();
            let selected = selected_auction_id();
            rsx! {
                section { class: "admin-panel admin-settlement", aria_labelledby: "admin-settlement-title",
                    h1 { id: "admin-settlement-title", {t!("page-settlement-title")} }
                    p { class: "admin-panel__note", {t!("page-settlement-note")} }
                    AdminSettlementPicker {
                        page: page.clone(),
                        selected_auction_id,
                        auction_page,
                        lot_page,
                        recovery_page,
                        confirmation,
                        reason,
                        action_feedback,
                    }
                    if selected.is_empty() {
                        p { class: "admin-panel__note", {t!("admin-settlement-pick-prompt")} }
                    }
                    if !selected.is_empty() {
                        if settlement_state.is_none() {
                            p { class: "admin-panel__note", role: "status", {t!("admin-settlement-loading")} }
                        }
                        if let Some(Err(_)) = settlement_state.as_ref() {
                            div { class: "admin-panel__note", role: "alert",
                                p { {t!("admin-settlement-error")} }
                                button {
                                    r#type: "button",
                                    onclick: move |_| reload_generation.set(reload_generation().saturating_add(1)),
                                    {t!("admin-retry")}
                                }
                            }
                        }
                        if let Some(Ok(Some(report))) = settlement_state.clone() {
                            AdminSettlementReportView {
                                report,
                                recovery: recovery_state.clone(),
                                lot_page,
                                recovery_page,
                                confirmation,
                                reason,
                                action_feedback,
                            }
                        }
                        if recovery_state.is_none() {
                            p { class: "admin-panel__note", role: "status", {t!("admin-recovery-loading")} }
                        }
                        if let Some(Err(_)) = recovery_state.as_ref() {
                            div { class: "admin-panel__note", role: "alert",
                                p { {t!("admin-recovery-error")} }
                                button {
                                    r#type: "button",
                                    onclick: move |_| reload_generation.set(reload_generation().saturating_add(1)),
                                    {t!("admin-retry")}
                                }
                            }
                        }
                    }
                    if let Some(draft) = confirmation() {
                        AdminRecoveryDialog {
                            draft,
                            confirmation,
                            reason,
                            busy: action_busy,
                            feedback: action_feedback,
                            reload_generation,
                        }
                    }
                    if let Some(success) = action_feedback() {
                        p {
                            class: if success {
                                "admin-panel__note action-feedback action-feedback--success"
                            } else {
                                "admin-panel__note action-feedback action-feedback--error"
                            },
                            role: if success { "status" } else { "alert" },
                            {if success { t!("admin-recovery-action-success") } else { t!("admin-recovery-action-error") }}
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn AdminSettlementPicker(
    page: AdminAuctionPage,
    mut selected_auction_id: Signal<String>,
    mut auction_page: Signal<i64>,
    mut lot_page: Signal<i64>,
    mut recovery_page: Signal<i64>,
    mut confirmation: Signal<Option<RecoveryConfirmationDraft>>,
    mut reason: Signal<String>,
    mut action_feedback: Signal<Option<bool>>,
) -> Element {
    let current_page = page.pagination.page.max(1);
    let page_size = page.pagination.page_size.max(1);
    rsx! {
        div { class: "admin-panel__actions admin-settlement__picker",
            label {
                {t!("admin-settlement-select-auction")}
                select {
                    value: selected_auction_id(),
                    onchange: move |event| {
                        selected_auction_id.set(event.value());
                        lot_page.set(1);
                        recovery_page.set(1);
                        confirmation.set(None);
                        reason.set(String::new());
                        action_feedback.set(None);
                    },
                    option { value: "", {t!("admin-settlement-choose-auction")} }
                    for auction in page.auctions.clone() {
                        option {
                            value: auction.id.clone(),
                            {auction_option_label(&auction)}
                        }
                    }
                }
            }
        }
        AdminPager {
            page: current_page,
            page_size,
            total: page.pagination.total,
            label: t!("admin-settlement-auction-pagination"),
            on_page_change: move |next_page| {
                auction_page.set(next_page);
                selected_auction_id.set(String::new());
                lot_page.set(1);
                recovery_page.set(1);
            },
        }
    }
}

#[component]
fn AdminSettlementReportView(
    report: AdminSettlementReport,
    recovery: Option<Result<Option<AdminRecoveryReport>, String>>,
    mut lot_page: Signal<i64>,
    mut recovery_page: Signal<i64>,
    confirmation: Signal<Option<RecoveryConfirmationDraft>>,
    reason: Signal<String>,
    action_feedback: Signal<Option<bool>>,
) -> Element {
    let lot_total_pages = page_count(report.lots.len());
    let current_lot_page = lot_page().clamp(1, lot_total_pages as i64);
    let lot_rows = page_slice(&report.lots, current_lot_page);
    let recovery_report = recovery.and_then(|value| value.ok()).flatten();
    let recovery_total_pages =
        recovery_report.as_ref().map_or(1, |value| page_count(value.recoveries.len()));
    let current_recovery_page = recovery_page().clamp(1, recovery_total_pages as i64);

    rsx! {
        section { class: "admin-settlement__report", aria_labelledby: "admin-settlement-report-title",
            h2 { id: "admin-settlement-report-title",
                {t!("admin-settlement-report-title", world: auction_world_label(&report.auction.world_id), sequence: report.auction.sequence)}
            }
            div { class: "admin-settlement__table-wrap",
                table {
                    caption { class: "admin-visually-hidden", {t!("admin-settlement-lots-title")} }
                    thead {
                        tr {
                            th { {t!("admin-settlement-lot")} }
                            th { {t!("admin-settlement-equipment")} }
                            th { {t!("admin-settlement-seller")} }
                            th { {t!("admin-settlement-winner")} }
                            th { {t!("admin-settlement-final-price")} }
                            th { {t!("admin-settlement-tax")} }
                            th { {t!("admin-settlement-hold")} }
                            th { {t!("admin-settlement-adjustment")} }
                            th { {t!("admin-settlement-listed-price")} }
                            th { {t!("admin-settlement-compensation")} }
                            th { {t!("admin-settlement-buyer")} }
                            th { {t!("admin-settlement-seller-amount")} }
                        }
                    }
                    tbody {
                        for lot in lot_rows {
                            AdminSettlementLotRow { lot }
                        }
                    }
                }
            }
            if report.lots.is_empty() {
                p { class: "admin-panel__note", {t!("admin-settlement-no-lots")} }
            }
            AdminSettlementPager {
                current_page: current_lot_page,
                total_pages: lot_total_pages,
                page: lot_page,
                label: t!("admin-settlement-lots-pagination"),
            }
            AdminSettlementSummaryCard { summary: report.settlement.clone() }
            if let Some(recovery_report) = recovery_report {
                AdminRecoveryTable {
                    report: recovery_report,
                    current_page: current_recovery_page,
                    total_pages: recovery_total_pages,
                    page: recovery_page,
                    confirmation,
                    reason,
                    action_feedback,
                }
            }
        }
    }
}

#[component]
fn AdminSettlementLotRow(lot: AdminSettlementLot) -> Element {
    let equipment_name = lot.equipment_name.clone();
    let seller = lot.seller.unwrap_or_else(|| t!("value-empty"));
    let winner = lot.winner.unwrap_or_else(|| t!("value-empty"));
    let adjustment = lot.adjustment.policy.unwrap_or_else(|| t!("value-empty"));
    let hold_share = hold_share(lot.tax_amount, lot.final_price);
    rsx! {
        tr { class: "admin-settlement__row",
            td { "data-label": t!("admin-settlement-lot"), "{lot.lot_number}" }
            td { "data-label": t!("admin-settlement-equipment"), TruncatedText { value: equipment_name } }
            td { "data-label": t!("admin-settlement-seller"), TruncatedText { value: seller } }
            td { "data-label": t!("admin-settlement-winner"), TruncatedText { value: winner } }
            td { "data-label": t!("admin-settlement-final-price"), class: "admin-settlement__amount", {format_amount(lot.final_price)} }
            td { "data-label": t!("admin-settlement-tax"), class: "admin-settlement__amount", {format_amount(lot.tax_amount)} }
            td { "data-label": t!("admin-settlement-hold"), class: "admin-settlement__amount", "{hold_share}" }
            td { "data-label": t!("admin-settlement-adjustment"), TruncatedText { value: adjustment } }
            td { "data-label": t!("admin-settlement-listed-price"), class: "admin-settlement__amount", {t!("value-empty")} }
            td { "data-label": t!("admin-settlement-compensation"), class: "admin-settlement__amount", {format_amount(lot.compensation_amount)} }
            td { "data-label": t!("admin-settlement-buyer"), class: "admin-settlement__amount", {format_amount(lot.buyer_amount)} }
            td { "data-label": t!("admin-settlement-seller-amount"), class: "admin-settlement__amount", {format_amount(lot.seller_amount)} }
        }
    }
}

#[component]
fn AdminSettlementSummaryCard(summary: Option<AdminSettlementSummary>) -> Element {
    rsx! {
        section { class: "admin-settlement__summary", aria_labelledby: "admin-settlement-summary-title",
            h2 { id: "admin-settlement-summary-title", {t!("admin-settlement-summary-title")} }
            if let Some(summary) = summary {
                div { class: "admin-settlement__cards",
                    SettlementMetric { label: t!("admin-settlement-opening"), value: summary.opening_balance }
                    SettlementMetric { label: t!("admin-settlement-tax-income"), value: summary.tax_income }
                    SettlementMetric { label: t!("admin-settlement-discount"), value: summary.discount_demand }
                    SettlementMetric { label: t!("admin-settlement-compensation-out"), value: summary.compensation_out }
                    SettlementMetric { label: t!("admin-settlement-closing"), value: summary.closing_balance }
                }
                p { class: "admin-panel__note",
                    {t!(
                        "admin-settlement-summary-meta",
                        movements: summary.movement_count,
                        at: format_admin_time(&summary.settled_at)
                    )}
                }
            } else {
                p { class: "admin-panel__note", {t!("admin-settlement-not-settled")} }
            }
        }
    }
}

#[component]
fn SettlementMetric(label: String, value: i64) -> Element {
    rsx! {
        div { class: "admin-settlement__metric",
            strong { {format_amount(value)} }
            span { {label} }
        }
    }
}

#[component]
fn AdminSettlementPager(
    current_page: i64,
    total_pages: usize,
    page: Signal<i64>,
    label: String,
) -> Element {
    rsx! {
        AdminPager {
            page: current_page,
            page_size: 1,
            total: i64::try_from(total_pages).unwrap_or(i64::MAX),
            label,
            on_page_change: move |next_page| page.set(next_page),
        }
    }
}

#[component]
fn AdminRecoveryTable(
    report: AdminRecoveryReport,
    current_page: i64,
    total_pages: usize,
    mut page: Signal<i64>,
    mut confirmation: Signal<Option<RecoveryConfirmationDraft>>,
    mut reason: Signal<String>,
    mut action_feedback: Signal<Option<bool>>,
) -> Element {
    let rows = page_slice(&report.recoveries, current_page);
    rsx! {
        section { class: "admin-settlement__recovery", aria_labelledby: "admin-recovery-title",
            h2 { id: "admin-recovery-title", {t!("admin-recovery-title")} }
            if report.recoveries.is_empty() {
                p { class: "admin-panel__note", {t!("admin-recovery-empty")} }
            } else {
                div { class: "admin-settlement__table-wrap",
                    table {
                        caption { class: "admin-visually-hidden", {t!("admin-recovery-title")} }
                        thead {
                            tr {
                                th { {t!("admin-recovery-lot")} }
                                th { {t!("admin-recovery-equipment")} }
                                th { {t!("admin-recovery-stage")} }
                                th { {t!("admin-recovery-deadline")} }
                                th { {t!("admin-recovery-evidence")} }
                                th { {t!("admin-recovery-actions")} }
                                th { {t!("admin-recovery-commands")} }
                            }
                        }
                        tbody {
                            for recovery in rows {
                                AdminRecoveryRow { recovery, confirmation, reason, action_feedback }
                            }
                        }
                    }
                }
            }
            AdminSettlementPager {
                current_page,
                total_pages,
                page,
                label: t!("admin-recovery-pagination"),
            }
        }
    }
}

#[component]
fn AdminRecoveryRow(
    recovery: AdminRecovery,
    mut confirmation: Signal<Option<RecoveryConfirmationDraft>>,
    mut reason: Signal<String>,
    mut action_feedback: Signal<Option<bool>>,
) -> Element {
    let recovery_id = recovery.id.clone();
    let display_equipment_name = recovery.equipment_name.clone();
    let deadline_attribute = recovery.payment_deadline_at.clone();
    let deadline_text = format_admin_time(&recovery.payment_deadline_at);
    let evidence = recovery
        .blocking_code
        .clone()
        .or(recovery.custody_proof_kind.clone())
        .unwrap_or_else(|| t!("value-empty"));
    let actions = recovery_action_summary(&recovery.actions);
    rsx! {
        tr { class: "admin-settlement__recovery-row",
            td { "data-label": t!("admin-recovery-lot"), "{recovery.lot_number}" }
            td { "data-label": t!("admin-recovery-equipment"),
                TruncatedText { value: display_equipment_name }
                small { TruncatedText { value: recovery.world_id.clone(), size: "compact" } }
            }
            td { "data-label": t!("admin-recovery-stage"),
                span { class: recovery_stage_class(&recovery.stage), {recovery_stage_label(&recovery.stage)} }
            }
            td { "data-label": t!("admin-recovery-deadline"), time { datetime: deadline_attribute, {deadline_text} } }
            td { "data-label": t!("admin-recovery-evidence"), TruncatedText { value: evidence, size: "wide" } }
            td { "data-label": t!("admin-recovery-actions"), TruncatedText { value: actions, size: "wide" } }
            td { "data-label": t!("admin-recovery-commands"),
                div { class: "admin-settlement__commands",
                    for action in recovery.allowed_commands.clone() {
                        {
                            let draft = RecoveryConfirmationDraft {
                                recovery_id: recovery_id.clone(),
                                state_version: recovery.state_version,
                                action: action.clone(),
                                equipment_name: recovery.equipment_name.clone(),
                                lot_number: recovery.lot_number,
                            };
                            rsx! {
                                button {
                                    class: "admin-btn-primary",
                                    r#type: "button",
                                    onclick: move |_| {
                                        confirmation.set(Some(draft.clone()));
                                        reason.set(String::new());
                                        action_feedback.set(None);
                                    },
                                    {recovery_command_label(&action)}
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
fn AdminRecoveryDialog(
    draft: RecoveryConfirmationDraft,
    mut confirmation: Signal<Option<RecoveryConfirmationDraft>>,
    mut reason: Signal<String>,
    mut busy: Signal<bool>,
    mut feedback: Signal<Option<bool>>,
    mut reload_generation: Signal<u64>,
) -> Element {
    let requires_reason = draft.action != "reclaimed";
    let mut command_state = use_signal(|| None::<(String, Uuid)>);
    rsx! {
        div { class: "admin-settlement__dialog-backdrop", role: "presentation",
            section {
                class: "admin-settlement__dialog",
                role: "dialog",
                aria_modal: "true",
                aria_labelledby: "admin-recovery-dialog-title",
                tabindex: "-1",
                onkeydown: move |event| {
                    if event.key() == Key::Escape && !busy() {
                        confirmation.set(None);
                        reason.set(String::new());
                        feedback.set(None);
                    }
                },
                h2 { id: "admin-recovery-dialog-title", {recovery_command_label(&draft.action)} }
                p { {t!("admin-recovery-confirm-description", lot: draft.lot_number, equipment: draft.equipment_name.clone())} }
                form {
                    onsubmit: move |event| {
                        event.prevent_default();
                        if busy() || (requires_reason && reason().trim().is_empty()) {
                            return;
                        }
                        busy.set(true);
                        let draft = draft.clone();
                        let reason_value = if draft.action == "reclaimed" {
                            String::new()
                        } else {
                            reason().trim().to_owned()
                        };
                        let command_id = stable_command_id(
                            command_state,
                            format!(
                                "{}:{}:{}:{reason_value}",
                                draft.recovery_id, draft.state_version, draft.action
                            ),
                        );
                        let mut busy = busy;
                        let mut confirmation = confirmation;
                        let mut reason = reason;
                        let mut feedback = feedback;
                        let mut reload_generation = reload_generation;
                        spawn(async move {
                            match confirm_admin_recovery(
                                command_id,
                                draft.recovery_id,
                                draft.state_version,
                                draft.action,
                                reason_value,
                            )
                            .await
                            {
                                Ok(_) => {
                                    confirmation.set(None);
                                    reason.set(String::new());
                                    feedback.set(Some(true));
                                    command_state.set(None);
                                    reload_generation
                                        .set(reload_generation().saturating_add(1));
                                }
                                Err(_) => feedback.set(Some(false)),
                            }
                            busy.set(false);
                        });
                    },
                    if requires_reason {
                        label {
                            r#for: "admin-recovery-reason",
                            {t!("admin-recovery-reason")}
                            textarea {
                                id: "admin-recovery-reason",
                                rows: "4",
                                maxlength: "512",
                                required: true,
                                disabled: busy(),
                                value: reason(),
                                oninput: move |event| reason.set(event.value()),
                            }
                        }
                    }
                    div { class: "admin-panel__actions",
                        button {
                            class: "admin-btn-secondary",
                            r#type: "button",
                            disabled: busy(),
                            autofocus: true,
                            onclick: move |_| {
                                confirmation.set(None);
                                reason.set(String::new());
                            },
                            {t!("admin-recovery-cancel")}
                        }
                        button {
                            class: "admin-btn-primary",
                            r#type: "submit",
                            disabled: busy() || (requires_reason && reason().trim().is_empty()),
                            if busy() { {t!("admin-action-working")} } else { {t!("admin-recovery-confirm")} }
                        }
                    }
                }
            }
        }
    }
}

fn stable_command_id(mut command_state: Signal<Option<(String, Uuid)>>, signature: String) -> Uuid {
    match command_state() {
        Some((current, command_id)) if current == signature => command_id,
        _ => {
            let command_id = Uuid::new_v4();
            command_state.set(Some((signature, command_id)));
            command_id
        }
    }
}

fn auction_option_label(auction: &AdminAuction) -> String {
    format!(
        "{} #{} ({})",
        auction_world_label(&auction.world_id),
        auction.sequence,
        auction_status_label(&auction.status)
    )
}

fn auction_world_label(world_id: &str) -> String {
    match world_id {
        "main" => t!("admin-auctions-world-main"),
        "isekai" => t!("admin-auctions-world-isekai"),
        _ => world_id.to_owned(),
    }
}

fn auction_status_label(status: &str) -> String {
    match status {
        "settled" => t!("admin-auctions-status-settled"),
        "settling" => t!("admin-auctions-status-settling"),
        "cancelled" => t!("admin-auctions-status-cancelled"),
        _ => status.to_owned(),
    }
}

fn recovery_stage_label(stage: &str) -> String {
    match stage {
        "awaiting_payment" => t!("admin-recovery-stage-awaiting-payment"),
        "payment_overdue" => t!("admin-recovery-stage-payment-overdue"),
        "delivering_equipment" => t!("admin-recovery-stage-delivering"),
        "reclaiming" => t!("admin-recovery-stage-reclaiming"),
        "relist_pending" => t!("admin-recovery-stage-relist-pending"),
        "refunding" => t!("admin-recovery-stage-refunding"),
        "completed" => t!("admin-recovery-stage-completed"),
        _ => stage.to_owned(),
    }
}

fn recovery_stage_class(stage: &str) -> &'static str {
    match stage {
        "completed" => "admin-settlement__stage admin-settlement__stage--completed",
        "payment_overdue" | "reclaiming" | "refunding" => {
            "admin-settlement__stage admin-settlement__stage--attention"
        }
        _ => "admin-settlement__stage admin-settlement__stage--active",
    }
}

fn recovery_action_summary(actions: &[AdminRecoveryAction]) -> String {
    if actions.is_empty() {
        return t!("value-empty");
    }
    actions
        .iter()
        .map(|action| {
            format!(
                "{}: {}",
                recovery_action_kind_label(&action.kind),
                recovery_action_state_label(&action.state)
            )
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

fn hold_share(tax_amount: i64, final_price: i64) -> String {
    if final_price <= 0 {
        return t!("value-empty");
    }
    let scaled_percent =
        tax_amount.max(0).saturating_mul(10_000).checked_div(final_price).unwrap_or_default();
    let whole = scaled_percent / 100;
    let fraction = scaled_percent % 100;
    if fraction == 0 {
        format!("{whole}%")
    } else if fraction % 10 == 0 {
        format!("{whole}.{}%", fraction / 10)
    } else {
        format!("{whole}.{fraction:02}%")
    }
}

fn recovery_action_kind_label(kind: &str) -> String {
    match kind {
        "payment_reminder" => t!("admin-recovery-action-payment-reminder"),
        "reclaim_equipment_take" => t!("admin-recovery-action-reclaim"),
        "cancel_carrier_take" => t!("admin-recovery-action-cancel-carrier"),
        "recover_equipment_take" => t!("admin-recovery-action-recover-equipment"),
        "buyer_blacklist" => t!("admin-recovery-action-buyer-blacklist"),
        _ => kind.to_owned(),
    }
}

fn recovery_action_state_label(state: &str) -> String {
    match state {
        "pending" => t!("admin-recovery-state-pending"),
        "prepared" => t!("admin-recovery-state-prepared"),
        "enqueued" => t!("admin-recovery-state-enqueued"),
        "satisfied" => t!("admin-recovery-state-satisfied"),
        "failed" => t!("admin-recovery-state-failed"),
        "manual_reconcile" => t!("admin-recovery-state-manual"),
        _ => state.to_owned(),
    }
}

fn recovery_command_label(action: &str) -> String {
    match action {
        "reclaimed" => t!("admin-recovery-command-reclaimed"),
        "cancelled" => t!("admin-recovery-command-cancelled"),
        "equipment_recovered" => t!("admin-recovery-command-equipment-recovered"),
        _ => action.to_owned(),
    }
}

fn page_count(length: usize) -> usize {
    length.div_ceil(PAGE_SIZE).max(1)
}

fn page_slice<T: Clone>(items: &[T], page: i64) -> Vec<T> {
    let page = page.max(1) as usize;
    items.iter().skip((page - 1) * PAGE_SIZE).take(PAGE_SIZE).cloned().collect()
}
