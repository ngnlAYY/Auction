//! 认证会话内持有已提交命令，按实体限制重复意图并通知当前显示该实体的读取视图。

use std::{
    future::Future,
    pin::Pin,
    rc::{Rc, Weak},
};

use dioxus::{core::Runtime, prelude::*};
use uuid::Uuid;

use self::sources::CommandSource;
use super::users::AdminUserGeneratedKey;
use crate::api::AdminApiFailure;

mod panel;
mod sources;

pub(crate) use panel::{PendingCommandNotices, PendingCommandPanel};
pub(super) use sources::advance_command_reload;
pub(crate) use sources::{use_pending_commands, use_pending_commands_with_gate};

type Deliver = Box<dyn FnOnce(bool) -> CommandDelivery>;
type Attempt = Pin<Box<dyn Future<Output = Result<Deliver, String>>>>;
type CommandFactory = Rc<dyn Fn(Uuid) -> Attempt>;

#[derive(Clone)]
struct CommandIdentity {
    entity: String,
    action: String,
    command_id: Uuid,
}

struct PendingCommand {
    identity: CommandIdentity,
    source: Weak<CommandSource>,
    busy: bool,
    unknown: bool,
    error: Option<String>,
    factory: CommandFactory,
    on_failure: Rc<dyn Fn(String)>,
}

struct CommandNotice {
    identity: CommandIdentity,
    result: Result<Option<CommandReceipt>, String>,
    key_source: Option<Weak<CommandSource>>,
}

/// 来源页卸载后仍需交付的安全回执信息；Key 只能由操作员打开一次性对话框查看。
pub(super) enum CommandReceipt {
    Text(String),
    GeneratedKey(AdminUserGeneratedKey),
}

/// 成功回调确认页面是否实际显示结果。仍需交付的内容由会话持有。
pub(super) enum CommandDelivery {
    Delivered,
    Receipt(Option<CommandReceipt>),
}

impl CommandDelivery {
    pub(super) fn reported(
        delivered: bool,
        receipt: impl FnOnce() -> Option<CommandReceipt>,
    ) -> Self {
        if delivered { Self::Delivered } else { Self::Receipt(receipt()) }
    }
}

/// 唯一的会话宿主；条目与回执只驻留在当前认证外壳。
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct PendingCommandHost {
    entries: Signal<Vec<PendingCommand>>,
    notices: Signal<Vec<CommandNotice>>,
    sources: Signal<Vec<Weak<CommandSource>>>,
}

