//! 当前 Admin 稳定 owner 内已提交命令的稀疏宿主；不保存未提交草稿或成功结果。
//! 工厂只拥有首次提交的完整参数与 UUID，重试不读取行、版本或表单。任务登记在 owner
//! 的 Scope，行或弹窗卸载不取消任务；owner 销毁同时释放工厂，敏感负载只驻留内存。

use std::{future::Future, pin::Pin, rc::Rc};

use auction_web_shared::{TruncatedText, t};
use dioxus::{core::Runtime, prelude::*};
use uuid::Uuid;

use super::admin_api_error_message;

type Attempt = Pin<Box<dyn Future<Output = Result<(), String>>>>;
type CommandFactory = Rc<dyn Fn(Uuid) -> Attempt>;

struct PendingCommand {
    entity: String,
    action: String,
    command_id: Uuid,
    busy: bool,
    unknown: bool,
    error: Option<String>,
    factory: CommandFactory,
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
    /// 同实体的未知、忙碌及待确认拒绝都阻止新意图；读信号使行按钮随集合变化更新。
    pub(crate) fn contains(self, entity: &str) -> bool {
        self.entries.read().iter().any(|entry| entry.entity == entity)
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
    /// 必须 try_write，EventHandler 只能来自稳定页面（例如用户一次性 Key 的交付）。
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
                        on_failure(code.clone());
                        Err(code)
                    }
                }
            })
        });
        self.entries.write().push(PendingCommand {
            entity,
            action,
            command_id,
            busy: false,
            unknown: false,
            error: None,
            factory,
        });
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
        let factory = entry.factory.clone();
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
                            // 未知是累计事实：后续401/429或其他拒绝不能证明先前请求未提交。
                            entry.unknown |= !definite_rejection(&code);
                            entry.error = Some(code);
                        }
                    }
                }
            });
        });
    }

    fn dismiss(mut self, command_id: Uuid) {
        self.entries.write().retain(|entry| {
            entry.command_id != command_id || entry.busy || entry.unknown || entry.error.is_none()
        });
    }
}

/// 页面命令完成后只回写仍存活的行状态，旧 Signal 不会指向新挂载的实体。
pub(crate) fn set_if_alive<T: 'static>(mut signal: Signal<T>, value: T) {
    if let Ok(mut target) = signal.try_write() {
        *target = value;
    }
}

/// 只认明确无写入的拒绝码；超时、断连、无效响应、服务端故障和未识别码保守保留。
pub(crate) fn definite_rejection(code: &str) -> bool {
    matches!(
        code,
        "invalid_request"
            | "unauthenticated"
            | "forbidden"
            | "rate_limited"
            | "http_400"
            | "http_401"
            | "http_403"
            | "http_429"
            | "stale_state_version"
            | "stale_user_state"
            | "stale_credential_state"
            | "stale_delivery_state"
            | "stale_world_state"
            | "rename_not_revertable"
            | "user_conflict"
            | "world_not_found"
            | "invalid_user_command"
            | "invalid_delivery_command"
            | "delivery_sent_sighted"
            | "invalid_mail_request"
            | "poll_rejected"
            | "invalid_locale"
            | "invalid_rule_document"
            | "invalid_runtime_config"
            | "invalid_replay"
            | "state_conflict"
            | "conflict"
            | "command_conflict"
            | "not_found"
            | "invalid_auction_state"
            | "invalid_lifecycle"
            | "equipment_conflict"
            | "seller_blacklisted"
            | "auction_intake_player_blacklisted"
    )
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
    rsx! {
        if !entries.is_empty() {
            section { class: "admin-panel admin-pending", aria_label: t!("admin-pending-title"),
                h2 { {t!("admin-pending-title")} }
                p { class: "admin-panel__note",
                    if session_scoped { {t!("admin-pending-session-help")} } else { {t!("admin-pending-help")} }
                }
                ul { class: "admin-pending__list",
                    for entry in entries.iter() {
                        {
                            let command_id = entry.command_id;
                            rsx! {
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
                                        button { r#type: "button",
                                            disabled: entry.busy || writes_blocked,
                                            onclick: move |_| owner.retry(command_id),
                                            {t!("admin-pending-retry")}
                                        }
                                        if !entry.busy && !entry.unknown {
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
    }
}
