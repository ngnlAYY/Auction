//! 按已确认身份管理已发出的竞价命令、请求任务与一次性回执。

use std::collections::BTreeMap;
use std::rc::Rc;

use dioxus::core::{Runtime, Task, current_scope_id};
use dioxus::prelude::*;
use uuid::Uuid;

use super::notice::CompetitionNotices;
use super::request::{
    CommandCompletion, CommandPayload, CommandReceipt, CommandStatus, CommandTarget, SentCommand,
    outcome_unknown,
};
use super::session::{CommandSession, use_command_session};
use crate::api::{CommandError, estimated_now};
use crate::surface::CatalogProxySnapshots;

/// 稳定的外壳入口；身份切换时只替换命令宿主。
#[derive(Clone, Copy)]
pub(in crate::surface) struct CompetitionCommands {
    active: Signal<Option<CommandStore>>,
    identity_revision: Signal<u64>,
}

/// 弹窗实例标识，仅比较信号身份。请求任务不读取或写入弹窗拥有的信号。
pub(in crate::surface) type CommandDialogKey = Signal<()>;

/// 在路由出口上方提供命令宿主入口。
pub(in crate::surface) fn use_competition_commands_provider() -> CompetitionCommands {
    let active = use_signal(|| None::<CommandStore>);
    let identity_revision = use_signal(|| 0_u64);
    use_context_provider(|| CompetitionCommands { active, identity_revision })
}

impl CompetitionCommands {
    pub(in crate::surface) fn current(self) -> Option<CommandStore> {
        self.active.read().as_ref().filter(|store| store.session.identity_confirmed()).cloned()
    }

    pub(in crate::surface) fn discard_identity(mut self) {
        let active = self.active.write().take();
        if let Some(mut store) = active {
            // 认证失败的请求也可能正在调用本入口；先释放状态写借用，再取消身份任务。
            let retired = std::mem::take(&mut *store.state.write());
            for task in retired.tasks.into_values().flatten() {
                task.cancel();
            }
        }
        // 复核即使在下一帧确认同一账号，已销毁的宿主也不能重新登记。
        let next = self.identity_revision.peek().wrapping_add(1);
        self.identity_revision.set(next);
    }

    pub(in crate::surface) fn detach(self, dialog: CommandDialogKey) {
        if let Ok(active) = self.active.try_peek()
            && let Some(store) = &*active
        {
            store.detach(dialog);
        }
    }

    pub(super) fn can_start(self, lot_id: &str) -> bool {
        self.current().is_some_and(|store| {
            let state = store.state.peek();
            store.can_submit()
                && !state.pending.contains_key(lot_id)
                && state.dialog_receipt.is_none()
        })
    }
}

#[component]
pub(in crate::surface) fn CompetitionCommandHost(owner: Signal<Option<String>>) -> Element {
    let user_id = owner();
    let revision = (use_context::<CompetitionCommands>().identity_revision)();
    rsx! {
        for user_id in user_id {
            IdentityCommands { key: "{revision}:{user_id}", user_id, owner, revision }
        }
        CompetitionNotices {}
    }
}

#[component]
fn IdentityCommands(user_id: String, owner: Signal<Option<String>>, revision: u64) -> Element {
    let commands = use_context::<CompetitionCommands>();
    let mut active = commands.active;
    let session = use_command_session(Some(user_id.clone()), owner);
    let state = use_signal(CommandState::default);
    let proxy_snapshots = use_hook(CatalogProxySnapshots::default);
    let scope = use_hook(current_scope_id);
    let refresh = use_context::<Signal<u64>>();
    let parsed = Uuid::try_parse(&user_id).ok();
    use_effect(move || {
        if let Some(parsed) = parsed
            && owner.peek().as_deref() == Some(user_id.as_str())
            && *commands.identity_revision.peek() == revision
        {
            active.set(Some(CommandStore {
                user_id: parsed,
                session: session.clone(),
                state,
                scope,
                refresh,
                proxy_snapshots: proxy_snapshots.clone(),
            }));
        }
    });
    use_drop(move || {
        if let Ok(mut current) = active.try_write()
            && current.as_ref().is_some_and(|store| store.state == state)
        {
            *current = None;
        }
    });
    rsx! {}
}

