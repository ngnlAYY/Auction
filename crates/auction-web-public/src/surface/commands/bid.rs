//! 普通竞价表单和命令提交。

use auction_contracts::MAX_PUBLIC_BID_AMOUNT;
use auction_web_shared::t;
use dioxus::prelude::*;

use crate::api::place_bid;

use super::super::format_credit_amount;
use super::common::{
    BidConfirmCommand, PendingBidCommand, PublicCommandIdentity, bid_error_message, confirm_panel,
};

const CREDIT_K: i64 = 1_000;

/// 普通竞价成功后的浏览器展示投影；只保留成功提示所需字段，避免把 api 内部 DTO
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
    pub next_bid: i64,
    pub lot_status: String,
    pub can_bid: bool,
    pub blocked: bool,
    pub blocked_reason: String,
    /// 由弹窗宿主注入的两阶段确认命令 hook（整体 Copy）；宿主据其 `submitting`
    /// 在提交中禁用遮罩/Escape/关闭按钮（Dialog busy 守卫），避免弹窗 scope 销毁
    /// 导致成功后的刷新与成功反馈丢失。
    pub command: BidConfirmCommand,
    /// 命令成功后把结果交给弹窗宿主：宿主关闭弹窗并显示页面级提示条。表单自身不保留
    /// 成功状态，也不决定弹窗生命周期。
    pub on_success: EventHandler<BidSuccess>,
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
                "= {format_credit_amount(amount)}"
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

/// 为表单建立独立组件作用域，避免弹窗开关改变父组件的 Hook 顺序。
#[component]
pub fn BidForm(input: BidFormInput) -> Element {
    bid_form(input)
}

/// 显示普通竞价表单；只由 BidForm 无条件调用，最低价与 lot 状态的最终判断留在服务端。
fn bid_form(input: BidFormInput) -> Element {
    let BidFormInput {
        lot_id,
        next_bid,
        lot_status,
        can_bid,
        blocked,
        blocked_reason,
        command,
        on_success,
        on_close,
    } = input;
    let amount_id = format!("public-bid-amount-{lot_id}");
    let user_id = use_context::<PublicCommandIdentity>().0;
    let mut amount = use_signal(|| default_bid_units(next_bid));
    let mut validation = use_signal(|| None::<String>);
    // 两阶段确认状态（command/confirmation/submitting/failure）由宿主注入的 hook 持有。
    let mut command = command;
    let submitting = command.submitting;
    let mut refresh_generation = use_context::<Signal<u64>>();
    let confirming = (command.confirmation)().is_some();
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
                        validation.set(Some(t!("bid-below-minimum", price: format_credit_amount(next_bid))));
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
                        validation.set(Some(t!("bid-below-minimum", price: format_credit_amount(next_bid))));
                        (command.failure).set(None);
                        return;
                    }
                    command.begin(value.to_string(), |command_id| PendingBidCommand {
                        user_id,
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
                        let lot_id = lot_id;
                        async move {
                            place_bid(pending.user_id, lot_id, pending.amount, pending.command_id)
                                .await
                        }
                    },
                    crate::api::CommandError::outcome_uncertain,
                    move |result| {
                        // 先重读权威价格与领先者，再把结果交给宿主；宿主随即关闭弹窗，
                        // 所以这里之后不能再依赖表单自身的状态。
                        refresh_generation.set(refresh_generation().saturating_add(1));
                        on_success.call(BidSuccess {
                            leading: result.leading,
                            current_price: result.current_price,
                        });
                    },
                    move || {
                        // 失败后 hook 保留输入、command_id 和错误，这里重新读取权威价格与领先者。
                        refresh_generation.set(refresh_generation().saturating_add(1));
                    },
                );
            },
            if let Some(pending) = (command.confirmation)() {
                {confirm_panel(
                    "public-bid-confirmation",
                    &t!("bid-tab-direct"),
                    format_credit_amount(pending.amount),
                    t!("bid-binding-note"),
                )}
            } else {
                // 当前价与最低价已由弹窗顶部的拍品摘要展示，表单内不再重复。
                label { for: amount_id.clone(), {t!("bid-amount-label")} }
                input {
                    id: amount_id,
                    name: "amount",
                    r#type: "text",
                    autofocus: true,
                    inputmode: "numeric",
                    enterkeyhint: "done",
                    pattern: "[0-9]*",
                    autocomplete: "off",
                    spellcheck: "false",
                    aria_invalid: validation().is_some().to_string(),
                    aria_describedby: if validation().is_some() {
                        "public-bid-amount-hint public-bid-unit-hint public-bid-error"
                    } else {
                        "public-bid-amount-hint public-bid-unit-hint"
                    },
                    disabled: submitting() || blocked,
                    value: "{amount}",
                    // 打开弹窗时由焦点陷阱按 autofocus 聚焦；从确认视图“返回修改”或提交被拒后
                    // 金额框重新挂载，此时焦点还留在按钮上，这里把它交还给要修改的金额。
                    onmounted: move |event| {
                        let input = event.data();
                        async move {
                            let _ = input.set_focus(true).await;
                        }
                    },
                    oninput: move |event| {
                        amount.set(event.value());
                        validation.set(None);
                    },
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
                p { id: "public-bid-error", class: "action-feedback action-feedback--error", role: "alert", "{message}" }
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
                    if confirming {
                        {t!("bid-confirm-back")}
                    } else {
                        {t!("dialog-cancel")}
                    }
                }
                button {
                    r#type: "submit",
                    // 用 Enter 提交金额后输入框被确认面板替换；确认视图的首选焦点是确认按钮，
                    // 而不是弹窗右上角的关闭按钮。编辑态的首选焦点仍是金额框。
                    autofocus: confirming,
                    disabled: submitting() || blocked,
                    aria_describedby: if confirming { "public-bid-confirmation" } else { "" },
                    if submitting() {
                        {t!("bid-working")}
                    } else if confirming {
                        {t!("bid-confirm-submit")}
                    } else {
                        {t!("bid-submit")}
                    }
                }
            }
        }
    }
}
