//! 未结命令的只读恢复视图与显式放弃确认。

use auction_web_shared::{Dialog, format_credit_amount, t};
use dioxus::prelude::*;

use super::host::{CommandDialogKey, CompetitionCommands};
use super::pending::{bid_error_message, proxy_error_message};
use super::request::{
    CommandCompletion, CommandKind, CommandReceipt, CommandStatus, SentCommand, command_conflict,
};
use crate::api::CommandError;
use crate::surface::{LotLinkLocation, LotLinkRequest, Route};

#[component]
pub(in crate::surface) fn CommandRecovery(
    command: SentCommand,
    #[props(default)] origin: Option<CommandDialogKey>,
) -> Element {
    let commands = use_context::<CompetitionCommands>();
    let mut link_request = use_context::<LotLinkRequest>().0;
    let can_retry = commands.current().is_some_and(|store| store.can_submit());
    let mut confirming = use_signal(|| false);
    let command_id = command.payload.command_id();
    let amount = format_credit_amount(command.payload.amount());
    let kind = command.payload.kind();
    let label = kind.label();
    let target = command.target;
    let in_dialog = origin.is_some();
    let summary_id = if in_dialog {
        format!("public-command-dialog-{}", target.lot_id)
    } else {
        format!("public-command-notice-{}", target.lot_id)
    };
    let busy = command.status.busy();
    let unknown = command.status.unknown();
    let paused = unknown && !can_retry;
    let message_id = format!("{summary_id}-status");
    let pause_id = format!("{summary_id}-pause");
    let retry_description = if paused {
        format!("{summary_id} {message_id} {pause_id}")
    } else {
        format!("{summary_id} {message_id}")
    };
    let error = match command.status {
        CommandStatus::Submitting => None,
        CommandStatus::Unknown { error, .. } if command_conflict(&error) => {
            Some(t!("command-replay-conflict"))
        }
        CommandStatus::Unknown { error, .. } => Some(command_error_message(kind, &error)),
    };
    let retry = use_callback({
        let target = target.clone();
        move |()| {
            if let Some(store) = commands.current() {
                store.retry(&target.lot_id, command_id, origin);
            }
        }
    });
    let ask_abandon = use_callback(move |()| {
        if !busy {
            confirming.set(true);
        }
    });
    let route = Route::LotDetail { lot_code: target.lot_id.clone() };
    let view_lot = use_callback({
        let target = target.clone();
        let href = route.to_string();
        move |()| {
            link_request.set(Some(LotLinkLocation {
                lot_id: target.lot_id.clone(),
                auction_id: target.auction_id.clone(),
                world: target.world.clone(),
            }));
            if navigator().push(href.clone()).is_some() {
                link_request.set(None);
            }
        }
    });
    #[cfg(test)]
    super::tests::observe_recovery(in_dialog, retry, ask_abandon);

    rsx! {
        div { class: "public-command-notices__entry",
            p { id: "{summary_id}", class: "public-command-notices__summary",
                b { "{target.lot_name}" }
                span { "{label}" }
                span { "{amount}" }
            }
            if in_dialog && unknown {
                p { {t!("command-retry-unchanged")} }
                p { {t!("command-memory-only")} }
            }
            // 忙碌标志放在操作组。状态节点持续可播报，结果条数不变时也能听到进度与原因。
            div { class: "public-command-notices__entry-status", role: "status", aria_atomic: "true", aria_labelledby: "{summary_id}",
                p { id: "{message_id}", class: "public-command-notices__message",
                    if busy {
                        if unknown { {t!("command-retrying")} } else { {t!("bid-working")} }
                    } else if let Some(error) = error {
                        "{error}"
                    }
                }
                if paused {
                    p { id: "{pause_id}", class: "public-command-notices__message", {t!("command-retry-session-pending")} }
                }
            }
            if unknown {
                div { class: "public-command-notices__actions", aria_busy: busy.to_string(),
                    button {
                        r#type: "button",
                        disabled: busy || !can_retry,
                        aria_describedby: "{retry_description}",
                        onclick: move |_| retry.call(()),
                        {t!("data-retry")}
                    }
                    if !in_dialog {
                        Link {
                            to: route,
                            onclick_only: true,
                            onclick: move |_| view_lot.call(()),
                            aria_label: t!("catalog-view-lot", code: target.lot_code.clone()),
                            {t!("command-view-lot")}
                        }
                    }
                    button {
                        r#type: "button",
                        disabled: busy,
                        aria_haspopup: "dialog",
                        aria_describedby: "{summary_id}",
                        onclick: move |_| ask_abandon.call(()),
                        {t!("command-abandon")}
                    }
                }
                if confirming() {
                    AbandonCommandDialog {
                        on_close: move |_| confirming.set(false),
                        on_confirm: move |_| {
                            if let Some(store) = commands.current() {
                                store.abandon(&target.lot_id, command_id);
                            }
                            confirming.set(false);
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn AbandonCommandDialog(on_close: EventHandler<()>, on_confirm: EventHandler<()>) -> Element {
    #[cfg(test)]
    super::tests::observe_confirmation(on_close, on_confirm);
    rsx! {
        Dialog {
            title: t!("command-abandon-title"),
            close_label: t!("catalog-close"),
            on_close,
            div { class: "public-command-notices__confirmation",
                p { {t!("command-abandon-description")} }
                div { class: "public-command-notices__actions",
                    button {
                        r#type: "button",
                        autofocus: true,
                        onclick: move |_| on_close.call(()),
                        {t!("dialog-cancel")}
                    }
                    button {
                        r#type: "button",
                        onclick: move |_| on_confirm.call(()),
                        {t!("command-abandon")}
                    }
                }
            }
        }
    }
}

/// 未知结果回显或跨路由拒绝不掌握当前最低价，不能用冻结金额替代价格下限。
fn retry_error_message(
    error: &CommandError,
    command_message: impl FnOnce(&CommandError) -> String,
) -> String {
    if error.code == "bid_too_low" {
        return t!("command-retry-price-changed");
    }
    command_message(error)
}

fn command_error_message(kind: CommandKind, error: &CommandError) -> String {
    if error.not_sent() {
        return t!("command-not-sent");
    }
    retry_error_message(error, |error| match kind {
        CommandKind::Bid => bid_error_message(error, 0),
        CommandKind::CreateProxy | CommandKind::LowerProxy => proxy_error_message(error, 0),
    })
}

impl CommandReceipt {
    pub(in crate::surface) fn message(&self) -> String {
        match &self.completion {
            CommandCompletion::Rejected(error) => command_error_message(self.kind, error),
            CommandCompletion::Accepted => match self.kind {
                CommandKind::Bid => t!("bid-success"),
                CommandKind::CreateProxy | CommandKind::LowerProxy => t!("proxy-saved"),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use auction_web_shared::i18n::{DEFAULT_PUBLIC_LOCALE, build_bundle};
    use dioxus::prelude::*;
    use dioxus_i18n::prelude::try_use_init_i18n_from_bundle;

    use super::{
        CommandError, bid_error_message, format_credit_amount, proxy_error_message,
        retry_error_message,
    };
    use crate::surface::router::PUBLIC_LOCALE_CATALOG;

    /// 用户在结果未知前提交的金额。
    const SUBMITTED: i64 = 50_000;

    thread_local! {
        static MESSAGES: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }

    fn error(code: &str) -> CommandError {
        CommandError { code: code.to_owned(), status: 409 }
    }

    /// 在挂载英文译文的组件里生成重试文案：出价被拒、代理被拒、出价的其他错误。
    #[component]
    fn RetryMessages() -> Element {
        let bundle = try_use_init_i18n_from_bundle(|| {
            build_bundle(
                &DEFAULT_PUBLIC_LOCALE,
                PUBLIC_LOCALE_CATALOG.default_ftl,
                &DEFAULT_PUBLIC_LOCALE,
                None,
            )
        });
        if bundle.is_ok() {
            let too_low = error("bid_too_low");
            let leading = error("already_leading");
            let messages = vec![
                retry_error_message(&too_low, |error| bid_error_message(error, SUBMITTED)),
                retry_error_message(&too_low, |error| proxy_error_message(error, SUBMITTED)),
                retry_error_message(&leading, |error| bid_error_message(error, SUBMITTED)),
                bid_error_message(&leading, SUBMITTED),
            ];
            MESSAGES.with(|rendered| *rendered.borrow_mut() = messages);
        }
        rsx! {}
    }

    #[test]
    fn price_change_on_retry_does_not_quote_the_submitted_amount_as_minimum() {
        let mut dom = VirtualDom::new(RetryMessages);
        dom.rebuild_in_place();
        let messages = MESSAGES.with(RefCell::take);
        assert_eq!(messages.len(), 4, "译文未能初始化");
        let submitted = format_credit_amount(SUBMITTED);
        for message in &messages[..2] {
            assert!(!message.contains(&submitted), "{message}");
            assert!(message.contains("price has changed"), "{message}");
        }
        // 其他错误码与新命令的映射一致。
        assert_eq!(messages[2], messages[3]);
    }
}
