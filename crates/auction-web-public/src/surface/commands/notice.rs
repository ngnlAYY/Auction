//! 外壳的未结命令列表和离开原弹窗后的命令回执。

use auction_web_shared::t;
use dioxus::prelude::*;
use uuid::Uuid;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::JsCast;

use super::host::{CommandStore, CompetitionCommands};
use super::request::CommandReceipt;
use super::retry::CommandRecovery;

const NOTICE_TITLE: &str = "public-command-notices-title";
const CONTENT: &str = "public-content";

#[component]
pub(super) fn CompetitionNotices() -> Element {
    let commands = use_context::<CompetitionCommands>();
    let store = commands.current();
    let unknown = store.as_ref().map_or_else(Vec::new, CommandStore::unknown_commands);
    let (receipts, latest_receipt) =
        store.as_ref().map_or_else(Default::default, CommandStore::receipts);
    let count = unknown.len();
    let title = t!("command-unknown-count", count: count);
    let mut has_announced = use_signal(|| false);
    use_effect(use_reactive((&count,), move |(count,)| {
        if count > 0 && !*has_announced.peek() {
            has_announced.set(true);
        }
    }));
    let latest_message =
        receipts.iter().find(|receipt| Some(receipt.command_id) == latest_receipt).map(|receipt| {
            (receipt.command_id, format!("{} · {}", receipt.target.lot_name, receipt.message()))
        });
    let rows = receipts.iter().enumerate().map(|(index, receipt)| {
        (receipt, receipt.command_id, receipt_focus(&receipts, index, count > 0))
    });

    rsx! {
        // 播报节点在首屏即挂载，变更只更新文本，不移动焦点或朗读操作按钮。
        div { class: "public-command-notices__status", role: "status", aria_atomic: "true",
            if count > 0 || has_announced() {
                span { "{title}" }
            }
        }
        div { class: "public-command-notices__status", role: "status", aria_atomic: "true",
            for (command_id, message) in latest_message {
                span { key: "{command_id}", "{message}" }
            }
        }
        if count > 0 {
            section { class: "public-command-notices", aria_labelledby: NOTICE_TITLE,
                h2 { id: NOTICE_TITLE, class: "public-command-notices__title", tabindex: "-1", "{title}" }
                p { class: "public-command-notices__boundary", {t!("command-memory-only")} }
                ul { class: "public-command-notices__list",
                    for command in unknown {
                        li { key: "{command.payload.command_id()}", class: "public-command-notices__item",
                            CommandRecovery { command }
                        }
                    }
                }
            }
        }
        if !receipts.is_empty() {
            ul { class: "public-command-notices__receipts", aria_label: t!("command-results-label"),
                for (receipt, command_id, focus) in rows {
                    li { key: "{receipt.command_id}", class: "public-command-notices__receipt",
                        p {
                            b { "{receipt.target.lot_name}" }
                            " · "
                            {receipt.kind.label()}
                            " · "
                            {receipt.message()}
                        }
                        button {
                            id: receipt_close_id(command_id),
                            class: "public-command-notices__receipt-close",
                            r#type: "button",
                            aria_label: t!("command-dismiss-result", lot: receipt.target.lot_name.clone()),
                            onclick: move |_| {
                                if let Some(store) = commands.current() {
                                    store.dismiss(command_id);
                                    focus_after_dismiss(&focus);
                                }
                            },
                            {t!("catalog-close")}
                        }
                    }
                }
            }
        }
    }
}

fn receipt_close_id(command_id: Uuid) -> String {
    format!("public-command-receipt-close-{command_id}")
}

fn receipt_focus(receipts: &[CommandReceipt], index: usize, has_unknown: bool) -> String {
    receipts
        .get(index + 1)
        .or_else(|| index.checked_sub(1).and_then(|index| receipts.get(index)))
        .map_or_else(
            || if has_unknown { NOTICE_TITLE } else { CONTENT }.to_owned(),
            |receipt| receipt_close_id(receipt.command_id),
        )
}

fn focus_after_dismiss(target: &str) {
    cfg_select! {
        target_arch = "wasm32" => {
            if let Some(document) = web_sys::window().and_then(|window| window.document())
                && let Some(element) = document
                    .get_element_by_id(target)
                    .or_else(|| document.get_element_by_id(CONTENT))
                    .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok())
            {
                let _ = element.focus();
            }
        }
        _ => { let _ = target; }
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use uuid::Uuid;

    use super::{CONTENT, NOTICE_TITLE, receipt_close_id, receipt_focus};
    use crate::surface::commands::request::{
        CommandCompletion, CommandKind, CommandReceipt, CommandTarget,
    };

    #[test]
    fn dismissing_a_receipt_focuses_a_remaining_receipt_or_page_landmark() {
        let target = Rc::new(CommandTarget {
            world: "main".to_owned(),
            auction_id: "auction".to_owned(),
            lot_id: "lot".to_owned(),
            lot_code: "S07".to_owned(),
            lot_name: "Shield".to_owned(),
        });
        let receipts = [1_u128, 2].map(|number| CommandReceipt {
            command_id: Uuid::from_u128(number),
            kind: CommandKind::Bid,
            target: target.clone(),
            completion: CommandCompletion::Accepted,
        });
        assert_eq!(receipt_focus(&receipts, 0, true), receipt_close_id(receipts[1].command_id));
        assert_eq!(receipt_focus(&receipts, 1, false), receipt_close_id(receipts[0].command_id));
        assert_eq!(receipt_focus(&receipts[..1], 0, true), NOTICE_TITLE);
        assert_eq!(receipt_focus(&receipts[..1], 0, false), CONTENT);
    }
}