/// 供来源页与它的子组件查询会话实体占用，并标记回执的交付位置。
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct PendingCommands {
    host: PendingCommandHost,
    source: Signal<Rc<CommandSource>>,
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

/// 由认证外壳安装一次；每个请求都由这个 Scope 持有。
pub(crate) fn use_pending_command_host() -> PendingCommandHost {
    let entries = use_signal(Vec::new);
    let notices = use_signal(Vec::new);
    let sources = use_signal(Vec::new);
    use_context_provider(|| PendingCommandHost { entries, notices, sources })
}

impl PendingCommands {
    /// 同实体的发送中或未知记录跨页面阻止新意图。
    pub(crate) fn contains(self, entity: &str) -> bool {
        self.host.entries.read().iter().any(|entry| entry.identity.entity == entity)
    }

    /// 区分正在发送与已停止但未决；浏览筛选可在请求结束后恢复，实体的新意图仍被封锁。
    pub(crate) fn is_busy(self, entity: &str) -> bool {
        self.host.entries.read().iter().any(|entry| entry.identity.entity == entity && entry.busy)
    }

    /// 保留本页面的串行操作策略，其他来源的请求只占用自己的实体。
    pub(crate) fn any_busy(self) -> bool {
        let source = self.source.read();
        self.host.entries.read().iter().any(|entry| entry.busy && entry.belongs_to(&source))
    }

    /// 本来源页的同类操作仍可共用同一个页内串行门禁。
    pub(crate) fn contains_local(self, prefix: &str) -> bool {
        let source = self.source.read();
        self.host
            .entries
            .read()
            .iter()
            .any(|entry| entry.identity.entity.starts_with(prefix) && entry.belongs_to(&source))
    }

    /// 只登记已经确认的命令。此入口的成功反馈由来源页面自身持有。
    pub(crate) fn submit<T, F>(
        self,
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
        self.submit_with_receipt(
            entity,
            action,
            operation,
            move |result, source_alive| {
                if source_alive {
                    on_success(result);
                    CommandDelivery::Delivered
                } else {
                    CommandDelivery::Receipt(None)
                }
            },
            on_failure,
        )
    }

    /// 投影成功回执。仅在 `source_alive` 时访问来源页面，临时行或卡片仍须检查自己的存活。
    pub(super) fn submit_with_receipt<T, F>(
        mut self,
        entity: String,
        action: String,
        operation: impl Fn(Uuid) -> F + 'static,
        on_success: impl Fn(T, bool) -> CommandDelivery + 'static,
        on_failure: impl Fn(String) + 'static,
    ) -> bool
    where
        T: 'static,
        F: Future<Output = Result<T, String>> + 'static,
    {
        let Ok(source) = self.source.try_peek() else { return false };
        if !crate::api::admin_session_writes_allowed()
            || self.contains(&entity)
            || source.gate.is_some_and(PendingCommandGate::blocked)
        {
            return false;
        }
        let command_id = Uuid::new_v4();
        let on_success = Rc::new(on_success);
        // 重试只使用首次确认的工厂和 UUID，不再读取当前草稿或版本。
        let factory: CommandFactory = Rc::new(move |command_id| {
            let operation = operation(command_id);
            let on_success = Rc::clone(&on_success);
            Box::pin(async move {
                let result = operation.await?;
                let deliver: Deliver =
                    Box::new(move |source_alive| on_success(result, source_alive));
                Ok(deliver)
            })
        });
        let mut rejections = source.rejections;
        rejections.write().retain(|notice| notice.identity.entity != entity);
        self.host.entries.write().push(PendingCommand {
            identity: CommandIdentity { entity, action, command_id },
            source: Rc::downgrade(&source),
            busy: false,
            unknown: false,
            error: None,
            factory,
            on_failure: Rc::new(on_failure),
        });
        self.host.retry(command_id);
        true
    }

    fn dismiss(self, command_id: Uuid) {
        let mut rejections = self.source.peek().rejections;
        rejections.write().retain(|notice| notice.identity.command_id != command_id);
    }

    /// 一次性 Key 由当前页面展示，回执由会话持有，直到操作员确认。
    pub(super) fn generated_key(self) -> Option<(Uuid, AdminUserGeneratedKey)> {
        let source = self.source.read();
        self.host.notices.read().iter().find_map(|notice| {
            let presented_here = notice
                .key_source
                .as_ref()
                .is_some_and(|origin| origin.as_ptr() == Rc::as_ptr(&source));
            if presented_here && let Ok(Some(CommandReceipt::GeneratedKey(key))) = &notice.result {
                Some((notice.identity.command_id, key.clone()))
            } else {
                None
            }
        })
    }

    pub(super) fn acknowledge_key(self, command_id: Uuid) {
        self.host.dismiss(command_id);
    }
}

impl PendingCommand {
    fn belongs_to(&self, source: &Rc<CommandSource>) -> bool {
        self.source.as_ptr() == Rc::as_ptr(source)
    }

    fn retry_blocked(&self, host: PendingCommandHost) -> bool {
        self.busy
            || host
                .live_source(&self.source)
                .and_then(|source| source.gate)
                .is_some_and(PendingCommandGate::blocked)
    }
}

impl PendingCommandHost {
    pub(super) fn retry(self, command_id: Uuid) {
        if !crate::api::admin_session_writes_allowed() {
            return;
        }
        let mut entries = self.entries;
        let mut entries = entries.write();
        let Some(entry) = entries.iter_mut().find(|entry| entry.identity.command_id == command_id)
        else {
            return;
        };
        if entry.busy {
            return;
        }
        let guard =
            if let Some(gate) = self.live_source(&entry.source).and_then(|source| source.gate) {
                let Some(guard) = gate.try_acquire() else {
                    return;
                };
                Some(guard)
            } else {
                None
            };
        entry.busy = true;
        let factory = Rc::clone(&entry.factory);
        drop(entries);
        // 站内导航不取消已发出的命令；认证子树销毁时同时释放请求、冻结负载和回执。
        Runtime::current().in_scope(self.entries.origin_scope(), || {
            spawn(async move {
                let _guard = guard;
                let result = factory(command_id).await;
                self.complete(command_id, result);
            });
        });
    }

    fn complete(mut self, command_id: Uuid, result: Result<Deliver, String>) {
        let mut entries = self.entries;
        let mut entries = entries.write();
        let Some(index) = entries.iter().position(|entry| entry.identity.command_id == command_id)
        else {
            return;
        };
        let entry = &mut entries[index];
        // 只有经本 ID 的账本核对的拒绝才能清除较早的未知。其余未知保留原 UUID 与负载。
        if let Err(code) = &result
            && (!definite_rejection(code)
                || (entry.unknown && !AdminApiFailure::rejection_after_replay_check(code)))
        {
            entry.busy = false;
            entry.unknown = true;
            let display = AdminApiFailure::display_code(code).to_owned();
            entry.error = Some(display.clone());
            let source = self.live_source(&entry.source);
            let on_failure = Rc::clone(&entry.on_failure);
            drop(entries);
            if source.is_some() {
                on_failure(display);
            }
            return;
        }
        let entry = entries.remove(index);
        drop(entries);
        let source = self.live_source(&entry.source);
        let reloads = self.reloads_for(&entry.identity.entity);
        match result {
            Ok(deliver) => {
                if let CommandDelivery::Receipt(receipt) = deliver(source.is_some()) {
                    // 已展示的 Key 随会话保留到确认。来源页卸载只解除本页门禁，外壳接管交付。
                    let key_source = if matches!(receipt, Some(CommandReceipt::GeneratedKey(_)))
                        && let Some(gate) = source.as_ref().and_then(|source| source.gate)
                    {
                        gate.hold_result();
                        Some(entry.source)
                    } else {
                        None
                    };
                    self.notices.write().push(CommandNotice {
                        identity: entry.identity,
                        result: Ok(receipt),
                        key_source,
                    });
                }
            }
            Err(code) => {
                let code = AdminApiFailure::display_code(&code).to_owned();
                let mut notices = if let Some(source) = source {
                    (entry.on_failure)(code.clone());
                    source.rejections
                } else {
                    self.notices
                };
                notices.write().push(CommandNotice {
                    identity: entry.identity,
                    result: Err(code),
                    key_source: None,
                });
            }
        }
        // 同一轮已有回调推进的世代无需再次推进。新挂载且正在显示同一实体的视图也收到失效通知。
        for (reload, previous) in reloads {
            if reload.try_peek().is_ok_and(|generation| *generation == previous) {
                advance_command_reload(reload);
            }
        }
    }

    fn abandon(mut self, command_id: Uuid) -> bool {
        if !crate::api::admin_session_writes_allowed() {
            return false;
        }
        // 放弃仅移除本地未知记录，已发出的命令仍可能生效。
        let mut entries = self.entries.write();
        let Some(index) = entries.iter().position(|entry| {
            entry.identity.command_id == command_id && !entry.busy && entry.unknown
        }) else {
            return false;
        };
        entries.remove(index);
        true
    }

    fn dismiss(mut self, command_id: Uuid) -> bool {
        let mut notices = self.notices.write();
        let Some(index) =
            notices.iter().position(|notice| notice.identity.command_id == command_id)
        else {
            return false;
        };
        let notice = notices.remove(index);
        drop(notices);
        if let Some(gate) = notice
            .key_source
            .as_ref()
            .and_then(|source| self.live_source(source))
            .and_then(|source| source.gate)
        {
            gate.acknowledge();
        }
        true
    }
}

/// 页面命令完成后只回写仍存活的行状态，旧 Signal 不会指向新挂载的实体。
pub(crate) fn set_if_alive<T: 'static>(signal: Signal<T>, value: T) {
    try_set_if_alive(signal, value);
}