#[derive(Default)]
struct CommandState {
    pending: BTreeMap<String, SentCommand>,
    tasks: BTreeMap<Uuid, Option<Task>>,
    active_dialog: Option<CommandDialogKey>,
    dialog_receipt: Option<CommandReceipt>,
    receipts: Vec<CommandReceipt>,
    latest_receipt: Option<Uuid>,
}

impl CommandState {
    fn show_receipt(&mut self, receipt: CommandReceipt) {
        self.latest_receipt = Some(receipt.command_id);
        self.receipts.push(receipt);
    }
}

/// 命令状态和任务都归同一个按用户加键的 scope，路由只读状态并发起明确操作。
#[derive(Clone)]
pub(in crate::surface) struct CommandStore {
    user_id: Uuid,
    session: CommandSession,
    state: Signal<CommandState>,
    scope: ScopeId,
    refresh: Signal<u64>,
    proxy_snapshots: CatalogProxySnapshots,
}

impl CommandStore {
    /// 代理快照与命令一起在确认换号或退出时销毁。
    pub(in crate::surface) fn proxy_snapshots(
        &self,
        user_id: Uuid,
    ) -> Option<&CatalogProxySnapshots> {
        (self.user_id == user_id).then_some(&self.proxy_snapshots)
    }

    pub(in crate::surface) fn command(&self, lot_id: &str) -> Option<SentCommand> {
        self.state.read().pending.get(lot_id).cloned()
    }

    pub(super) fn unknown_commands(&self) -> Vec<SentCommand> {
        let mut commands = self
            .state
            .read()
            .pending
            .values()
            .filter(|command| command.status.unknown())
            .cloned()
            .collect::<Vec<_>>();
        commands.sort_by_key(|command| command.sent_at);
        commands
    }

    pub(super) fn receipts(&self) -> (Vec<CommandReceipt>, Option<Uuid>) {
        let state = self.state.read();
        (state.receipts.clone(), state.latest_receipt)
    }

    pub(in crate::surface) fn can_submit(&self) -> bool {
        self.session.can_submit_new()
    }

    pub(in crate::surface) fn submit(
        mut self,
        target: Rc<CommandTarget>,
        payload: CommandPayload,
        origin: CommandDialogKey,
    ) -> bool {
        if !self.can_submit() || payload.user_id() != self.user_id {
            return false;
        }
        let command = SentCommand {
            target: target.clone(),
            payload: Rc::new(payload),
            sent_at: estimated_now(),
            status: CommandStatus::Submitting,
        };
        {
            let mut state = self.state.write();
            // 同一用户、同一拍品只有一个在途或结果未知的命令，迟到的提交事件也要经过此门禁。
            if state.pending.contains_key(&target.lot_id) || state.dialog_receipt.is_some() {
                return false;
            }
            state.pending.insert(target.lot_id.clone(), command.clone());
            state.active_dialog = Some(origin);
        }
        self.send(command, Some(origin), false);
        true
    }

    pub(super) fn retry(
        mut self,
        lot_id: &str,
        command_id: Uuid,
        origin: Option<CommandDialogKey>,
    ) {
        if !self.can_submit() {
            return;
        }
        let command = {
            let mut state = self.state.write();
            let Some(command) = state.pending.get_mut(lot_id) else { return };
            if command.payload.command_id() != command_id {
                return;
            }
            let CommandStatus::Unknown { retrying, .. } = &mut command.status else { return };
            if *retrying {
                return;
            }
            *retrying = true;
            let command = command.clone();
            if let Some(origin) = origin {
                state.active_dialog = Some(origin);
            }
            command
        };
        self.send(command, origin, true);
    }

    pub(super) fn abandon(mut self, lot_id: &str, command_id: Uuid) {
        let mut state = self.state.write();
        if state.pending.get(lot_id).is_some_and(|command| {
            command.payload.command_id() == command_id
                && matches!(command.status, CommandStatus::Unknown { retrying: false, .. })
        }) {
            // 明确放弃仅清理浏览器记录，绝不发送撤回或替代命令。
            state.pending.remove(lot_id);
        }
    }

