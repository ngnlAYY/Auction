//! Admin lot 行、改价和撤回交互。

use dioxus::{html::Key, prelude::*};
use dioxus_i18n::t;
use uuid::Uuid;

use auction_web_shared::{TruncatedText, format_credit_amount as format_amount};

use crate::api::{AdminLot, AdminLotUpdateAction, update_admin_lot};

use super::super::format_admin_time;
use super::helpers::{admin_lot_code_for, equipment_details, lot_status_label};
use super::state::{LotDialog, LotUpdateNotice};

#[component]
pub(super) fn AdminLotRow(
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
                        display: super::helpers::submission_id_display(&submission_id),
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