/// 反馈控件提前卸载时，调用方用返回值决定是否由外壳补交回执。
pub(super) fn try_set_if_alive<T: 'static>(mut signal: Signal<T>, value: T) -> bool {
    if let Ok(mut target) = signal.try_write() {
        *target = value;
        true
    } else {
        false
    }
}

/// 只认能排除本次命令效果的拒绝码；不据此推断较早的尝试。
pub(crate) fn definite_rejection(code: &str) -> bool {
    AdminApiFailure::definite_rejection(code)
}

#[cfg(test)]
mod tests {
    use std::{
        cell::Cell,
        cell::RefCell,
        collections::VecDeque,
        future::{Future, ready},
        rc::Rc,
    };

    use dioxus::prelude::*;
    use gloo_net::http::Method;
    use uuid::Uuid;

    use super::{
        CommandNotice, PendingCommandHost, PendingCommandNotices, PendingCommandPanel,
        PendingCommands, definite_rejection, use_pending_command_gate, use_pending_command_host,
        use_pending_commands, use_pending_commands_with_gate,
    };
    use crate::api::{
        AdminApiFailure, AdminDocumentKind, AdminSessionWritePermission, AdminUser,
        mutate_admin_user, publish_admin_rules,
    };
    use crate::surface::auctions::draft_page_tests::{
        RecordRead, button, click_button, has_live_text, has_text, mount_writer, settle_writer,
    };
    use crate::surface::realtime::TestPage;
    use crate::surface::users::{AdminUserGeneratedKey, AdminUserGeneratedKeyDialog};

