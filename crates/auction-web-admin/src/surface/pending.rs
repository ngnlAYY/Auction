//! 当前 Admin 稳定 owner 内已提交命令的稀疏宿主；不保存未提交草稿或成功结果。
//! 工厂只拥有首次提交的完整参数与 UUID，重试不读取行、版本或表单。任务登记在 owner
//! 的 Scope，行或弹窗卸载不取消任务；owner 销毁同时释放工厂，敏感负载只驻留内存。

use std::{future::Future, pin::Pin, rc::Rc};

use auction_web_shared::{TruncatedText, t};
use dioxus::{core::Runtime, prelude::*};
use uuid::Uuid;

use super::admin_api_error_message;
use crate::api::AdminApiFailure;

type Attempt = Pin<Box<dyn Future<Output = Result<(), String>>>>;
type CommandFactory = Rc<dyn Fn(Uuid) -> Attempt>;

struct PendingCommand {
    entity: String,
    action: String,
    command_id: Uuid,
    busy: bool,
    unknown: bool,
    error: Option<String>,
    factory: Option<CommandFactory>,
}

impl PendingCommand {
    fn occupies_entity(&self) -> bool {
        self.busy || self.unknown
    }
}

/// 只在稳定 owner 安装，子组件只消费该句柄；一个实体同时最多保留一个已提交意图。
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct PendingCommands {
    entries: Signal<Vec<PendingCommand>>,
    reload: Signal<u64>,
    gate: Option<PendingCommandGate>,
}

/// 可选页级串行门禁，只保存状态；一次性结果由页面展示，不进入此处。
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct PendingCommandGate {
    state: Signal<CommandGateState>,
}

#[derive(Clone, Copy, PartialEq)]
enum CommandGateState {
    Ready,
    Running,
    AwaitingAcknowledgement,
}

/// 由稳定页面创建，与首次提交及未决命令重试共享。
pub(crate) fn use_pending_command_gate() -> PendingCommandGate {
    let state = use_signal(|| CommandGateState::Ready);
    PendingCommandGate { state }
}

impl PendingCommandGate {
    /// 供控件展示及入口守卫读取；实际执行仍须同步取得占用。
    pub(crate) fn blocked(self) -> bool {
        *self.state.read() != CommandGateState::Ready
    }

    /// 必须在 spawn 前同步检查并占用，不能用渲染或 effect 作为执行授权。
    pub(crate) fn try_acquire(mut self) -> Option<PendingCommandGateGuard> {
        let mut state = self.state.write();
        if *state != CommandGateState::Ready {
            return None;
        }
        *state = CommandGateState::Running;
        drop(state);
        Some(PendingCommandGateGuard(self))
    }

    /// 成功结果已交页面；直到操作员确认展示完成前继续封锁新请求。
    pub(crate) fn hold_result(mut self) {
        self.state.set(CommandGateState::AwaitingAcknowledgement);
    }

    /// 只确认已展示结果，不允许关闭动作误释放仍在运行的请求。
    pub(crate) fn acknowledge(mut self) {
        let mut state = self.state.write();
        if *state == CommandGateState::AwaitingAcknowledgement {
            *state = CommandGateState::Ready;
        }
    }
}

/// 请求结束或取消时释放运行占用，但不能解除页面待确认的一次性结果。
pub(crate) struct PendingCommandGateGuard(PendingCommandGate);

impl Drop for PendingCommandGateGuard {
    fn drop(&mut self) {
        if let Ok(mut state) = self.0.state.try_write()
            && *state == CommandGateState::Running
        {
            *state = CommandGateState::Ready;
        }
    }
}

/// 生命周期与安装 owner 一致；通常属于页面，配置草稿随认证外壳，无持久化或自动重放。
pub(crate) fn use_pending_commands(reload: Signal<u64>) -> PendingCommands {
    use_pending_commands_with_gate(reload, None)
}

/// 只有明确需要串行交付结果的页面配置门禁，其他页面维持原有并发策略。
pub(crate) fn use_pending_commands_with_gate(
    reload: Signal<u64>,
    gate: Option<PendingCommandGate>,
) -> PendingCommands {
    let owner = use_owned_pending_commands(reload, gate);
    use_context_provider(|| owner)
}

