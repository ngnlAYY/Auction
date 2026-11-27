//! 普通竞价表单和命令提交。

use auction_contracts::MAX_PUBLIC_BID_AMOUNT;
use auction_web_shared::{format_credit_amount, t};
use dioxus::prelude::*;

use super::host::CompetitionCommands;
use super::pending::{BidConfirmCommand, PendingBidCommand, PublicCommandIdentity, confirm_panel};
use super::session::CommandSession;

const CREDIT_K: i64 = 1_000;
// 只限制字符写法，空值和金额边界由表单使用站内语言提示。
const BID_UNITS_PATTERN: &str = r"[ \x09-\x0D]*[0-9]*[ \x09-\x0D]*";

/// 普通竞价表单参数。表单只确认负载，发出后由外壳持有请求任务。
#[derive(Clone, PartialEq)]
pub struct BidFormInput {
    pub lot_id: String,
    pub next_bid: i64,
    pub lot_status: String,
    pub can_bid: bool,
    pub blocked: bool,
    pub blocked_reason: String,
    pub command: BidConfirmCommand,
    pub on_submit: EventHandler<PendingBidCommand>,
    pub on_close: EventHandler<()>,
}

/// 普通出价输入固定使用 K 单位；向上取整保证非整千的旧起拍价不会生成低于最低价的默认值。
fn default_bid_units(next_bid: i64) -> String {
    rounded_minimum_bid(next_bid).map_or_else(String::new, |amount| (amount / CREDIT_K).to_string())
}

fn rounded_minimum_bid(next_bid: i64) -> Option<i64> {
    if next_bid <= 0 {
        return None;
    }
    let units = next_bid / CREDIT_K + i64::from(next_bid % CREDIT_K != 0);
    let amount = units.checked_mul(CREDIT_K)?;
    (amount <= MAX_PUBLIC_BID_AMOUNT).then_some(amount)
}

/// 普通出价只接受纯数字 K 单位，避免把代理表单保留的 M/K 缩写规则扩散到确认流程。
fn parse_bid_units(input: &str) -> Option<i64> {
    let units = trim_bid_units(input);
    if units.is_empty() || !units.chars().all(|character| character.is_ascii_digit()) {
        return None;
    }
    let amount = units.parse::<i64>().ok()?.checked_mul(CREDIT_K)?;
    (amount > 0 && amount <= MAX_PUBLIC_BID_AMOUNT).then_some(amount)
}

fn trim_bid_units(input: &str) -> &str {
    input.trim_matches(|character| matches!(character, ' ' | '\t'..='\r'))
}

