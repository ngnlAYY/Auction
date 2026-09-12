//! 普通竞价表单和命令提交。

use auction_contracts::MAX_PUBLIC_BID_AMOUNT;
use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use crate::api::place_bid;

use super::super::format_amount;
use super::common::{BidConfirmCommand, PendingBidCommand, bid_error_message};

const CREDIT_K: i64 = 1_000;

/// 普通竞价成功后的浏览器展示投影；只保留反馈面板所需字段，避免把 api 内部 DTO
/// 类型（未在 api 出口公开）泄漏到组件参数。
#[derive(Debug, Clone, PartialEq)]
pub struct BidSuccess {
    pub leading: bool,
    pub current_price: i64,
}

/// 普通竞价表单参数；关闭与成功动作分开表达，避免调用方误用弹窗生命周期。
#[derive(Clone, PartialEq)]
pub struct BidFormInput {
    pub lot_id: String,
    pub lot_code: String,
    pub current_price: i64,
    pub next_bid: i64,
    pub lot_status: String,
    pub can_bid: bool,
    pub blocked: bool,
    pub blocked_reason: String,
    /// 由弹窗宿主注入的两阶段确认命令 hook（整体 Copy）；宿主据其 `submitting`
    /// 在提交中禁用遮罩/Escape/关闭按钮（Dialog busy 守卫），避免弹窗 scope 销毁
    /// 导致成功后的刷新与成功面板反馈丢失。
    pub command: BidConfirmCommand,
    /// 成功投影信号由弹窗宿主持有：命令成功后表单设置它，入口据其渲染成功面板
    /// （与代理 proxy_success 模式一致），宿主在关闭/重新打开弹窗时清零。
    pub success: Signal<Option<BidSuccess>>,
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
    (amount > 0 && amount <= MAX_PUBLIC_BID_AMOUNT).then_some(amount)
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
                "= {format_amount(amount)}"
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

/// 普通竞价成功后的反馈面板，与代理成功面板（proxy/success.rs）同款模式：
/// 弹窗保持打开，成功消息以 role=status 公告，用户点关闭按钮才退出弹窗。
fn bid_success_panel(success: &BidSuccess, on_close: EventHandler<()>) -> Element {
    let success_message = if success.leading {
        t!("bid-success-leading", price: format_amount(success.current_price))
    } else {
        t!("bid-success")
    };
    rsx! {
        section { class: "public-auction-bid-form",
            p { class: "action-feedback action-feedback--success", role: "status",
                "{success_message}"
            }
            p { class: "public-auction-bid-form__meta",
                {t!("bid-current-price", price: format_amount(success.current_price))}
            }
            div { class: "public-auction-bid-form__actions",
                button {
                    r#type: "button",
                    onclick: move |_| on_close.call(()),
                    {t!("catalog-close")}
                }
            }
        }
    }
}

/// 为表单建立独立组件作用域，避免弹窗开关改变父组件的 Hook 顺序。
#[component]
pub fn BidForm(input: BidFormInput) -> Element {
    bid_form(input)
}

/// 显示普通竞价表单；只由 BidForm 无条件调用，最低价与 lot 状态的最终判断留在服务端。
fn bid_form(input: BidFormInput) -> Element {
    let BidFormInput {
        lot_id,
        lot_code,
        current_price,
        next_bid,
        lot_status,
        can_bid,
        blocked,
        blocked_reason,
        command,
        mut success,
        on_close,
    } = input;
    let amount_id = format!("public-bid-amount-{lot_id}");
    let mut amount = use_signal(|| default_bid_units(next_bid));
    let mut validation = use_signal(|| None::<String>);
    // 两阶段确认状态（command/confirmation/submitting/failure）由宿主注入的 hook 持有。
    let mut command = command;
    let submitting = command.submitting;
    let mut refresh_generation = use_context::<Signal<u64>>();
    // 成功视图也在全部 Hook 之后选择，保证该组件每次渲染的状态槽顺序稳定。
    if let Some(success) = success() {
        return bid_success_panel(&success, on_close);
    }
    let bidding_open = can_bid && lot_status == "live";
    let limit_reached = next_bid > MAX_PUBLIC_BID_AMOUNT;
    let blocked = blocked || !bidding_open || limit_reached;
    let blocked_reason = if !bidding_open {
        t!("bid-unavailable")
    } else if limit_reached {
        t!("bid-limit-note")
    } else {
        blocked_reason
    };

    rsx! {
        form {
            class: "public-auction-bid-form",
            aria_busy: submitting().to_string(),
            onsubmit: move |event| {
                event.prevent_default();
                if submitting() || blocked {
                    return;
                }
                // 首次提交只冻结金额与 command_id 并进入确认态；确认态再次提交后才调用
                // 外部 API。签名取金额序列化：同一金额重试复用同一 command_id（幂等）。
                let pending = if let Some(pending) = (command.confirmation)() {
                    if pending.amount > MAX_PUBLIC_BID_AMOUNT {
                        command.cancel_confirmation();
                        validation.set(Some(t!("bid-invalid-amount")));
                        (command.failure).set(None);
                        return;
                    }
                    if pending.amount < next_bid {
                        command.cancel_confirmation();
                        validation.set(Some(t!("bid-below-minimum", price: format_amount(next_bid))));
                        (command.failure).set(None);
                        return;
                    }
                    pending
                } else {
                    let Some(value) = parse_bid_units(&amount()) else {
                        validation.set(Some(t!("bid-invalid-amount")));
                        (command.failure).set(None);
                        return;
                    };
                    if value < next_bid {
                        validation.set(Some(t!("bid-below-minimum", price: format_amount(next_bid))));
                        (command.failure).set(None);
                        return;
                    }
                    command.begin(value.to_string(), |command_id| PendingBidCommand {
                        amount: value,
                        command_id,
                    });
                    validation.set(None);
                    return;
                };
                validation.set(None);
                let lot_id = lot_id.clone();
                command.submit(
                    pending,
                    move |pending| {
                        let lot_id = lot_id.clone();
                        async move { place_bid(lot_id, pending.amount, pending.command_id).await }
                    },
                    crate::api::CommandError::outcome_uncertain,
                    move |result| {
                        // 成功后面板代替直接关弹窗：写入成功投影并刷新权威价格与领先者。
                        success.set(Some(BidSuccess {
                            leading: result.leading,
                            current_price: result.current_price,
                        }));
                        refresh_generation.set(refresh_generation().saturating_add(1));
                    },
                    move || {
                        // 失败后 hook 保留输入、command_id 和错误，这里重新读取权威价格与领先者。
                        refresh_generation.set(refresh_generation().saturating_add(1));
                    },
                );
            },
            if let Some(pending) = (command.confirmation)() {
                p {
                    id: "public-bid-confirmation",
                    class: "public-auction-bid-form__confirmation",
                    role: "status",
                    aria_live: "polite",
                    aria_atomic: "true",
                    {t!(
                        "bid-confirm-description",
                        amount: format_amount(pending.amount),
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
            if let Some(error) = (command.failure)() {
                p { class: "action-feedback action-feedback--error", role: "alert",
                    "{bid_error_message(&error, next_bid)}"
                }
            }
            div { class: "public-auction-bid-form__actions",
                button {
                    r#type: "button",
                    disabled: submitting(),
                    onclick: move |_| {
                        if (command.confirmation)().is_some() {
                            command.cancel_confirmation();
                        } else {
                            on_close.call(());
                        }
                    },
                    if (command.confirmation)().is_some() {
                        {t!("bid-confirm-back")}
                    } else {
                        {t!("dialog-cancel")}
                    }
                }
                button {
                    r#type: "submit",
                    disabled: submitting() || blocked,
                    aria_describedby: if (command.confirmation)().is_some() {
                        "public-bid-confirmation"
                    } else {
                        ""
                    },
                    if submitting() {
                        {t!("bid-working")}
                    } else if (command.confirmation)().is_some() {
                        {t!("bid-confirm-submit")}
                    } else {
                        {t!("bid-submit")}
                    }
                }
            }
        }
    }
}
