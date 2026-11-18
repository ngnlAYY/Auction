//! Admin lot 列表行和编辑入口。

use auction_web_shared::t;
use auction_web_shared::{TruncatedText, format_credit_amount};
use dioxus::prelude::*;

use super::super::{format_admin_time, lot_status_label};
use super::edit::{LotEdit, LotSnapshot, open_lot_edit};
use super::presentation::equipment_details;
use super::state::{LotDialog, LotUpdateNotice};
use crate::api::AdminLot;
use crate::surface::pending::PendingCommands;

#[component]
pub(super) fn AdminLotRow(
    lot: AdminLot,
    selected_lot: Signal<Option<(String, String)>>,
    mut notice: Signal<Option<LotUpdateNotice>>,
    allow_lot_update: bool,
    edits_ready: bool,
    draft: Signal<Option<LotEdit>>,
) -> Element {
    let lot_id = lot.id.clone();
    let can_update = allow_lot_update && lot.status == "pending";
    let owner = use_context::<PendingCommands>();
    let entity = format!("lot:{}", lot.id);
    let frozen = owner.contains(&entity);
    let edit_disabled = frozen || !edits_ready;
    let snapshot = LotSnapshot::from_lot(&lot);
    let reprice_base = snapshot.clone();
    let lot_code = snapshot.code.clone();
    let lot_code_for_proxy = lot_code.clone();
    rsx! {
        tr { class: "admin-lots__row",
            td { "data-label": t!("admin-lots-code"),
                span { class: "admin-lots__code", "{lot_code}" }
            }
            td { "data-label": t!("admin-lots-equipment"),
                TruncatedText {
                    value: equipment_details(&lot.equipment_name, &lot.equipment_attributes),
                    display: lot.equipment_name.clone(),
                    class_name: "admin-lots__equipment-name".to_owned(),
                }
            }
            td { "data-label": t!("admin-lots-seller"),
                TruncatedText {
                    value: lot.seller.clone().unwrap_or_else(|| t!("value-empty")),
                    class_name: "admin-lots__seller-name".to_owned(),
                }
            }
            td { "data-label": t!("admin-lots-submission"),
                if let Some(submission_id) = lot.submission_id.clone() {
                    TruncatedText {
                        value: submission_id.clone(),
                        display: super::presentation::submission_id_display(&submission_id),
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
                span { class: "admin-lots__price-value", "{format_credit_amount(lot.start_price)}" }
            }
            td { class: "admin-auction-num admin-lots__current-price", "data-label": t!("admin-lots-price"),
                strong { class: "admin-lots__price-value", "{format_credit_amount(lot.current_price)}" }
            }
            td { class: "admin-lots__actions-cell", "data-label": t!("admin-lots-actions"),
                div { class: "admin-auction-actions",
                    if can_update {
                        button {
                            class: "admin-btn-primary",
                            r#type: "button",
                            disabled: edit_disabled,
                            onclick: move |_| {
                                if !edit_disabled && open_lot_edit(draft, LotDialog::Reprice, reprice_base.clone()) {
                                    notice.set(None);
                                }
                            },
                            {t!("admin-lots-reprice")}
                        }
                        button {
                            class: "admin-btn-danger",
                            r#type: "button",
                            disabled: edit_disabled,
                            onclick: move |_| {
                                if !edit_disabled && open_lot_edit(draft, LotDialog::Withdraw, snapshot.clone()) {
                                    notice.set(None);
                                }
                            },
                            {t!("admin-lots-withdraw")}
                        }
                    }
                    button {
                        class: "admin-btn-secondary",
                        r#type: "button",
                        disabled: frozen,
                        onclick: move |_| {
                            selected_lot.set(Some((lot_id.clone(), lot_code_for_proxy.clone())));
                        },
                        {t!("admin-lots-proxy-open")}
                    }
                }
            }
        }
    }
}
