//! Admin lot 行、改价和撤回交互。

use auction_web_shared::legacy_t as t;
use dioxus::{html::Key, prelude::*};
use uuid::Uuid;

use auction_web_shared::{TruncatedText, format_credit_amount as format_amount};

use crate::api::{AdminLot, AdminLotUpdateAction, update_admin_lot};
use crate::app::surface::admin_api_error_message;

use super::super::format_admin_time;
use super::helpers::{admin_lot_code_for, equipment_details, lot_status_label};
use super::state::{LotDialog, LotUpdateNotice};

/// 同一拍品更新流程共享的浏览器状态；集中传递以免 dialog 分支形成易错的长参数列表。
#[derive(Clone, Copy)]
struct LotUpdateSignals {
    command_state: Signal<Option<(String, Uuid)>>,
    busy: Signal<bool>,
    failure_code: Signal<Option<String>>,
    dialog: Signal<Option<LotDialog>>,
    notice: Signal<Option<LotUpdateNotice>>,
    reload_generation: Signal<u64>,
}

#[component]
pub(super) fn AdminLotRow(
    lot: AdminLot,
    selected_lot: Signal<Option<(String, String)>>,
    mut notice: Signal<Option<LotUpdateNotice>>,
    reload_generation: Signal<u64>,
    allow_lot_update: bool,
) -> Element {
    let lot_id = lot.id.clone();
    let can_update = allow_lot_update && lot.status == "pending";
    let mut start_price = use_signal(|| lot.start_price.to_string());
    let mut dialog = use_signal(|| None::<LotDialog>);
    let busy = use_signal(|| false);
    let mut failure_code = use_signal(|| None::<String>);
    let mut invalid_price = use_signal(|| false);
    let command_state = use_signal(|| None::<(String, Uuid)>);
    let update_signals =
        LotUpdateSignals { command_state, busy, failure_code, dialog, notice, reload_generation };
    let lot_id_for_price = lot.id.clone();
    let lot_id_for_withdraw = lot.id.clone();
    let lot_code = admin_lot_code_for(&lot.equipment_category, lot.lot_number);
    let lot_code_for_proxy = lot_code.clone();
    let expected_state_version = lot.state_version;
    rsx! {
        tr { class: "admin-lots__row",
            td { "data-label": t!("admin-lots-code"),
                h4 { class: "admin-visually-hidden",
                    {t!(
                        "admin-lots-row-heading",
                        number: lot_code.clone(),
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
                        display: super::helpers::submission_id_display(&submission_id),
                        size: "compact",
                    }
                }
            }
            td { "data-label": t!("admin-lots-status"),
                span { class: "admin-auction-badge admin-lots__status--{lot.status}",
                    {lot_status_label(&lot.status)}
                }
                if let Some(slow_due_at) = lot.slow_due_at.clone() {
                    span { class: "admin-lots__slow-schedule",
                        title: lot.slow_authorization_id.clone().unwrap_or_default(),
                        {t!("admin-lots-slow-schedule", generation: lot.slow_generation, time: format_admin_time(&slow_due_at))}
                    }
                }
            }
            td { class: "admin-auction-num", "data-label": t!("admin-lots-start-price"), "{format_amount(lot.start_price)}" }
            td { class: "admin-auction-num", "data-label": t!("admin-lots-price"), "{format_amount(lot.current_price)}" }
            td { "data-label": t!("admin-lots-actions"),
                div { class: "admin-auction-actions",
                    if can_update {
                        button {
                            class: "admin-btn-primary",
                            r#type: "button",
                            disabled: busy(),
                            onclick: move |_| {
                                start_price.set(lot.start_price.to_string());
                                failure_code.set(None);
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
                                failure_code.set(None);
                                invalid_price.set(false);
                                notice.set(None);
                                dialog.set(Some(LotDialog::Withdraw));
                            },
                            {t!("admin-lots-withdraw")}
                        }
                        if let Some(code) = failure_code() {
                            span {
                                role: "alert",
                                class: "action-feedback action-feedback--error",
                                {admin_api_error_message(&code)}
                            }
                        }
                    }
                    button {
                        r#type: "button",
                        disabled: busy(),
                        onclick: move |_| {
                            selected_lot.set(Some((lot_id.clone(), lot_code_for_proxy.clone())));
                        },
                        {t!("admin-lots-proxy-open")}
                    }
                }
            }
        }
        if let Some(current_dialog) = dialog() {
            tr { class: "admin-auction-dialog-row",
                td { colspan: "8",
                    div { class: "admin-shell__dialog-backdrop", role: "presentation",
                        section {
                            class: "admin-shell__dialog",
                            role: "dialog",
                            aria_modal: "true",
                            aria_labelledby: if current_dialog == LotDialog::Reprice {
                                format!("price-title-{lot_id}")
                            } else {
                                format!("withdraw-title-{lot_id}")
                            },
                            onkeydown: move |event| {
                                if event.key() == Key::Escape && !busy() {
                                    event.prevent_default();
                                    event.stop_propagation();
                                    dialog.set(None);
                                    failure_code.set(None);
                                    invalid_price.set(false);
                                }
                            },
                            h2 { id: if current_dialog == LotDialog::Reprice {
                                format!("price-title-{lot_id}")
                            } else {
                                format!("withdraw-title-{lot_id}")
                            },
                                if current_dialog == LotDialog::Reprice {
                                    {t!("admin-lots-reprice-title", code: lot_code.clone())}
                                } else {
                                    {t!("admin-lots-withdraw-title", code: lot_code.clone())}
                                }
                            }
                            if current_dialog == LotDialog::Reprice {
                                form {
                                    class: "admin-auction-reprice",
                                    onsubmit: move |event| {
                                        event.prevent_default();
                                        if busy() {
                                            return;
                                        }
                                        let Ok(price) = start_price().parse::<i64>() else {
                                            invalid_price.set(true);
                                            return;
                                        };
                                        if price <= 0 {
                                            invalid_price.set(true);
                                            return;
                                        }
                                        invalid_price.set(false);
                                        submit_lot_update(
                                            update_signals,
                                            expected_state_version,
                                            lot_id_for_price.clone(),
                                            AdminLotUpdateAction::SetStartPrice,
                                            Some(price),
                                            LotUpdateNotice::Repriced,
                                        );
                                    },
                                    label {
                                        r#for: "price-{lot_id}",
                                        {t!("admin-lots-start-price-label")}
                                    }
                                    input {
                                        id: "price-{lot_id}",
                                        r#type: "number",
                                        min: "1",
                                        step: "1",
                                        required: true,
                                        disabled: busy(),
                                        autofocus: true,
                                        value: start_price(),
                                        oninput: move |event| {
                                            start_price.set(event.value());
                                            failure_code.set(None);
                                            invalid_price.set(false);
                                        },
                                    }
                                    if invalid_price() {
                                        p { role: "alert", {t!("admin-lots-invalid-price")} }
                                    }
                                    if let Some(code) = failure_code() {
                                        p { role: "alert", {admin_api_error_message(&code)} }
                                    }
                                    div { class: "admin-shell__dialog-actions",
                                        button {
                                            r#type: "button",
                                            disabled: busy(),
                                            onclick: move |_| {
                                                dialog.set(None);
                                                failure_code.set(None);
                                                invalid_price.set(false);
                                            },
                                            {t!("admin-lots-action-cancel")}
                                        }
                                        button {
                                            class: "admin-btn-primary",
                                            r#type: "submit",
                                            disabled: busy(),
                                            {t!("admin-lots-reprice-save")}
                                        }
                                    }
                                }
                            } else {
                                p {
                                    {t!(
                                        "admin-lots-withdraw-warning",
                                        seller: lot.seller.clone().unwrap_or_else(|| t!("value-empty"))
                                    )}
                                }
                                if let Some(code) = failure_code() {
                                    p { role: "alert", {admin_api_error_message(&code)} }
                                }
                                div { class: "admin-shell__dialog-actions",
                                    button {
                                        r#type: "button",
                                        disabled: busy(),
                                        onclick: move |_| {
                                            dialog.set(None);
                                            failure_code.set(None);
                                            invalid_price.set(false);
                                        },
                                        {t!("admin-lots-action-cancel")}
                                    }
                                    button {
                                        class: "admin-btn-danger",
                                        r#type: "button",
                                        disabled: busy(),
                                        onclick: move |_| {
                                            if busy() {
                                                return;
                                            }
                                            submit_lot_update(
                                                update_signals,
                                                expected_state_version,
                                                lot_id_for_withdraw.clone(),
                                                AdminLotUpdateAction::Withdraw,
                                                None,
                                                LotUpdateNotice::Withdrawn,
                                            );
                                        },
                                        {t!("admin-lots-withdraw-confirm")}
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

/// 提交 lot 修改命令并保留相同输入的 command_id，确保失败重试仍符合幂等合同。
fn submit_lot_update(
    mut signals: LotUpdateSignals,
    expected_state_version: i64,
    lot_id: String,
    action: AdminLotUpdateAction,
    price: Option<i64>,
    success_notice: LotUpdateNotice,
) {
    let action_key = match action {
        AdminLotUpdateAction::SetStartPrice => "set_start_price",
        AdminLotUpdateAction::Withdraw => "withdraw",
    };
    let signature = format!(
        "{action_key}:{expected_state_version}:{}",
        price.map_or_else(String::new, |value| value.to_string())
    );
    let command_id = match (signals.command_state)() {
        Some((current, command_id)) if current == signature => command_id,
        _ => {
            let command_id = Uuid::new_v4();
            signals.command_state.set(Some((signature, command_id)));
            command_id
        }
    };
    signals.notice.set(None);
    signals.busy.set(true);
    spawn(async move {
        match update_admin_lot(command_id, lot_id, expected_state_version, action, price).await {
            Ok(_) => {
                signals.failure_code.set(None);
                signals.dialog.set(None);
                signals.command_state.set(None);
                signals.notice.set(Some(success_notice));
                signals.reload_generation.set((signals.reload_generation)().saturating_add(1));
            }
            Err(code) => signals.failure_code.set(Some(code)),
        }
        signals.busy.set(false);
    });
}