    #[derive(Default)]
    struct CommandHarness {
        pending: Cell<Option<PendingCommands>>,
    }

    fn test_page() -> Element {
        let control = consume_context::<Rc<CommandHarness>>();
        let permission = use_memo(|| true);
        use_context_provider(|| AdminSessionWritePermission(permission));
        use_pending_command_host();
        let reload = use_signal(|| 0);
        control.pending.set(Some(use_pending_commands(reload, |_| true)));
        rsx! { div {} }
    }

    fn rejections(commands: PendingCommands) -> Signal<Vec<CommandNotice>> {
        commands.source.peek().rejections
    }

    #[test]
    fn rejected_panel_entries_render_their_specific_error_messages() {
        let (page, _) = mount_writer(|| {
            let reload = use_signal(|| 0);
            let owner = use_pending_commands(reload, |_| true);
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
        assert!(commands.host.entries.peek()[0].unknown);

        for _ in 0..4 {
            dom.in_scope(ScopeId::APP, || commands.host.retry(command_id));
            dom.process_events();
            assert!(commands.host.entries.peek()[0].unknown);
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

        dom.in_scope(ScopeId::APP, || commands.host.retry(command_id));
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
        assert!(commands.host.entries.peek()[0].unknown);

        dom.in_scope(ScopeId::APP, || commands.host.retry(command_id));
        dom.process_events();
        assert_eq!(command_ids.borrow().as_slice(), &[command_id; 2]);
        assert!(commands.host.entries.peek().is_empty());
        assert_eq!(rejections(commands).peek()[0].result.as_ref().err().unwrap(), "alert_stale");
        assert!(!commands.contains("alert"));

        dom.in_scope(ScopeId::APP, || {
            commands.dismiss(command_id);
            assert!(commands.host.entries.peek().is_empty());
            assert!(commands.submit(
                "alert".into(),
                "new".into(),
                |_| ready(Ok(())),
                |()| {},
                |_| {}
            ));
        });
        dom.process_events();
        assert!(commands.host.entries.peek().is_empty());
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
        let rejected_id = rejections(commands).peek()[0].identity.command_id;
        assert_eq!(failures.borrow().as_slice(), &["rate_limited"]);
        assert!(!commands.contains("first"));
        assert!(commands.host.entries.peek().is_empty());
        assert_eq!(rejections(commands).peek().len(), 2);

        dom.in_scope(ScopeId::APP, || {
            commands.host.retry(rejected_id);
            assert!(commands.submit(
                "first".into(),
                "new".into(),
                |_| ready(Err::<(), _>("invalid_json".into())),
                |()| {},
                |_| {},
            ));
        });
        dom.process_events();
        assert!(commands.host.entries.peek().is_empty());
        let notices = rejections(commands);
        let notices = notices.peek();
        assert_eq!(notices.len(), 2);
        assert_eq!(notices[0].identity.entity, "other");
        assert_eq!(notices[1].identity.entity, "first");
        assert_ne!(notices[1].identity.command_id, rejected_id);
        assert_eq!(notices[1].result.as_ref().err().unwrap(), "invalid_json");
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
            assert!(commands.host.entries.peek().is_empty());
            let command_id = rejections(commands).peek()[0].identity.command_id;
            dom.in_scope(ScopeId::APP, || commands.dismiss(command_id));
            assert!(rejections(commands).peek().is_empty());
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
        assert_eq!(rejections(commands).peek()[0].result.as_ref().err().unwrap(), "network_error");
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

    const SOURCE_ENTITY: &str = "alert:fixture";
    const SUBMIT: &str = "发送测试命令";
    const NAVIGATE: &str = "切换测试路由";

    struct SessionRequests {
        writes: RefCell<Vec<String>>,
        replies: RefCell<VecDeque<Result<(), String>>>,
        delayed: Option<RecordRead<()>>,
        source: Cell<Option<PendingCommands>>,
        delivered: Cell<usize>,
    }

    impl SessionRequests {
        async fn write(&self, command_id: Uuid) -> Result<(), String> {
            let first = self.writes.borrow().is_empty();
            self.writes.borrow_mut().push(
                serde_json::json!({"command_id": command_id, "reason": "第一次提交的原因"})
                    .to_string(),
            );
            if first && let Some(reply) = &self.delayed {
                return reply.fetch().await;
            }
            self.replies.borrow_mut().pop_front().expect("发送应有对应的测试回执")
        }
    }

    fn failure(status: u16, code: &str) -> String {
        let body = serde_json::json!({"code": code, "message": "请求失败", "data": null, "request_id": null});
        AdminApiFailure::from_http_body(status, &body.to_string()).code
    }

    fn mount_session_requests(
        delayed: bool,
        replies: impl IntoIterator<Item = Result<(), String>>,
        gated_destination: bool,
    ) -> (TestPage, Rc<SessionRequests>) {
        let requests = Rc::new(SessionRequests {
            writes: RefCell::new(Vec::new()),
            replies: RefCell::new(replies.into_iter().collect()),
            delayed: delayed.then(RecordRead::new),
            source: Cell::new(None),
            delivered: Cell::new(0),
        });
        let (page, _) = mount_writer({
            let requests = requests.clone();
            move || {
                use_context_provider(|| requests.clone());
                let host = use_context::<PendingCommandHost>();
                let mut origin = use_signal(|| true);
                rsx! {
                    button { onclick: move |_| origin.toggle(), {NAVIGATE.to_owned()} }
                    PendingCommandNotices { host }
                    if origin() { OriginPage {} }
                    else if gated_destination { GatedPage {} }
                    else { p { {"只读页面".to_owned()} } }
                }
            }
        });
        (page, requests)
    }

    #[component]
    fn OriginPage() -> Element {
        let requests = use_context::<Rc<SessionRequests>>();
        let reload = use_signal(|| 0);
        let gate = use_pending_command_gate();
        let owner = use_pending_commands_with_gate(reload, Some(gate), |_| true);
        requests.source.set(Some(owner));
        let success = use_signal(|| false);
        rsx! {
            button { disabled: owner.contains(SOURCE_ENTITY), onclick: move |_| {
                let send = requests.clone();
                let deliver = requests.clone();
                owner.submit(
                    SOURCE_ENTITY.to_owned(), "测试静默".to_owned(),
                    move |command_id| {
                        let send = send.clone();
                        async move { send.write(command_id).await }
                    },
                    move |()| {
                        let mut success = success;
                        // 若误交付到已卸载的来源，这次写入会 panic。
                        success.set(true);
                        deliver.delivered.set(deliver.delivered.get() + 1);
                    },
                    |_| {},
                );
            }, {SUBMIT.to_owned()} }
            button { onclick: move |_| {
                assert!(owner.submit_with_receipt("user:fixture".to_owned(), "生成 Key".to_owned(),
                    |_| ready(Ok("已生成的测试 Key".to_owned())),
                    |key, _| AdminUserGeneratedKey::delivery(Some(key), false), |_| {}));
            }, {"生成待确认 Key".to_owned()} }
            if success() { p { {"来源页收到回执".to_owned()} } }
            PendingCommandPanel { owner }
            if let Some((command_id, key)) = owner.generated_key() {
                AdminUserGeneratedKeyDialog { generated: key, on_close: move |_| owner.acknowledge_key(command_id) }
            }
        }
    }

    #[component]
    fn GatedPage() -> Element {
        let reload = use_signal(|| 0);
        let gate = use_pending_command_gate();
        let owner = use_pending_commands_with_gate(reload, Some(gate), |_| true);
        rsx! {
            button { disabled: owner.any_busy() || gate.blocked(), onclick: move |_| {
                assert!(owner.submit("world:fixture".to_owned(), "测试世界操作".to_owned(),
                    |_| ready(Ok(())), move |()| gate.hold_result(), |_| {}));
            }, {"交付一次性结果".to_owned()} }
            button { onclick: move |_| gate.acknowledge(), {"确认已查看".to_owned()} }
        }
    }

    #[test]
    fn in_flight_command_survives_navigation_and_replays_identical_bytes_from_shell() {
        let (mut page, requests) = mount_session_requests(true, [Ok(())], false);
        click_button(&mut page, SUBMIT);
        assert_eq!(requests.writes.borrow().len(), 1);
        assert!(button(&page, "原样重试").is_none());
        let source = requests.source.get().unwrap();
        click_button(&mut page, NAVIGATE);
        assert!(button(&page, SUBMIT).is_none());
        assert!(source.source.try_peek().is_err());
        assert!(source.is_busy(SOURCE_ENTITY));
        requests.delayed.as_ref().unwrap().respond(Err(failure(503, "storage_unavailable")));
        let unknown = settle_writer(&mut page);
        assert!(has_text(&unknown, "结果未知的操作"));
        assert!(button(&page, "原样重试").is_some());
        let succeeded = click_button(&mut page, "原样重试");
        assert!(button(&page, "原样重试").is_none());
        assert!(has_text(&succeeded, "已成功。"));
        assert!(source.host.entries.peek().is_empty());
        assert_eq!(source.host.notices.peek().len(), 1);
        assert_eq!(requests.delivered.get(), 0);
        let writes = requests.writes.borrow();
        assert_eq!(writes.len(), 2);
        assert_eq!(writes[0], writes[1]);
        click_button(&mut page, "关闭回执");
        assert!(source.host.notices.peek().is_empty());
    }

    #[test]
    fn abandoning_an_unknown_command_requires_confirmation_and_never_sends() {
        let (mut page, requests) =
            mount_session_requests(false, [Err(failure(503, "storage_unavailable"))], false);
        click_button(&mut page, SUBMIT);
        click_button(&mut page, NAVIGATE);
        let opened = click_button(&mut page, "放弃");
        assert!(has_text(&opened, "可能已经生效"));
        assert_eq!(requests.writes.borrow().len(), 1);
        click_button(&mut page, "保留未决操作");
        assert!(button(&page, "原样重试").is_some());
        assert!(button(&page, "确认放弃").is_none());
        click_button(&mut page, "放弃");
        click_button(&mut page, "确认放弃");
        assert!(button(&page, "原样重试").is_none());
        assert!(requests.source.get().unwrap().host.entries.peek().is_empty());
        assert_eq!(requests.writes.borrow().len(), 1);
        click_button(&mut page, NAVIGATE);
        assert!(button(&page, SUBMIT).is_some_and(|(_, disabled)| !disabled));
    }

    #[test]
    fn settled_commands_reload_the_mounted_source_without_posting_shell_receipts() {
        let (mut page, requests) =
            mount_session_requests(false, [Ok(()), Err(failure(409, "alert_stale"))], false);
        click_button(&mut page, SUBMIT);
        let source = requests.source.get().unwrap();
        assert!(has_live_text(&page, "来源页收到回执"));
        assert_eq!(*source.source.peek().reload.peek(), 1);
        assert!(source.host.entries.peek().is_empty());
        assert!(source.host.notices.peek().is_empty());
        click_button(&mut page, SUBMIT);
        assert_eq!(*source.source.peek().reload.peek(), 2);
        assert!(source.host.entries.peek().is_empty());
        assert!(source.host.notices.peek().is_empty());
        assert!(button(&page, "关闭已拒绝操作").is_some());
        assert!(button(&page, "原样重试").is_none());
        assert_eq!(requests.delivered.get(), 1);
    }

    #[test]
    fn a_detached_command_neither_occupies_nor_releases_another_pages_result_gate() {
        let (mut page, requests) = mount_session_requests(true, [], true);
        click_button(&mut page, SUBMIT);
        click_button(&mut page, NAVIGATE);
        click_button(&mut page, "交付一次性结果");
        assert!(button(&page, "交付一次性结果").unwrap().1);
        requests.delayed.as_ref().unwrap().respond(Ok(()));
        settle_writer(&mut page);
        assert!(button(&page, "交付一次性结果").unwrap().1);
        click_button(&mut page, "确认已查看");
        assert!(!button(&page, "交付一次性结果").unwrap().1);
        assert_eq!(requests.delivered.get(), 0);
    }

    #[test]
    fn unmount_immediately_unblocks_retry_at_the_shell() {
        let (mut page, requests) = mount_session_requests(
            false,
            [Err(failure(503, "storage_unavailable")), Ok(())],
            false,
        );
        click_button(&mut page, SUBMIT);
        click_button(&mut page, "生成待确认 Key");
        assert!(has_live_text(&page, "已生成的测试 Key"));
        assert!(button(&page, "查看生成的 Key").is_none());
        assert!(button(&page, "原样重试").unwrap().1);
        click_button(&mut page, NAVIGATE);
        assert!(has_live_text(&page, "只读页面"));
        assert!(!button(&page, "原样重试").unwrap().1);
        assert!(requests.source.get().unwrap().host.sources.peek().is_empty());
        assert!(!has_live_text(&page, "已生成的测试 Key"));
        assert!(button(&page, "查看生成的 Key").is_some());
        click_button(&mut page, "原样重试");
        click_button(&mut page, "查看生成的 Key");
        assert!(has_live_text(&page, "已生成的测试 Key"));
        click_button(&mut page, "关闭");
        assert!(button(&page, "查看生成的 Key").is_none());
        let writes = requests.writes.borrow();
        assert_eq!(writes.len(), 2);
        assert_eq!(writes[0], writes[1]);
    }

    #[component]
    fn GeneratedKeySource() -> Element {
        let reply = use_context::<Rc<RecordRead<String>>>();
        let reload = use_signal(|| 0);
        let owner = use_pending_commands(reload, |_| true);
        rsx! {
            button { onclick: move |_| {
                let reply = reply.clone();
                assert!(owner.submit_with_receipt(
                    "create-user".to_owned(), "测试创建用户".to_owned(),
                    move |_| {
                        let reply = reply.clone();
                        async move { reply.fetch().await }
                    },
                    |key, live| {
                        assert!(!live, "来源页已卸载");
                        AdminUserGeneratedKey::delivery(Some(key), false)
                    }, |_| {},
                ));
            }, {SUBMIT.to_owned()} }
        }
    }

    #[test]
    fn detached_generated_key_is_only_exposed_in_the_one_time_dialog() {
        let reply = Rc::new(RecordRead::new());
        let (mut page, _) = mount_writer({
            let reply = reply.clone();
            move || {
                use_context_provider(|| reply.clone());
                let host = use_context::<PendingCommandHost>();
                let mut origin = use_signal(|| true);
                rsx! {
                    button { onclick: move |_| origin.toggle(), {NAVIGATE.to_owned()} }
                    PendingCommandNotices { host }
                    if origin() { GeneratedKeySource {} }
                }
            }
        });
        click_button(&mut page, SUBMIT);
        click_button(&mut page, NAVIGATE);
        let key = "仅供测试的生成 Key";
        reply.respond(Ok(key.to_owned()));
        settle_writer(&mut page);
        assert!(!has_live_text(&page, key));
        click_button(&mut page, "查看生成的 Key");
        assert!(has_live_text(&page, key));
        click_button(&mut page, "关闭");
        assert!(!has_live_text(&page, key));
        assert!(button(&page, "查看生成的 Key").is_none());
    }
}