/// 只创建命令宿主，不覆盖同一 Scope 中其他业务的 PendingCommands context。
pub(crate) fn use_owned_pending_commands(
    reload: Signal<u64>,
    gate: Option<PendingCommandGate>,
) -> PendingCommands {
    let entries = use_signal(Vec::new);
    PendingCommands { entries, reload, gate }
}

impl PendingCommands {
    /// 同实体只有发送中或结果未知时阻止新意图。
    pub(crate) fn contains(self, entity: &str) -> bool {
        self.entries.read().iter().any(|entry| entry.entity == entity && entry.occupies_entity())
    }

    /// 区分正在发送与已停止但未决；浏览筛选可在请求结束后恢复，实体的新意图仍被封锁。
    pub(crate) fn is_busy(self, entity: &str) -> bool {
        self.entries.read().iter().any(|entry| entry.entity == entity && entry.busy)
    }

    /// 页面级原有 busy 守卫可据此聚合，不能被行卸载的 use_drop 提前释放。
    pub(crate) fn any_busy(self) -> bool {
        self.entries.read().iter().any(|entry| entry.busy)
    }

    /// 只登记已经确认的命令。成功结果直接交给回调，不进入未决集合；回调中的行 Signal
    /// 必须 try_write，一次性结果只交给稳定页面持有的状态。
    pub(crate) fn submit<T, F>(
        mut self,
        entity: String,
        action: String,
        operation: impl Fn(Uuid) -> F + 'static,
        on_success: impl Fn(T) + 'static,
        on_failure: impl Fn(String) + 'static,
    ) -> bool
    where
        T: 'static,
        F: Future<Output = Result<T, String>> + 'static,
    {
        if !crate::api::admin_session_writes_allowed()
            || self.contains(&entity)
            || self.gate.is_some_and(PendingCommandGate::blocked)
        {
            return false;
        }
        let command_id = Uuid::new_v4();
        let on_success = Rc::new(on_success);
        let on_failure = Rc::new(on_failure);
        let factory: CommandFactory = Rc::new(move |command_id| {
            let operation = operation(command_id);
            let on_success = on_success.clone();
            let on_failure = on_failure.clone();
            Box::pin(async move {
                match operation.await {
                    Ok(result) => {
                        on_success(result);
                        Ok(())
                    }
                    Err(code) => {
                        on_failure(AdminApiFailure::display_code(&code).to_owned());
                        Err(code)
                    }
                }
            })
        });
        let mut entries = self.entries.write();
        // 未决意图已被守卫挡住，同实体仅可能剩下上一条拒绝通知。
        entries.retain(|entry| entry.entity != entity);
        entries.push(PendingCommand {
            entity,
            action,
            command_id,
            busy: false,
            unknown: false,
            error: None,
            factory: Some(factory),
        });
        drop(entries);
        self.retry(command_id);
        true
    }

