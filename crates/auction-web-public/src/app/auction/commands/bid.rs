//! 普通竞价表单和命令提交。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;
use uuid::Uuid;

use crate::api::{CommandError, place_bid};

use super::super::format_amount;
use super::common::{PendingBidCommand, bid_error_message};

const CREDIT_K: i64 = 1_000;
const MAX_SAFE_JSON_INTEGER: i64 = 9_007_199_254_740_991;

/// 普通竞价表单参数；关闭与成功动作分开表达，避免调用方误用弹窗生命周期。
pub struct BidFormInput {
    pub lot_id: String,
    pub lot_code: String,
    pub current_price: i64,
    pub next_bid: i64,
    pub lot_status: String,
    pub can_bid: bool,
    pub blocked: bool,
    pub blocked_reason: String,
    /// 由弹窗宿主注入的共享提交状态；宿主据此在提交中禁用遮罩/Escape/关闭按钮，
    /// 避免弹窗 scope 销毁导致成功后的刷新与 on_accepted 回调丢失。
    pub submitting: Signal<bool>,
    pub on_accepted: EventHandler<()>,
    pub on_close: EventHandler<()>,
}

/// 普通出价输入固定使用 K 单位；向上取整保证非整千的旧起拍价不会生成低于最低价的默认值。
fn default_bid_units(next_bid: i64) -> String {
    let units = next_bid / CREDIT_K + i64::from(next_bid % CREDIT_K > 0);
    units.max(1).to_string()
}

/// 普通出价只接受纯数字 K 单位，避免把代理表单保留的 M/K 缩写规则扩散到确认流程。
fn parse_bid_units(input: &str) -> Option<i64> {
    let units = input.trim();
    if units.is_empty() || !units.chars().all(|character| character.is_ascii_digit()) {
        return None;
    }
    let amount = units.parse::<i64>().ok()?.checked_mul(CREDIT_K)?;
    (amount > 0 && amount <= MAX_SAFE_JSON_INTEGER).then_some(amount)
}

fn format_bid_units(amount: i64) -> String {
    format!("{}K", amount / CREDIT_K)
}

fn bid_amount_hint(value: &str) -> Element {
    if value.trim().is_empty() {
        return rsx! {};
    }
    match parse_bid_units(value) {
        Some(amount) => rsx! {
            p {
                id: "public-bid-amount-hint",
                class: "public-auction-bid-form__amount-hint",
                role: "status",
                "= {format_bid_units(amount)}"
            }
        },
        None => rsx! {
            p {
                id: "public-bid-amount-hint",
                class: "public-auction-bid-form__amount-hint public-auction-bid-form__amount-hint--invalid",
                role: "alert",
                {t!("bid-invalid-amount")}
            }
        },
    }
}

/// 显示普通竞价表单；最低价和 lot 状态只用于即时反馈，最终结果由服务端决定。
pub fn bid_form(input: BidFormInput) -> Element {
    let BidFormInput {
        lot_id,
        lot_code,
        current_price,
        next_bid,
        lot_status,
        can_bid,
        blocked,
        blocked_reason,
        submitting,
        on_accepted,
        on_close,
    } = input;
    let amount_id = format!("public-bid-amount-{lot_id}");
    let mut amount = use_signal(|| default_bid_units(next_bid));
    let mut failure = use_signal(|| None::<CommandError>);
    let mut validation = use_signal(|| None::<String>);
    // 提交状态由宿主注入（见 BidFormInput::submitting）。
    let mut submitting = submitting;
    let mut command = use_signal(|| None::<PendingBidCommand>);
    let mut confirmation = use_signal(|| None::<PendingBidCommand>);
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
                let pending = if let Some(pending) = confirmation() {
                    if pending.amount < next_bid {
                        confirmation.set(None);
                        validation.set(Some(t!("bid-below-minimum", price: format_amount(next_bid))));
                        failure.set(None);
                        return;
                    }
                    pending
                } else {
                    let Some(value) = parse_bid_units(&amount()) else {
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
                    confirmation.set(Some(pending));
                    validation.set(None);
                    failure.set(None);
                    return;
                };
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
                            confirmation.set(None);
                            failure.set(Some(error));
                            // 失败后保留输入、command_id 和错误，同时重新读取权威价格与领先者。
                            refresh_generation.set(refresh_generation().saturating_add(1));
                        }
                    }
                });
            },
            if let Some(pending) = confirmation() {
                p {
                    id: "public-bid-confirmation",
                    class: "public-auction-bid-form__confirmation",
                    role: "status",
                    aria_live: "polite",
                    aria_atomic: "true",
                    {t!(
                        "bid-confirm-description",
                        amount: format_bid_units(pending.amount),
                        code: lot_code.clone()
                    )}
                }
            } else {
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
                    inputmode: "numeric",
                    pattern: "[0-9]*",
                    autocomplete: "off",
                    spellcheck: "false",
                    aria_describedby: "public-bid-amount-hint public-bid-unit-hint",
                    disabled: submitting() || blocked,
                    value: "{amount}",
                    oninput: move |event| amount.set(event.value()),
                }
                {bid_amount_hint(&amount())}
                p {
                    id: "public-bid-unit-hint",
                    class: "public-auction-bid-form__hint",
                    {t!("bid-shorthand-hint")}
                }
            }
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
                    disabled: submitting(),
                    onclick: move |_| {
                        if confirmation().is_some() {
                            confirmation.set(None);
                        } else {
                            on_close.call(());
                        }
                    },
                    if confirmation().is_some() {
                        {t!("bid-confirm-back")}
                    } else {
                        {t!("dialog-cancel")}
                    }
                }
                button {
                    r#type: "submit",
                    disabled: submitting() || blocked,
                    aria_describedby: if confirmation().is_some() {
                        "public-bid-confirmation"
                    } else {
                        ""
                    },
                    if submitting() {
                        {t!("bid-working")}
                    } else if confirmation().is_some() {
                        {t!("bid-confirm-submit")}
                    } else {
                        {t!("bid-submit")}
                    }
                }
            }
        }
    }
}
