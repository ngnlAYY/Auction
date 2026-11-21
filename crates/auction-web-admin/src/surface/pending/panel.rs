//! Admin 来源页未决面板与认证外壳的恢复、回执入口。

use auction_web_shared::{Dialog, TruncatedText, t};
use dioxus::prelude::*;
use uuid::Uuid;

use super::{CommandIdentity, CommandReceipt, PendingCommandHost, PendingCommands};
use crate::surface::admin_api_error_message;
use crate::surface::auth::generated_key_visibility;
use crate::surface::users::AdminUserGeneratedKeyDialog;

/// 来源页内的操作区读取会话宿主的同一条命令，并展示本页的拒绝原因。
#[component]
pub(crate) fn PendingCommandPanel(owner: PendingCommands) -> Element {
    let source = owner.source.read();
    let entries = owner.host.entries.read();
    let rejections = source.rejections.read();
    let active = entries.iter().filter(|entry| entry.belongs_to(&source));
    let has_active = active.clone().next().is_some();
    let writes_blocked = !crate::api::admin_session_writes_allowed();
    let rows = active
        .map(|entry| {
            (entry.identity.command_id, &entry.identity, Some(entry), entry.error.as_deref())
        })
        .chain(rejections.iter().map(|notice| {
            (
                notice.identity.command_id,
                &notice.identity,
                None,
                notice.result.as_ref().err().map(String::as_str),
            )
        }));
    rsx! {
        if has_active || !rejections.is_empty() {
            section { class: "admin-panel admin-pending", aria_label: t!("admin-pending-title"),
                h2 { {t!("admin-pending-title")} }
                if has_active {
                    p { class: "admin-panel__note", {t!("admin-pending-session-help")} }
                }
                ul { class: "admin-pending__list",
                    for (command_id, identity, active, error) in rows {
                        li { class: "admin-pending__entry", key: "{command_id}",
                            "data-command-id": "{command_id}",
                            {command_identity(identity)}
                            div { class: "admin-pending__result",
                                p { role: "status",
                                    if active.is_some_and(|entry| entry.busy) { {t!("admin-pending-busy")} }
                                    if active.is_some_and(|entry| entry.unknown) { {t!("admin-pending-unknown")} }
                                    if active.is_none() { {t!("admin-pending-rejected")} }
                                }
                                if let Some(code) = error {
                                    p { class: "action-feedback action-feedback--error",
                                        {admin_api_error_message(code)}
                                    }
                                }
                            }
                            div { class: "admin-pending__actions",
                                if let Some(entry) = active.filter(|entry| entry.unknown) {
                                    button { r#type: "button",
                                        disabled: writes_blocked || entry.retry_blocked(owner.host),
                                        onclick: move |_| owner.host.retry(command_id),
                                        {t!("admin-pending-retry")}
                                    }
                                } else if active.is_none() {
                                    button { r#type: "button",
                                        onclick: move |_| owner.dismiss(command_id),
                                        {t!("admin-pending-dismiss")}
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

fn command_identity(identity: &CommandIdentity) -> Element {
    rsx! {
        div { class: "admin-pending__identity",
            strong { TruncatedText { value: identity.action.clone() } }
            TruncatedText { value: identity.entity.clone() }
            code { TruncatedText { value: identity.command_id.to_string() } }
        }
    }
}

/// 每个认证路由均可恢复未知命令；回执保留到操作员关闭，播报区始终挂载。
#[component]
pub(crate) fn PendingCommandNotices(host: PendingCommandHost) -> Element {
    let mut confirm_abandon = use_signal(|| None::<Uuid>);
    let entries = host.entries.read();
    let unknown = entries.iter().filter(|entry| entry.unknown);
    let count = unknown.clone().count() as i64;
    let retrying = unknown.clone().filter(|entry| entry.busy).count() as i64;
    let rows = unknown.map(|entry| (entry.identity.command_id, entry));
    let writes_blocked = !crate::api::admin_session_writes_allowed();
    let abandon = confirm_abandon().and_then(|command_id| {
        entries
            .iter()
            .find(|entry| entry.identity.command_id == command_id && entry.unknown && !entry.busy)
    });
    rsx! {
        p { class: "admin-command-notices__status", role: "status", aria_atomic: "true",
            {t!("admin-pending-announcement", count: count, retrying: retrying)}
        }
        if count > 0 {
            section { class: "admin-panel admin-pending admin-command-notices", aria_labelledby: "admin-command-notices-title",
                h2 { id: "admin-command-notices-title", {t!("admin-pending-unknown-count", count: count)} }
                p { class: "admin-panel__note", {t!("admin-pending-session-help")} }
                ul { class: "admin-pending__list",
                    for (command_id, entry) in rows {
                        li { class: "admin-pending__entry", key: "{command_id}", "data-command-id": "{command_id}",
                            {command_identity(&entry.identity)}
                            div { class: "admin-pending__result",
                                if entry.busy { p { {t!("admin-pending-busy")} } }
                                if let Some(code) = &entry.error {
                                    p { class: "action-feedback action-feedback--error", {admin_api_error_message(code)} }
                                }
                            }
                            div { class: "admin-pending__actions",
                                button { id: record_button_id(command_id, "retry"), r#type: "button", disabled: writes_blocked || entry.retry_blocked(host),
                                    onclick: move |_| host.retry(command_id), {t!("admin-pending-retry")}
                                }
                                button { id: record_button_id(command_id, "abandon"), r#type: "button", disabled: entry.busy,
                                    onclick: move |_| confirm_abandon.set(Some(command_id)), {t!("admin-pending-abandon")}
                                }
                            }
                        }
                    }
                }
            }
        }
        if let Some(entry) = abandon {
            Dialog {
                title: t!("admin-pending-abandon-title"), close_label: t!("admin-pending-keep"),
                on_close: move |_| confirm_abandon.set(None),
                {command_identity(&entry.identity)}
                p { {t!("admin-pending-abandon-detail")} }
                div { class: "admin-shell__dialog-actions",
                    button { r#type: "button", class: "admin-btn-secondary", autofocus: true,
                        onclick: move |_| confirm_abandon.set(None), {t!("admin-pending-keep")}
                    }
                    button { r#type: "button", class: "admin-btn-danger",
                        onclick: {
                            let command_id = entry.identity.command_id;
                            move |_| {
                                let focus = focus_targets_after(host, command_id);
                                confirm_abandon.set(None);
                                if host.abandon(command_id) {
                                    focus_after_removal(focus);
                                }
                            }
                        },
                        {t!("admin-pending-abandon-confirm")}
                    }
                }
            }
        }
        CommandReceipts { host }
    }
}

#[component]
fn CommandReceipts(host: PendingCommandHost) -> Element {
    let mut open_key = use_signal(|| None::<(Uuid, u64)>);
    let visibility = generated_key_visibility();
    let notices = host.notices.read();
    let visible = notices.iter().filter(|notice| !host.key_in_source(notice));
    let active_key = open_key().and_then(|(command_id, revision)| {
        if visibility.hidden || visibility.revision != revision {
            return None;
        }
        let notice = visible.clone().find(|notice| notice.identity.command_id == command_id)?;
        match &notice.result {
            Ok(Some(CommandReceipt::GeneratedKey(key))) => {
                Some((command_id, revision, key.clone()))
            }
            _ => None,
        }
    });
    rsx! {
        div { class: "admin-command-notices__status", role: "status", aria_atomic: "false", aria_relevant: "additions text",
            for notice in visible.clone() {
                p { key: "{notice.identity.command_id}",
                    {receipt_message(&notice.identity, &notice.result)}
                    if let Ok(Some(CommandReceipt::Text(message))) = &notice.result {
                        " {message}"
                    }
                }
            }
        }
        if visible.clone().next().is_some() {
            section { class: "admin-panel admin-pending admin-command-notices", aria_label: t!("admin-pending-receipts"),
                ul { class: "admin-pending__list",
                    // 直接生成同级回执，让关闭前一项时后续回执保留原按钮及焦点。
                    {visible.map(|notice| {
                        let command_id = notice.identity.command_id;
                        rsx! {
                            li { class: "admin-command-notices__receipt", key: "{command_id}",
                                p { {receipt_message(&notice.identity, &notice.result)} }
                                if let Ok(Some(CommandReceipt::Text(message))) = &notice.result {
                                    p { class: "admin-panel__note", "{message}" }
                                }
                                if matches!(notice.result, Ok(Some(CommandReceipt::GeneratedKey(_)))) {
                                    button { id: record_button_id(command_id, "receipt"), r#type: "button", disabled: visibility.hidden,
                                        onclick: move |_| {
                                            let current = generated_key_visibility();
                                            if !current.hidden && current == visibility {
                                                open_key.set(Some((command_id, current.revision)));
                                            }
                                        },
                                        {t!("admin-pending-view-key")}
                                    }
                                    if visibility.hidden {
                                        p { class: "admin-panel__note", {t!("admin-session-key-hidden")} }
                                    }
                                } else {
                                    button { id: record_button_id(command_id, "receipt"), r#type: "button", onclick: move |_| close_receipt(host, command_id),
                                        {t!("admin-pending-close-receipt")}
                                    }
                                }
                            }
                        }
                    })}
                }
            }
        }
        if let Some(active) = active_key {
            for (command_id, revision, generated) in [active] {
                AdminUserGeneratedKeyDialog {
                    key: "{command_id}:{revision}",
                    generated,
                    on_close: move |_| {
                        // 迟到的关闭动作只可确认创建该对话框时的回执。
                        if generated_key_visibility() == visibility
                            && open_key.try_peek().is_ok_and(|opened| *opened == Some((command_id, revision)))
                        {
                            open_key.set(None);
                            close_receipt(host, command_id);
                        }
                    },
                }
            }
        }
    }
}

impl PendingCommandHost {
    fn key_in_source(self, notice: &super::CommandNotice) -> bool {
        notice.key_source.as_ref().is_some_and(|source| self.live_source(source).is_some())
    }
}

fn close_receipt(host: PendingCommandHost, command_id: Uuid) {
    let focus = focus_targets_after(host, command_id);
    if host.dismiss(command_id) {
        focus_after_removal(focus);
    }
}

fn record_button_id(command_id: Uuid, action: &str) -> String {
    format!("admin-command-{action}-{command_id}")
}

fn focus_targets_after(host: PendingCommandHost, removed: Uuid) -> Vec<String> {
    let writes_allowed = crate::api::admin_session_writes_allowed();
    let entries = host.entries.peek();
    let notices = host.notices.peek();
    let mut rows: Vec<_> = entries
        .iter()
        .filter(|entry| entry.unknown)
        .map(|entry| {
            let buttons = [
                (writes_allowed && !entry.retry_blocked(host)).then_some("retry"),
                (!entry.busy).then_some("abandon"),
            ];
            (entry.identity.command_id, buttons)
        })
        .chain(
            notices
                .iter()
                .filter(|notice| !host.key_in_source(notice))
                .map(|notice| (notice.identity.command_id, [Some("receipt"), None])),
        )
        .collect();
    if let Some(index) = rows.iter().position(|(command_id, _)| *command_id == removed) {
        let next = (index + 1) % rows.len();
        rows.rotate_left(next);
    }
    rows.into_iter()
        .filter(|(command_id, _)| *command_id != removed)
        .flat_map(|(command_id, buttons)| {
            buttons.into_iter().flatten().map(move |button| record_button_id(command_id, button))
        })
        .chain(std::iter::once("admin-content".to_owned()))
        .collect()
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[derive(Clone, Default)]
struct FocusTargets(std::rc::Rc<std::cell::RefCell<Vec<String>>>);

fn focus_after_removal(_targets: Vec<String>) {
    #[cfg(all(test, not(target_arch = "wasm32")))]
    if let Some(recorded) = try_consume_context::<FocusTargets>() {
        recorded.0.replace(_targets);
    }
    #[cfg(target_arch = "wasm32")]
    spawn(async move {
        use auction_web_shared::{a11y::focus_element_by_id, timer::TimeoutFuture};

        // 先让组件移除条目与弹窗，避免焦点陷阱把焦点送回已消失的打开按钮。
        TimeoutFuture::new(0).await;
        let Some(document) = web_sys::window().and_then(|window| window.document()) else { return };
        for target in &_targets {
            if document
                .get_element_by_id(target)
                .is_some_and(|element| element.matches(":disabled").is_ok_and(|disabled| !disabled))
            {
                focus_element_by_id(target);
                break;
            }
        }
    });
}

fn receipt_message(
    identity: &CommandIdentity,
    result: &Result<Option<CommandReceipt>, String>,
) -> String {
    let action = &identity.action;
    let entity = &identity.entity;
    match result {
        Ok(_) => t!("admin-pending-success-receipt", action: action, entity: entity),
        Err(code) => {
            t!("admin-pending-rejection-receipt", action: action, entity: entity, reason: admin_api_error_message(code))
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use std::future::ready;

    use dioxus::core::Mutation;
    use dioxus::prelude::*;
    use uuid::Uuid;

    use super::{CommandReceipts, FocusTargets, PendingCommandNotices, record_button_id};
    use crate::api::{AdminWriteEndpoint, AdminWriteFailure};
    use crate::surface::auctions::editor::tests::{button, click_button, mount_writer};
    use crate::surface::pending::{
        CommandDelivery, CommandIdentity, CommandNotice, PendingCommandHost, use_pending_commands,
    };
    use crate::surface::realtime::text_attributes;

    #[test]
    fn keyed_receipts_keep_the_remaining_button_when_the_first_is_closed() {
        let second = Uuid::from_u128(2);
        let (mut page, initial) = mount_writer(move || {
            let mut host = use_context::<PendingCommandHost>();
            use_hook(move || {
                host.notices.set(
                    [Uuid::from_u128(1), second]
                        .into_iter()
                        .map(|command_id| CommandNotice {
                            identity: CommandIdentity {
                                entity: command_id.to_string(),
                                action: "测试操作".to_owned(),
                                command_id,
                            },
                            result: Ok(None),
                            key_source: None,
                        })
                        .collect(),
                );
            });
            rsx! { CommandReceipts { host } }
        });
        let second_button_id = record_button_id(second, "receipt");
        let second_button = text_attributes(&initial, "id")
            .find_map(|(id, value)| (value == second_button_id).then_some(id))
            .expect("第二条回执按钮已挂载");
        let dismissed = click_button(&mut page, "关闭回执");
        assert_eq!(button(&page, "关闭回执").unwrap().0, second_button);
        assert!(
            !dismissed.iter().any(|edit| matches!(
                edit, Mutation::NewEventListener { name, id } if name == "click" && *id == second_button
            )),
            "保留第二条回执节点，不能复用元素编号重新挂载按钮"
        );
    }

    #[test]
    fn explicit_removal_moves_focus_to_the_next_record_then_back_to_main() {
        for unknown in [true, false] {
            let targets = FocusTargets::default();
            let (mut page, _) = mount_writer({
                let targets = targets.clone();
                move || {
                    use_context_provider(|| targets.clone());
                    let host = use_context::<PendingCommandHost>();
                    let reload = use_signal(|| 0);
                    let owner = use_pending_commands(reload, |_| true);
                    rsx! {
                        main { id: "admin-content", tabindex: "-1",
                            button { onclick: move |_| {
                                for entity in ["world:one", "world:two"] {
                                    assert!(owner.submit_with_receipt(
                                        entity.to_owned(), "测试操作".to_owned(), move |_| {
                                            ready((!unknown).ok_or_else(|| {
                                                AdminWriteFailure::from_test_http(
                                                    AdminWriteEndpoint::General,
                                                    503,
                                                    r#"{"code":"storage_unavailable","message":"暂不可用","data":null,"request_id":null}"#,
                                                )
                                            }))
                                        },
                                        |(), _| CommandDelivery::Receipt(None), |_| {},
                                    ));
                                }
                            }, {"发送两条命令".to_owned()} }
                            PendingCommandNotices { host }
                        }
                    }
                }
            });
            click_button(&mut page, "发送两条命令");
            assert!(targets.0.borrow().is_empty(), "新状态不能抢占焦点");
            let host = page.dom().in_scope(ScopeId::APP, consume_context::<PendingCommandHost>);
            let second = if unknown {
                host.entries.peek()[1].identity.command_id
            } else {
                host.notices.peek()[1].identity.command_id
            };
            let button_label = if unknown { "放弃" } else { "关闭回执" };
            click_button(&mut page, button_label);
            if unknown {
                click_button(&mut page, "保留未决操作");
                assert!(targets.0.borrow().is_empty(), "取消时沿用对话框的焦点归还");
                click_button(&mut page, button_label);
                click_button(&mut page, "确认放弃");
            }
            let next = record_button_id(second, if unknown { "retry" } else { "receipt" });
            assert_eq!(targets.0.borrow()[0], next);
            click_button(&mut page, button_label);
            if unknown {
                click_button(&mut page, "确认放弃");
            }
            assert_eq!(targets.0.borrow().as_slice(), ["admin-content"]);
            assert!(button(&page, button_label).is_none());
        }
    }
}