    fn retry(mut self, command_id: Uuid) {
        if !crate::api::admin_session_writes_allowed() {
            return;
        }
        let mut entries = self.entries.write();
        let Some(entry) = entries.iter_mut().find(|entry| entry.command_id == command_id) else {
            return;
        };
        if entry.busy {
            return;
        }
        let Some(factory) = entry.factory.clone() else {
            return;
        };
        let guard = if let Some(gate) = self.gate {
            let Some(guard) = gate.try_acquire() else {
                return;
            };
            Some(guard)
        } else {
            None
        };
        // busy 属于稀疏条目；逐次创建页级 Signal 会一直保留到离开页面，即使命令早已完成。
        entry.busy = true;
        drop(entries);
        // 任务归稳定的集合宿主，不随行卸载取消；页面销毁同时释放条目与请求。
        Runtime::current().in_scope(self.entries.origin_scope(), || {
            spawn(async move {
                // 工厂内先向页面交付结果，再释放占用；尚未 poll 就取消也会释放。
                let _guard = guard;
                match factory(command_id).await {
                    Ok(()) => {
                        let Ok(mut entries) = self.entries.try_write() else {
                            return;
                        };
                        entries.retain(|entry| entry.command_id != command_id);
                        drop(entries);
                        if let Ok(mut reload) = self.reload.try_write() {
                            *reload = reload.saturating_add(1);
                        }
                    }
                    Err(code) => {
                        let Ok(mut entries) = self.entries.try_write() else {
                            return;
                        };
                        if let Some(entry) =
                            entries.iter_mut().find(|entry| entry.command_id == command_id)
                        {
                            entry.busy = false;
                            // 只有查过同 ID 账本的拒绝能排除更早的尝试已提交。其余未知必须保留原 UUID 与负载。
                            entry.unknown = !definite_rejection(&code)
                                || (entry.unknown
                                    && !AdminApiFailure::rejection_after_replay_check(&code));
                            if !entry.unknown {
                                entry.factory = None;
                            }
                            entry.error = Some(code);
                        }
                    }
                }
            });
        });
    }

    fn dismiss(mut self, command_id: Uuid) {
        self.entries.write().retain(|entry| {
            entry.command_id != command_id || entry.occupies_entity() || entry.error.is_none()
        });
    }
}

/// 页面命令完成后只回写仍存活的行状态，旧 Signal 不会指向新挂载的实体。
pub(crate) fn set_if_alive<T: 'static>(mut signal: Signal<T>, value: T) {
    if let Ok(mut target) = signal.try_write() {
        *target = value;
    }
}

/// 只认能排除本次命令效果的拒绝码；不据此推断较早的尝试。
pub(crate) fn definite_rejection(code: &str) -> bool {
    AdminApiFailure::definite_rejection(code)
}