fn bid_amount_hint(value: &str) -> Element {
    if trim_bid_units(value).is_empty() {
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
        on_submit,
        on_close,
    } = input;
    let amount_id = format!("public-bid-amount-{lot_id}");
    let user_id = use_context::<PublicCommandIdentity>().0;
    let session = use_context::<CommandSession>();
    let can_submit = session.can_submit_new();
    let commands = try_use_context::<CompetitionCommands>();
    let mut amount = use_signal(|| default_bid_units(next_bid));
    let mut validation = use_signal(|| None::<String>);
    let mut command = command;
    let confirming = (command.confirmation)().is_some();
    let bidding_open = can_bid && lot_status == "live";
    let limit_reached = rounded_minimum_bid(next_bid).is_none();
    let blocked = blocked || !bidding_open || limit_reached;
    let blocked_reason = if !bidding_open {
        t!("bid-unavailable")
    } else if limit_reached && blocked_reason.is_empty() {
        t!("bid-limit-note")
    } else {
        blocked_reason
    };
    // 没有说明文字（如已有代理授权）时不渲染说明节点。
    let show_blocked_reason = blocked && !blocked_reason.is_empty();
    let submit_described_by =
        if confirming { "public-bid-confirmation".to_owned() } else { String::new() };

    let submit = use_callback(move |()| {
        if blocked
            || !session.can_submit_new()
            || !commands.is_some_and(|commands| commands.can_start(&lot_id))
        {
            return;
        }
        // 只有尚未发送的表单可以准备命令；外壳再次检查同拍品互斥后才发送。
        if let Some(pending) = (command.confirmation)() {
            if pending.amount <= 0
                || pending.amount > MAX_PUBLIC_BID_AMOUNT
                || pending.amount % CREDIT_K != 0
            {
                command.cancel_confirmation();
                validation.set(Some(t!("bid-invalid-amount")));
                return;
            }
            if pending.amount < next_bid {
                command.cancel_confirmation();
                validation
                    .set(Some(t!("bid-below-minimum", price: format_credit_amount(next_bid))));
                return;
            }
            validation.set(None);
            on_submit.call(pending);
            return;
        }
        let Some(value) = parse_bid_units(&amount()) else {
            validation.set(Some(t!("bid-invalid-amount")));
            return;
        };
        if value < next_bid {
            validation.set(Some(t!("bid-below-minimum", price: format_credit_amount(next_bid))));
            return;
        }
        command.begin(value.to_string(), |command_id| PendingBidCommand {
            user_id,
            amount: value,
            command_id,
        });
        validation.set(None);
    });
    #[cfg(test)]
    super::tests::test_support::BID_SUBMIT.set(Some(submit));

    rsx! {
        form {
            class: "public-auction-bid-form",
            onsubmit: move |event| {
                event.prevent_default();
                submit.call(());
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
                    pattern: BID_UNITS_PATTERN,
                    autocomplete: "off",
                    spellcheck: "false",
                    aria_invalid: validation().is_some().to_string(),
                    aria_describedby: if validation().is_some() {
                        "public-bid-amount-hint public-bid-unit-hint public-bid-error"
                    } else {
                        "public-bid-amount-hint public-bid-unit-hint"
                    },
                    disabled: blocked,
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
            if show_blocked_reason {
                p { class: "action-feedback action-feedback--notice", role: "note", "{blocked_reason}" }
            }
            if let Some(message) = validation() {
                p { id: "public-bid-error", class: "action-feedback action-feedback--error", role: "alert", "{message}" }
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
                    disabled: blocked || !can_submit,
                    aria_describedby: submit_described_by,
                    if confirming {
                        {t!("bid-confirm-submit")}
                    } else {
                        {t!("bid-submit")}
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use auction_contracts::MAX_PUBLIC_BID_AMOUNT;
    use auction_web_shared::i18n::{DEFAULT_PUBLIC_LOCALE, build_bundle};
    use auction_web_shared::use_confirm_command;
    use dioxus::dioxus_core::{AttributeValue, Mutation, Mutations};
    use dioxus::prelude::*;
    use dioxus_i18n::prelude::try_use_init_i18n_from_bundle;
    use uuid::Uuid;

    use super::{BID_UNITS_PATTERN, BidForm, BidFormInput, CREDIT_K, parse_bid_units};
    use crate::surface::commands::pending::{BidConfirmCommand, PublicCommandIdentity};
    use crate::surface::commands::session::use_command_session_provider;
    use crate::surface::router::PUBLIC_LOCALE_CATALOG;
    use crate::surface::{PublicCommandOwner, PublicSessionResource};

    #[test]
    fn amount_input_pattern_validates_only_token_syntax() {
        assert_eq!(BID_UNITS_PATTERN, r"[ \x09-\x0D]*[0-9]*[ \x09-\x0D]*");
    }

    #[component]
    fn AmountInputFixture() -> Element {
        try_use_init_i18n_from_bundle(|| {
            build_bundle(
                &DEFAULT_PUBLIC_LOCALE,
                PUBLIC_LOCALE_CATALOG.default_ftl,
                &DEFAULT_PUBLIC_LOCALE,
                None,
            )
        })
        .expect("加载公开端英文译文");
        let session: PublicSessionResource = use_resource(|| async { Ok(None) });
        use_context_provider(|| session);
        use_context_provider(|| PublicCommandOwner(Signal::new(None)));
        use_context_provider(|| PublicCommandIdentity(Uuid::nil()));
        use_context_provider(|| Signal::new(0_u64));
        use_command_session_provider(None);
        let input = BidFormInput {
            lot_id: "test-lot".to_owned(),
            next_bid: 50_000,
            lot_status: "live".to_owned(),
            can_bid: true,
            blocked: false,
            blocked_reason: String::new(),
            command: BidConfirmCommand(use_confirm_command()),
            on_submit: use_callback(|_| {}),
            on_close: use_callback(|()| {}),
        };
        rsx! { BidForm { input } }
    }

    #[test]
    fn amount_input_renders_the_lexical_pattern() {
        let mut dom = VirtualDom::new(AmountInputFixture);
        let mut mutations = Mutations::default();
        dom.rebuild(&mut mutations);
        let amount = mutations
            .edits
            .iter()
            .find_map(|edit| match edit {
                Mutation::SetAttribute {
                    name: "id",
                    id,
                    value: AttributeValue::Text(value),
                    ..
                } if value == "public-bid-amount-test-lot" => Some(*id),
                _ => None,
            })
            .expect("金额输入框已渲染");
        let pattern = mutations.edits.iter().find_map(|edit| match edit {
            Mutation::SetAttribute {
                name: "pattern",
                id,
                value: AttributeValue::Text(value),
                ..
            } if *id == amount => Some(value.as_str()),
            _ => None,
        });
        assert_eq!(pattern, Some(r"[ \x09-\x0D]*[0-9]*[ \x09-\x0D]*"));
    }

    #[test]
    fn surrounding_ascii_whitespace_and_leading_zeros_are_accepted() {
        assert_eq!(parse_bid_units(" \t\n\x0b\r\x0c0050 \t"), Some(50 * CREDIT_K));
        let maximum_units = MAX_PUBLIC_BID_AMOUNT / CREDIT_K;
        assert_eq!(parse_bid_units(&format!(" 000{maximum_units} ")), Some(MAX_PUBLIC_BID_AMOUNT));
    }

    #[test]
    fn only_ascii_whitespace_and_in_range_ascii_integers_are_accepted() {
        for invalid in [
            "",
            " \t ",
            "0",
            "5 0",
            "5\x0b0",
            "50\u{a0}",
            "\u{3000}50",
            "５０",
            "1,000",
            "5.0",
            "+50",
        ] {
            assert_eq!(parse_bid_units(invalid), None, "{invalid:?}");
        }
        let above_maximum = MAX_PUBLIC_BID_AMOUNT / CREDIT_K + 1;
        assert_eq!(parse_bid_units(&above_maximum.to_string()), None);
        assert_eq!(parse_bid_units("99999999999999999999"), None);
    }
}
