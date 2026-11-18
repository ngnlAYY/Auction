//! 代理授权创建表单及其提交状态。

use auction_contracts::MAX_PUBLIC_BID_AMOUNT;
use auction_web_shared::{format_credit_amount, t};
use dioxus::prelude::*;

use super::super::super::parse_credit_amount;
use super::super::host::CompetitionCommands;
use super::super::pending::{
    PendingProxyCommand, ProxyConfirmCommand, PublicCommandIdentity, amount_hint, confirm_panel,
    initial_proxy_amount,
};
use super::super::session::CommandSession;

#[component]
pub(super) fn ProxyCreateForm(
    lot_id: String,
    initial_minimum: i64,
    lot_status: String,
    allow_slow: bool,
    /// 该场冻结策略的代理默认模式；未知时表单失败关闭。
    default_mode: Option<String>,
    blocked: bool,
    blocked_reason: String,
    authorization_ready: bool,
    on_submit: EventHandler<PendingProxyCommand>,
    /// 由弹窗宿主注入的两阶段确认命令 hook（创建实例；见 ProxyFormInput）。
    command: ProxyConfirmCommand,
    on_close: EventHandler<()>,
) -> Element {
    let user_id = use_context::<PublicCommandIdentity>().0;
    let session = use_context::<CommandSession>();
    let can_submit = authorization_ready && session.can_submit_new();
    let commands = try_use_context::<CompetitionCommands>();
    let maximum_id = format!("public-proxy-maximum-{lot_id}");
    let mode_name = format!("public-proxy-mode-{lot_id}");
    let fast_mode_id = format!("{mode_name}-fast");
    let slow_mode_id = format!("{mode_name}-slow");
    let fast_hint_id = format!("{mode_name}-fast-hint");
    let slow_hint_id = format!("{mode_name}-slow-hint");
    let slow_reason_id = format!("{mode_name}-slow-disabled");
    let slow_described_by =
        if allow_slow { slow_hint_id.clone() } else { format!("{slow_hint_id} {slow_reason_id}") };
    let mut maximum_amount = use_signal(|| initial_proxy_amount(initial_minimum));
    let initial_mode = initial_proxy_mode(default_mode.as_deref(), allow_slow);
    // 策略未知时整张表单禁用（见下方 `blocked`），占位选项不会被提交。
    let mut mode = use_signal(move || initial_mode.unwrap_or("fast").to_owned());
    let mut validation = use_signal(|| None::<String>);
    #[cfg(test)]
    super::tests::observe_draft(maximum_amount, mode, validation);
    let mut command = command;
    // 冻结策略的默认模式未知（只有往期目录拼装的已结算场次）时不提供新建，不替玩家假设模式。
    let bidding_open = lot_status == "live" && initial_mode.is_some();
    let limit_reached = initial_minimum > MAX_PUBLIC_BID_AMOUNT;
    let blocked = blocked || !bidding_open || limit_reached;
    let blocked_reason = if !bidding_open {
        t!("bid-unavailable")
    } else if limit_reached {
        t!("bid-limit-note")
    } else {
        blocked_reason
    };
    let confirmation_id = format!("public-proxy-create-confirmation-{lot_id}");
    let confirmation = match (command.confirmation)() {
        Some(PendingProxyCommand::Create { amount, mode, .. }) => {
            let mode_label =
                if mode == "slow" { t!("proxy-mode-slow") } else { t!("proxy-mode-fast") };
            Some((amount, mode_label))
        }
        _ => None,
    };
    let confirming = confirmation.is_some();

    let submit = use_callback(move |()| {
        if blocked
            || !authorization_ready
            || !session.can_submit_new()
            || !commands.is_some_and(|commands| commands.can_start(&lot_id))
        {
            return;
        }
        // 两阶段确认：签名取 (amount, mode)，同输入重试复用同一 command_id。
        let pending = if let Some(pending) = (command.confirmation)() {
            match &pending {
                PendingProxyCommand::Create { amount, mode, .. } => {
                    if *amount > MAX_PUBLIC_BID_AMOUNT {
                        command.cancel_confirmation();
                        validation.set(Some(t!("proxy-invalid-amount")));
                        return;
                    }
                    if *amount < initial_minimum {
                        command.cancel_confirmation();
                        validation.set(Some(t!(
                            "proxy-below-minimum",
                            price: format_credit_amount(initial_minimum)
                        )));
                        return;
                    }
                    if mode == "slow" && !allow_slow {
                        command.cancel_confirmation();
                        validation.set(Some(t!("proxy-slow-forbidden")));
                        return;
                    }
                }
                // 本表单只冻结 Create；防御性处理串型，退回编辑态。
                PendingProxyCommand::Lower { .. } => {
                    command.reset();
                    return;
                }
            }
            pending
        } else {
            let Some(value) = parse_credit_amount(&maximum_amount()) else {
                validation.set(Some(t!("proxy-invalid-amount")));
                return;
            };
            if value < initial_minimum {
                validation.set(Some(t!(
                    "proxy-below-minimum",
                    price: format_credit_amount(initial_minimum)
                )));
                return;
            }
            let selected_mode = mode();
            if selected_mode == "slow" && !allow_slow {
                validation.set(Some(t!("proxy-slow-forbidden")));
                return;
            }
            command.begin(format!("{value}:{selected_mode}"), |command_id| {
                PendingProxyCommand::Create {
                    user_id,
                    amount: value,
                    mode: selected_mode,
                    command_id,
                }
            });
            validation.set(None);
            return;
        };
        validation.set(None);
        on_submit.call(pending);
    });
    rsx! {
        section {
            class: "public-auction-bid-form",
            form {
                onsubmit: move |event| {
                    event.prevent_default();
                    submit.call(());
                },
                if let Some((amount, mode_label)) = &confirmation {
                    {confirm_panel(
                        &confirmation_id,
                        &format!("{} · {}", t!("bid-tab-proxy"), mode_label),
                        format_credit_amount(*amount),
                        t!("proxy-binding-note"),
                    )}
                } else {
                    // 当前价与最低价已由弹窗顶部的拍品摘要展示，表单内不再重复。
                    label { for: maximum_id.clone(), {t!("proxy-create-maximum-label")} }
                    input {
                        id: maximum_id,
                        name: "maximum_amount",
                        r#type: "text",
                        autofocus: true,
                        // 代理上限允许 K/M 缩写，手机键盘必须保留字母输入入口。
                        inputmode: "text",
                        enterkeyhint: "done",
                        autocapitalize: "none",
                        autocomplete: "off",
                        spellcheck: "false",
                        aria_invalid: validation().is_some().to_string(),
                        aria_describedby: if validation().is_some() {
                            "public-proxy-amount-hint public-proxy-error"
                        } else {
                            "public-proxy-amount-hint"
                        },
                        disabled: blocked,
                        value: "{maximum_amount}",
                        // 与直接出价一致：从确认视图“返回修改”或提交被拒后上限框重新挂载，
                        // 此时焦点还留在按钮上，这里把它交还给要修改的上限。
                        onmounted: move |event| {
                            let input = event.data();
                            async move {
                                let _ = input.set_focus(true).await;
                            }
                        },
                        oninput: move |event| {
                            maximum_amount.set(event.value());
                            validation.set(None);
                        },
                    }
                    {amount_hint(
                        &maximum_amount(),
                        "public-proxy-amount-hint",
                        "proxy-invalid-amount",
                    )}
                    p { class: "public-auction-bid-form__meta", {t!("proxy-maximum-hint")} }
                    p { class: "public-auction-bid-form__hint", {t!("proxy-shorthand-hint")} }
                    fieldset {
                        disabled: blocked,
                        legend { {t!("proxy-create-mode-label")} }
                        div { class: "public-auction-bid-form__mode",
                            input {
                                id: fast_mode_id.clone(),
                                r#type: "radio",
                                name: mode_name.clone(),
                                value: "fast",
                                checked: mode() == "fast",
                                aria_describedby: fast_hint_id.clone(),
                                onchange: move |_| mode.set("fast".to_owned()),
                            }
                            label { for: fast_mode_id, {t!("proxy-mode-fast")} }
                            p { id: fast_hint_id, class: "public-auction-bid-form__mode-hint",
                                {t!("proxy-mode-fast-hint")}
                            }
                        }
                        div { class: "public-auction-bid-form__mode",
                            input {
                                id: slow_mode_id.clone(),
                                r#type: "radio",
                                name: mode_name,
                                value: "slow",
                                checked: mode() == "slow",
                                disabled: !allow_slow,
                                aria_describedby: slow_described_by,
                                onchange: move |_| mode.set("slow".to_owned()),
                            }
                            label { for: slow_mode_id, {t!("proxy-mode-slow")} }
                            p { id: slow_hint_id, class: "public-auction-bid-form__mode-hint",
                                {t!("proxy-mode-slow-hint")}
                            }
                        }
                    }
                    if !allow_slow {
                        p { id: slow_reason_id, class: "public-auction-bid-form__hint", role: "note",
                            {t!("proxy-slow-forbidden")}
                        }
                    }
                }
                if blocked {
                    p { class: "action-feedback action-feedback--notice", role: "note", "{blocked_reason}" }
                }
                if let Some(message) = validation() {
                    p { id: "public-proxy-error", class: "action-feedback action-feedback--error", role: "alert", "{message}" }
                }
                div { class: "public-auction-bid-form__actions",
                    button {
                        r#type: "button",
                        onclick: move |_| {
                            if (command.confirmation)().is_some() {
                                command.cancel_confirmation();
                            } else {
                                on_close.call(());
                            }
                        },
                        if confirming { {t!("proxy-confirm-back")} } else { {t!("dialog-cancel")} }
                    }
                    button {
                        r#type: "submit",
                        // 确认视图的首选焦点是确认按钮；否则焦点陷阱会落到弹窗右上角的关闭按钮，
                        // 再按一次 Enter 就成了关闭弹窗。编辑态的首选焦点仍是上限框。
                        autofocus: confirming,
                        disabled: blocked || !can_submit,
                        aria_describedby: if confirming { confirmation_id } else { String::new() },
                        if confirming {
                            {t!("proxy-create-confirm-submit")}
                        } else {
                            {t!("proxy-create-submit")}
                        }
                    }
                }
            }
        }
    }
}