/// 常驻操作区只展示实体、动作、命令 ID 与结果类别，不展示冻结负载或成功结果。
#[component]
pub(crate) fn PendingCommandPanel(
    owner: PendingCommands,
    #[props(default)] session_scoped: bool,
) -> Element {
    let entries = owner.entries.read();
    let writes_blocked = !crate::api::admin_session_writes_allowed()
        || owner.gate.is_some_and(PendingCommandGate::blocked);
    // 命令 ID 在 rsx 外取出，按钮闭包只捕获可复制的 ID；循环体根节点才能直接是带 key 的条目。
    let rows = entries.iter().map(|entry| (entry.command_id, entry));
    rsx! {
        if !entries.is_empty() {
            section { class: "admin-panel admin-pending", aria_label: t!("admin-pending-title"),
                h2 { {t!("admin-pending-title")} }
                if entries.iter().any(PendingCommand::occupies_entity) {
                    p { class: "admin-panel__note",
                        if session_scoped { {t!("admin-pending-session-help")} } else { {t!("admin-pending-help")} }
                    }
                }
                ul { class: "admin-pending__list",
                    for (command_id, entry) in rows {
                        li { class: "admin-pending__entry", key: "{command_id}",
                            "data-command-id": "{command_id}",
                            div { class: "admin-pending__identity",
                                strong { TruncatedText { value: entry.action.clone() } }
                                TruncatedText { value: entry.entity.clone() }
                                code { TruncatedText { value: command_id.to_string() } }
                            }
                            div { class: "admin-pending__result",
                                p { role: "status",
                                    if entry.busy { {t!("admin-pending-busy")} }
                                    if entry.unknown { {t!("admin-pending-unknown")} }
                                    else if !entry.busy { {t!("admin-pending-rejected")} }
                                }
                                if let Some(code) = entry.error.as_deref() {
                                    p { class: "action-feedback action-feedback--error",
                                        {admin_api_error_message(code)}
                                    }
                                }
                            }
                            div { class: "admin-pending__actions",
                                if entry.unknown {
                                    button { r#type: "button",
                                        disabled: entry.busy || writes_blocked,
                                        onclick: move |_| owner.retry(command_id),
                                        {t!("admin-pending-retry")}
                                    }
                                } else if !entry.busy {
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

#[cfg(test)]
mod tests {
    use std::{
        cell::Cell,
        cell::RefCell,
        future::{Future, ready},
        rc::Rc,
    };

    use dioxus::prelude::*;
    use gloo_net::http::Method;
    use uuid::Uuid;

    use super::{PendingCommandPanel, PendingCommands, definite_rejection, use_pending_commands};
    use crate::api::{
        AdminApiFailure, AdminDocumentKind, AdminSessionWritePermission, AdminUser,
        mutate_admin_user, publish_admin_rules,
    };
    use crate::surface::auctions::draft_page_tests::{has_live_text, mount_writer};

    #[derive(Default)]
    struct CommandHarness {
        pending: Cell<Option<PendingCommands>>,
    }

    fn test_page() -> Element {
        let control = consume_context::<Rc<CommandHarness>>();
        let permission = use_memo(|| true);
        use_context_provider(|| AdminSessionWritePermission(permission));
        let reload = use_signal(|| 0);
        control.pending.set(Some(use_pending_commands(reload)));
        rsx! { div {} }
    }

    #[test]
    fn rejected_panel_entries_render_their_specific_error_messages() {
        let (page, _) = mount_writer(|| {
            let reload = use_signal(|| 0);
            let owner = use_pending_commands(reload);
            use_hook(move || {
                for (status, code) in [(409, "alert_stale"), (413, "body_too_large")] {
                    assert!(owner.submit(
                        code.to_owned(),
                        "测试写入".to_owned(),
                        move |_| {
                            let body = serde_json::json!({
                                "code": code, "message": "写入已拒绝", "data": null,
                                "request_id": null,
                            });
                            ready(Err::<(), _>(
                                AdminApiFailure::from_http_body(status, &body.to_string()).code,
                            ))
                        },
                        |()| panic!("拒绝不能交付成功回执"),
                        |_| {},
                    ));
                }
            });
            rsx! { PendingCommandPanel { owner } }
        });
        assert!(has_live_text(&page, "关闭已拒绝操作"));
        assert!(has_live_text(&page, "告警状态已变化。请核对重新读取后的列表，再决定是否处置。"));
        assert!(has_live_text(&page, "请求体过大。请缩短内容后重试。"));
        assert!(!has_live_text(&page, "原样重试"));
    }

    #[test]
    fn discarded_auxiliary_actions_and_ledger_conflicts_are_definite_rejections() {
        for code in [
            "command_conflict",
            "not_found",
            "stale_credential_state",
            "credential_conflict",
            "credential_login_rejected",
            "credential_login_challenge",
            "credential_login_busy",
            "invalid_auction_intake",
            "unsupported_equipment_source",
            "auction_intake_unavailable",
            "seller_blacklisted",
            "equipment_conflict",
        ] {
            assert!(definite_rejection(code), "{code}");
        }
    }

    #[test]
    fn errors_that_can_follow_a_committed_command_are_kept_pending() {
        for code in [
            "state_conflict",
            "storage_unavailable",
            "reconciliation_required",
            "outcome_unknown",
            "internal_error",
            "invalid_credentials",
        ] {
            assert!(!definite_rejection(code), "{code}");
        }
    }

    #[test]
    fn clean_rejections_include_rolled_back_transactions_and_transient_guards() {
        for code in [
            "invalid_json",
            "invalid_replay",
            "rate_limited",
            "invalid_runtime_config",
            "invalid_runtime_switch",
            "stale_recovery_state",
            "recovery_send_unattributed",
            "recovery_not_confirmable",
            "recovery_evidence_unresolved",
            "stale_auction_state",
            "stale_lot_state",
            "stale_delivery_state",
            "sent_proof_rejected",
            "world_has_active_auction",
            "world_not_found",
            "delivery_not_found",
            "not_acknowledgeable",
            "rename_name_taken",
            "user_conflict",
            "alert_stale",
            "invalid_alert_command",
            "invalid_delivery_command",
            "delivery_sent_sighted",
            "settlement_obligation_ineligible",
            "invalid_auction_state",
            "invalid_lot_state",
            "manual_early_settlement_disabled",
            "invalid_credential_request",
            "invalid_credential_secret",
            "external_access_paused",
            "invalid_mail_request",
            "poll_rejected",
            "conflict",
        ] {
            assert!(definite_rejection(code), "{code}");
        }
    }

    #[test]
    fn pre_write_validation_is_a_definite_rejection() {
        for code in [
            "invalid_request_id",
            "invalid_query",
            "unsupported_media_type",
            "body_too_large",
            "method_not_allowed",
            "origin_forbidden",
            "csrf_invalid",
            "invalid_command_id",
            "invalid_auction_id",
            "invalid_auction_command",
            "invalid_auction_schedule",
            "invalid_lot_id",
            "invalid_lot_action",
            "invalid_lot_command",
            "invalid_recovery_command",
            "invalid_world",
            "invalid_world_command",
            "invalid_delivery_id",
            "invalid_user_id",
            "invalid_rename_id",
            "invalid_rename_command",
            "invalid_credential_action",
            "invalid_credential_kind",
            "invalid_credential_label",
            "invalid_credential_player_name",
            "invalid_credential_status",
            "invalid_credential_world",
            "invalid_mail_template",
            "mail_template_not_found",
            "mail_template_conflict",
            "runtime_switch_conflict",
            "already_disposed",
        ] {
            assert!(definite_rejection(code), "{code}");
        }
    }

    #[test]
    fn an_unreadable_error_response_does_not_prove_rejection() {
        for code in ["invalid_response", "http_400", "http_413", "http_503", "new_error"] {
            assert!(!definite_rejection(code), "{code}");
        }
    }

    #[test]
    fn rejections_without_a_replay_check_cannot_clear_an_earlier_unknown_attempt() {
        let control = Rc::new(CommandHarness::default());
        let mut dom = VirtualDom::new(test_page).with_root_context(control.clone());
        dom.rebuild_in_place();
        let commands = control.pending.get().unwrap();

        let command_ids = Rc::new(RefCell::new(Vec::new()));
        dom.in_scope(ScopeId::APP, || {
            let seen_ids = command_ids.clone();
            assert!(commands.submit(
                "first".into(),
                "test".into(),
                move |command_id| {
                    let mut seen = seen_ids.borrow_mut();
                    seen.push(command_id);
                    ready(match seen.len() {
                        1 => Err("request_timeout".to_owned()),
                        2 => Err(AdminApiFailure::not_sent_code("alert_stale")),
                        3 => Err("rate_limited".to_owned()),
                        4 => Err("unauthenticated".to_owned()),
                        5 => Err("rename_name_taken".to_owned()),
                        _ => Ok(()),
                    })
                },
                |()| {},
                |_| {}
            ));
        });
        dom.process_events();
        let command_id = command_ids.borrow()[0];
        assert!(commands.entries.peek()[0].unknown);

        for _ in 0..4 {
            dom.in_scope(ScopeId::APP, || commands.retry(command_id));
            dom.process_events();
            assert!(commands.entries.peek()[0].unknown);
            dom.in_scope(ScopeId::APP, || {
                commands.dismiss(command_id);
                assert!(!commands.submit(
                    "first".into(),
                    "new".into(),
                    |_| ready(Ok(())),
                    |()| {},
                    |_| {},
                ));
            });
            assert!(commands.contains("first"));
        }

        dom.in_scope(ScopeId::APP, || commands.retry(command_id));
        dom.process_events();
        assert!(!commands.contains("first"));
        assert_eq!(command_ids.borrow().as_slice(), &[command_id; 6]);
    }

    #[test]
    fn a_replay_checked_alert_rejection_clears_an_earlier_unknown_attempt() {
        let control = Rc::new(CommandHarness::default());
        let mut dom = VirtualDom::new(test_page).with_root_context(control.clone());
        dom.rebuild_in_place();
        let commands = control.pending.get().unwrap();

        let command_ids = Rc::new(RefCell::new(Vec::new()));
        dom.in_scope(ScopeId::APP, || {
            let seen_ids = command_ids.clone();
            assert!(commands.submit(
                "alert".into(),
                "silence".into(),
                move |command_id| {
                    let mut seen = seen_ids.borrow_mut();
                    seen.push(command_id);
                    ready(Err::<(), _>(
                        if seen.len() == 1 { "request_timeout" } else { "alert_stale" }.to_owned(),
                    ))
                },
                |()| {},
                |_| {},
            ));
        });
        dom.process_events();
        let command_id = command_ids.borrow()[0];
        assert!(commands.entries.peek()[0].unknown);

        dom.in_scope(ScopeId::APP, || commands.retry(command_id));
        dom.process_events();
        assert_eq!(command_ids.borrow().as_slice(), &[command_id; 2]);
        assert_eq!(commands.entries.peek()[0].error.as_deref(), Some("alert_stale"));
        assert!(!commands.entries.peek()[0].unknown);
        assert!(!commands.contains("alert"));

        dom.in_scope(ScopeId::APP, || {
            commands.dismiss(command_id);
            assert!(commands.entries.peek().is_empty());
            assert!(commands.submit(
                "alert".into(),
                "new".into(),
                |_| ready(Ok(())),
                |()| {},
                |_| {}
            ));
        });
        dom.process_events();
        assert!(commands.entries.peek().is_empty());
    }

    #[test]
    fn a_new_command_replaces_the_rejected_notice_for_its_entity() {
        let control = Rc::new(CommandHarness::default());
        let mut dom = VirtualDom::new(test_page).with_root_context(control.clone());
        dom.rebuild_in_place();
        let commands = control.pending.get().unwrap();
        let failures = Rc::new(RefCell::new(Vec::new()));
        dom.in_scope(ScopeId::APP, || {
            let observed = failures.clone();
            assert!(commands.submit(
                "first".into(),
                "old".into(),
                |_| ready(Err::<(), _>("rate_limited".into())),
                |()| {},
                move |code| observed.borrow_mut().push(code),
            ));
            assert!(commands.contains("first"));
            assert!(commands.is_busy("first"));
            assert!(!commands.submit(
                "first".into(),
                "new".into(),
                |_| ready(Ok(())),
                |()| {},
                |_| {},
            ));
            assert!(commands.submit(
                "other".into(),
                "other".into(),
                |_| ready(Err::<(), _>("invalid_json".into())),
                |()| {},
                |_| {},
            ));
        });
        dom.process_events();
        let rejected_id = commands.entries.peek()[0].command_id;
        assert_eq!(failures.borrow().as_slice(), &["rate_limited"]);
        assert!(!commands.contains("first"));
        assert_eq!(commands.entries.peek().len(), 2);

        dom.in_scope(ScopeId::APP, || {
            commands.retry(rejected_id);
            assert!(commands.submit(
                "first".into(),
                "new".into(),
                |_| ready(Err::<(), _>("invalid_json".into())),
                |()| {},
                |_| {},
            ));
        });
        dom.process_events();
        let entries = commands.entries.peek();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].entity, "other");
        assert_eq!(entries[1].entity, "first");
        assert_ne!(entries[1].command_id, rejected_id);
        assert_eq!(entries[1].error.as_deref(), Some("invalid_json"));
        assert_eq!(failures.borrow().as_slice(), &["rate_limited"]);
    }

    #[test]
    fn a_first_definite_failure_can_be_dismissed() {
        for code in [AdminApiFailure::not_sent_code("network_error"), "stale_lot_state".to_owned()]
        {
            let control = Rc::new(CommandHarness::default());
            let mut dom = VirtualDom::new(test_page).with_root_context(control.clone());
            dom.rebuild_in_place();
            let commands = control.pending.get().unwrap();
            dom.in_scope(ScopeId::APP, || {
                assert!(commands.submit(
                    "first".into(),
                    "test".into(),
                    move |_| ready(Err::<(), _>(code.clone())),
                    |()| {},
                    |_| {}
                ));
            });
            dom.process_events();
            let command_id = commands.entries.peek()[0].command_id;
            assert!(!commands.entries.peek()[0].unknown);
            dom.in_scope(ScopeId::APP, || commands.dismiss(command_id));
            assert!(commands.entries.peek().is_empty());
        }
    }

    #[test]
    fn an_unsent_error_reaches_the_initiator_as_a_display_code() {
        let control = Rc::new(CommandHarness::default());
        let mut dom = VirtualDom::new(test_page).with_root_context(control.clone());
        dom.rebuild_in_place();
        let commands = control.pending.get().unwrap();
        let displayed = Rc::new(RefCell::new(None));
        dom.in_scope(ScopeId::APP, || {
            let displayed = displayed.clone();
            assert!(commands.submit(
                "first".into(),
                "test".into(),
                |_| ready(Err::<(), _>(AdminApiFailure::not_sent_code("network_error"))),
                |()| {},
                move |code| *displayed.borrow_mut() = Some(code),
            ));
        });
        dom.process_events();
        assert_eq!(displayed.borrow().as_deref(), Some("network_error"));
        assert_eq!(commands.entries.peek()[0].error.as_deref(), Some("not_sent:network_error"));
        assert!(!commands.contains("first"));
    }

    fn assert_first_version_conflict_releases_entity<F>(
        url: &'static str,
        method: Method,
        body: &'static str,
        send: impl Fn(Uuid) -> F + 'static,
    ) where
        F: Future<Output = Result<(), String>> + 'static,
    {
        let control = Rc::new(CommandHarness::default());
        let mut dom = VirtualDom::new(test_page).with_root_context(control.clone());
        dom.rebuild_in_place();
        let commands = control.pending.get().unwrap();
        let displayed = Rc::new(RefCell::new(None));
        dom.in_scope(ScopeId::APP, || {
            AdminApiFailure::stub_write_response(url, method, 409, body);
            let displayed = displayed.clone();
            assert!(commands.submit(
                "first".into(),
                "test".into(),
                send,
                |()| panic!("版本冲突不能交付成功结果"),
                move |code| *displayed.borrow_mut() = Some(code),
            ));
            assert!(commands.contains("first"));
        });
        dom.process_events();
        assert_eq!(displayed.borrow().as_deref(), Some("state_conflict"));
        assert!(!commands.contains("first"));
    }

    #[test]
    fn a_first_user_api_version_conflict_leaves_the_entity_available() {
        let user = AdminUser {
            id: Uuid::new_v4().to_string(),
            username: "玩家甲".to_owned(),
            forum_account_id: None,
            status: "active".to_owned(),
            key_status: "active".to_owned(),
            state_version: 5,
            created_at: "2026-11-01T00:00:00Z".to_owned(),
            updated_at: "2026-11-01T00:00:00Z".to_owned(),
            blacklist_count: 0,
            blacklist_history: Vec::new(),
        };
        assert_first_version_conflict_releases_entity(
            "/api/v1/admin/users",
            Method::PATCH,
            r#"{"code":"state_conflict","message":"用户版本已变化","data":{"current_state_version":6},"request_id":null}"#,
            move |command_id| {
                let user = user.clone();
                async move {
                    mutate_admin_user(command_id, &user, "rename", Some("玩家乙".to_owned()), None)
                        .await
                        .map(|_| ())
                }
            },
        );
    }

    #[test]
    fn first_rules_and_about_api_version_conflicts_leave_the_entity_available() {
        for (kind, url) in [
            (AdminDocumentKind::Rules, "/api/v1/admin/rules"),
            (AdminDocumentKind::About, "/api/v1/admin/about"),
        ] {
            assert_first_version_conflict_releases_entity(
                url,
                Method::PUT,
                r#"{"code":"state_conflict","message":"文档版本已变化","data":{"current_revision":6},"request_id":null}"#,
                move |command_id| async move {
                    publish_admin_rules(
                        kind,
                        command_id,
                        5,
                        "zh-CN".to_owned(),
                        "标题".to_owned(),
                        "正文".to_owned(),
                    )
                    .await
                    .map(|_| ())
                    .map_err(|failure| failure.code)
                },
            );
        }
    }
}
