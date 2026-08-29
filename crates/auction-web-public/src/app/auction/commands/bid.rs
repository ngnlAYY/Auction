//! 普通竞价表单和命令提交。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;
use uuid::Uuid;

use crate::api::{CommandError, place_bid};

use super::super::{format_amount, parse_credit_amount};
use super::common::{PendingBidCommand, amount_hint, bid_error_message};

/// 普通竞价表单参数；关闭与成功动作分开表达，避免调用方误用弹窗生命周期。
pub struct BidFormInput {
    pub lot_id: String,
    pub current_price: i64,
    pub next_bid: i64,
    pub lot_status: String,
    pub can_bid: bool,
    pub blocked: bool,
    pub blocked_reason: String,
    pub on_accepted: EventHandler<()>,
    pub on_close: EventHandler<()>,
}

/// 显示普通竞价表单；最低价和 lot 状态只用于即时反馈，最终结果由服务端决定。
pub fn bid_form(input: BidFormInput) -> Element {
    let BidFormInput {
        lot_id,
        current_price,
        next_bid,
        lot_status,
        can_bid,
        blocked,
        blocked_reason,
        on_accepted,
        on_close,
    } = input;
    let amount_id = format!("public-bid-amount-{lot_id}");
    let mut amount = use_signal(|| format_amount(next_bid));
    let mut failure = use_signal(|| None::<CommandError>);
    let mut validation = use_signal(|| None::<String>);
    let mut submitting = use_signal(|| false);
    let mut command = use_signal(|| None::<PendingBidCommand>);
    let mut refresh_generation = use_context::<Signal<u64>>();
    let bidding_open = can_bid && lot_status == "live";
    let blocked = blocked || !bidding_open;
    let blocked_reason = if bidding_open { blocked_reason } else { t!("bid-unavailable") };

    rsx! {
        form {
            class: "public-auction-bid-form",
            aria_busy: submitting().to_string(),
            onsubmit: move |event| {
                event.prevent_default();
                if submitting() || blocked {
                    return;
                }
                let Some(value) = parse_credit_amount(&amount()) else {
                    validation.set(Some(t!("bid-invalid-amount")));
                    failure.set(None);
                    return;
                };
                if value < next_bid {
                    validation.set(Some(t!("bid-below-minimum", price: format_amount(next_bid))));
                    failure.set(None);
                    return;
                }
                let pending = match command() {
                    Some(existing) if existing.amount == value => existing,
                    _ => PendingBidCommand { amount: value, command_id: Uuid::new_v4() },
                };
                command.set(Some(pending));
                validation.set(None);
                failure.set(None);
                submitting.set(true);
                let lot_id = lot_id.clone();
                spawn(async move {
                    let bid_result = place_bid(lot_id, pending.amount, pending.command_id).await;
                    submitting.set(false);
                    match bid_result {
                        Ok(_) => {
                            command.set(None);
                            refresh_generation.set(refresh_generation().saturating_add(1));
                            on_accepted.call(());
                        }
                        Err(error) => {
                            failure.set(Some(error));
                            // 失败后保留输入、command_id 和错误，同时重新读取权威价格与领先者。
                            refresh_generation.set(refresh_generation().saturating_add(1));
                        }
                    }
                });
            },
            p { class: "public-auction-bid-form__meta",
                {t!("bid-current-price", price: format_amount(current_price))}
                " · "
                {t!("bid-minimum", price: format_amount(next_bid))}
            }
            label { for: amount_id.clone(), {t!("bid-amount-label")} }
            input {
                id: amount_id,
                r#type: "text",
                autofocus: true,
                inputmode: "decimal",
                aria_describedby: "public-bid-amount-hint",
                disabled: submitting() || blocked,
                value: "{amount}",
                oninput: move |event| amount.set(event.value()),
            }
            {amount_hint(&amount(), "public-bid-amount-hint", "bid-invalid-amount")}
            p { class: "public-auction-bid-form__hint", {t!("bid-shorthand-hint")} }
            if blocked {
                p { class: "action-feedback action-feedback--notice", role: "note", "{blocked_reason}" }
            }
            if let Some(message) = validation() {
                p { class: "action-feedback action-feedback--error", role: "alert", "{message}" }
            }
            if let Some(error) = failure() {
                p { class: "action-feedback action-feedback--error", role: "alert",
                    "{bid_error_message(&error, next_bid)}"
                }
            }
            div { class: "public-auction-bid-form__actions",
                button {
                    r#type: "button",
                    onclick: move |_| on_close.call(()),
                    {t!("dialog-cancel")}
                }
                button { r#type: "submit", disabled: submitting() || blocked,
                    if submitting() { {t!("bid-working")} } else { {t!("bid-submit")} }
                }
            }
        }
    }
}