    fn send(mut self, command: SentCommand, origin: Option<CommandDialogKey>, retry: bool) {
        let command_id = command.payload.command_id();
        let mut state = self.state;
        // 先登记命令身份，再 spawn；立即完成的 future 也能通过成员检查并提交结果。
        state.write().tasks.insert(command_id, None);
        let task = Runtime::current().in_scope(self.scope, || {
            // spawn 登记到身份 scope。界面卸载只解除回执接收，身份销毁才会取消任务。
            spawn(async move {
                // 交接后可能开始会话复核。首次尝试须回显未发送，重试须保留原未结记录。
                let result = if self.can_submit() {
                    command.payload.send(&command.target).await
                } else {
                    Err(CommandError::before_send())
                };
                if !self.state.peek().tasks.contains_key(&command_id) {
                    return;
                }
                if !self.session.identity_confirmed() {
                    self.complete(
                        command,
                        origin,
                        retry,
                        Err(CommandError { code: "invalid_response".to_owned(), status: 0 }),
                    );
                    return;
                }
                // 三类命令的 API 在请求结束时已经失效公开读缓存；推进外壳修订让存活的资源
                // 重读权威视图，回执中的旧价格或授权状态不写进读模型。
                if result.as_ref().err().is_none_or(|error| !error.not_sent()) {
                    let next_refresh = self.refresh.peek().saturating_add(1);
                    self.refresh.set(next_refresh);
                }
                self.complete(command, origin, retry, result);
            })
        });
        let mut task = Some(task);
        {
            let mut current = state.write();
            if let Some(slot) = current.tasks.get_mut(&command_id) {
                *slot = task.take();
            }
        }
        if let Some(task) = task {
            task.cancel();
        }
    }

    fn complete(
        &mut self,
        mut command: SentCommand,
        origin: Option<CommandDialogKey>,
        retry: bool,
        result: Result<(), CommandError>,
    ) {
        let mut state = self.state.write();
        state.tasks.remove(&command.payload.command_id());
        let lot_id = &command.target.lot_id;
        let error = result.err();
        if let Some(error) = error.as_ref().filter(|error| outcome_unknown(error, retry)) {
            if error.not_sent()
                && let CommandStatus::Unknown { retrying, .. } = &mut command.status
            {
                // 没有发出的重试不能覆盖上一次请求的结果。
                *retrying = false;
            } else {
                command.status = CommandStatus::Unknown { retrying: false, error: error.clone() };
            }
            state.pending.insert(lot_id.clone(), command);
            return;
        }
        state.pending.remove(lot_id);
        let receipt = CommandReceipt {
            command_id: command.payload.command_id(),
            target: command.target,
            kind: command.payload.kind(),
            completion: match error {
                Some(error) => CommandCompletion::Rejected(error),
                None => CommandCompletion::Accepted,
            },
        };
        if origin.is_some() && origin == state.active_dialog {
            state.dialog_receipt = Some(receipt);
        } else {
            state.show_receipt(receipt);
        }
    }

    pub(in crate::surface) fn take_dialog_receipt(
        mut self,
        dialog: CommandDialogKey,
    ) -> Option<CommandReceipt> {
        let state = self.state.read();
        if state.active_dialog != Some(dialog) || state.dialog_receipt.is_none() {
            return None;
        }
        drop(state);
        self.state.write().dialog_receipt.take()
    }

    fn detach(&self, dialog: CommandDialogKey) {
        let mut state = self.state;
        if let Ok(mut state) = state.try_write()
            && state.active_dialog == Some(dialog)
        {
            state.active_dialog = None;
            // 响应可能与导航落在同一轮；弹窗尚未消费的回执仍在外壳展示一次。
            if let Some(receipt) = state.dialog_receipt.take() {
                state.show_receipt(receipt);
            }
        }
    }

    pub(super) fn dismiss(mut self, command_id: Uuid) {
        let mut state = self.state.write();
        state.receipts.retain(|receipt| receipt.command_id != command_id);
        if state.latest_receipt == Some(command_id) {
            state.latest_receipt = None;
        }
    }
}