/// 代理新建表单的初始模式：取该场冻结策略的默认模式；慢速当前不可新建（终阶段）时退为快速，
/// 玩家仍可切换，提交照常显式携带模式。策略未知或不在闭集内时返回 `None`，由表单失败关闭。
fn initial_proxy_mode(default_mode: Option<&str>, allow_slow: bool) -> Option<&'static str> {
    match default_mode? {
        "slow" if allow_slow => Some("slow"),
        "fast" | "slow" => Some("fast"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::initial_proxy_mode;

    /// 初始选项跟随冻结策略：slow 策略在可新建慢速代理时默认 slow，终阶段退为 fast；fast 策略
    /// 总是 fast。策略缺失或闭集外的值不给初始选项，表单据此禁用。
    #[test]
    fn initial_mode_follows_the_frozen_default() {
        assert_eq!(initial_proxy_mode(Some("slow"), true), Some("slow"));
        assert_eq!(initial_proxy_mode(Some("slow"), false), Some("fast"));
        assert_eq!(initial_proxy_mode(Some("fast"), true), Some("fast"));
        assert_eq!(initial_proxy_mode(Some("fast"), false), Some("fast"));
        assert_eq!(initial_proxy_mode(None, true), None);
        assert_eq!(initial_proxy_mode(Some("Slow"), true), None);
        assert_eq!(initial_proxy_mode(Some(""), true), None);
    }
}
