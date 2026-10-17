//! Admin lot 行、改价和撤回交互。

use auction_web_shared::{Dialog, legacy_t as t};
use dioxus::prelude::*;

use auction_web_shared::{TruncatedText, format_credit_amount as format_amount};

use crate::api::{AdminLot, AdminLotUpdateAction, update_admin_lot};
use crate::app::surface::admin_api_error_message;
use crate::app::surface::pending::{PendingCommands, set_if_alive};

use super::super::format_admin_time;
use super::helpers::{admin_lot_code_for, equipment_details, lot_status_label};
use super::state::{LotDialog, LotUpdateNotice};

/// 同一拍品更新流程共享的浏览器状态；集中传递以免 dialog 分支形成易错的长参数列表。
#[derive(Clone, Copy)]
struct LotUpdateSignals {
    owner: PendingCommands,
    failure_code: Signal<Option<String>>,
    dialog: Signal<Option<LotDialog>>,
    notice: Signal<Option<LotUpdateNotice>>,
}

#[component]
pub(super) fn AdminLotRow(
    lot: AdminLot,
    selected_lot: Signal<Option<(String, String)>>,
    mut notice: Signal<Option<LotUpdateNotice>>,
    allow_lot_update: bool,
) -> Element {
    let lot_id = lot.id.clone();
    let can_update = allow_lot_update && lot.status == "pending";
    let mut start_price = use_signal(|| lot.start_price.to_string());
    let mut dialog = use_signal(|| None::<LotDialog>);
    let owner = use_context::<PendingCommands>();
    let entity = format!("lot:{}", lot.id);
    let busy = use_memo(move || owner.contains(&entity));
    let mut failure_code = use_signal(|| None::<String>);
    let mut invalid_price = use_signal(|| false);
    let update_signals = LotUpdateSignals { owner, failure_code, dialog, notice };
    let lot_id_for_price = lot.id.clone();
    let lot_id_for_withdraw = lot.id.clone();
    let lot_code = admin_lot_code_for(&lot.equipment_category, lot.lot_number);
    let lot_code_for_proxy = lot_code.clone();
    let expected_state_version = lot.state_version;
    rsx! {
        tr { class: "admin-lots__row",
            td { class: "admin-lots__code-cell", "data-label": t!("admin-lots-code"),
                h4 { class: "admin-visually-hidden",
                    {t!(
                        "admin-lots-row-heading",
                        number: lot_code.clone(),
                        name: lot.equipment_name.clone()
                    )}
                }
                span { class: "admin-lots__code", "{lot_code}" }
            }
            td { class: "admin-lots__equipment", "data-label": t!("admin-lots-equipment"),
                TruncatedText {
                    value: equipment_details(&lot.equipment_name, &lot.equipment_attributes),
                    display: lot.equipment_name.clone(),
                    class_name: "admin-lots__equipment-name".to_owned(),
                }
            }
            td { class: "admin-lots__seller", "data-label": t!("admin-lots-seller"),
                TruncatedText {
                    value: lot.seller.clone().unwrap_or_else(|| t!("value-empty")),
                    class_name: "admin-lots__seller-name".to_owned(),
                }
            }
            td { class: "admin-lots__submission", "data-label": t!("admin-lots-submission"),
                if let Some(submission_id) = lot.submission_id.clone() {
                    TruncatedText {
                        value: submission_id.clone(),
                        display: super::helpers::submission_id_display(&submission_id),
                        size: "compact",
                    }
                } else {
                    span { class: "admin-lots__empty-value", {t!("value-empty")} }
                }
            }
            td { class: "admin-lots__status-cell", "data-label": t!("admin-lots-status"),
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
            td { class: "admin-auction-num admin-lots__start-price", "data-label": t!("admin-lots-start-price"),
                span { class: "admin-lots__price-value", "{format_amount(lot.start_price)}" }
            }
            td { class: "admin-auction-num admin-lots__current-price", "data-label": t!("admin-lots-price"),
                strong { class: "admin-lots__price-value", "{format_amount(lot.current_price)}" }
            }
            td { class: "admin-lots__actions-cell", "data-label": t!("admin-lots-actions"),
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
                        class: "admin-btn-secondary",
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
                    // 外壳（遮罩/标题栏/关闭守卫）由共享 Dialog 接管。
                    Dialog {
                        title: if current_dialog == LotDialog::Reprice {
                            t!("admin-lots-reprice-title", code: lot_code.clone())
                        } else {
                            t!("admin-lots-withdraw-title", code: lot_code.clone())
                        },
                        close_label: t!("admin-lots-action-cancel"),
                        busy: busy(),
                        on_close: move |_| {
                            dialog.set(None);
                            failure_code.set(None);
                            invalid_price.set(false);
                        },
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

/// 提交 lot 修改命令并保留相同输入的 command_id，确保失败重试仍符合幂等合同。
fn submit_lot_update(
    signals: LotUpdateSignals,
    expected_state_version: i64,
    lot_id: String,
    action: AdminLotUpdateAction,
    price: Option<i64>,
    success_notice: LotUpdateNotice,
) {
    let label = match action {
        AdminLotUpdateAction::SetStartPrice => t!("admin-lots-reprice"),
        AdminLotUpdateAction::Withdraw => t!("admin-lots-withdraw"),
    };
    if signals.owner.submit(
        format!("lot:{lot_id}"),
        label,
        move |command_id| {
            update_admin_lot(command_id, lot_id.clone(), expected_state_version, action, price)
        },
        move |_| {
            set_if_alive(signals.failure_code, None);
            set_if_alive(signals.notice, Some(success_notice));
        },
        move |code| set_if_alive(signals.failure_code, Some(code)),
    ) {
        set_if_alive(signals.dialog, None);
        set_if_alive(signals.notice, None);
    }
}
