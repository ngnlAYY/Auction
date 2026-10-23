//! 泛型两阶段确认命令状态机。
//!
//! 表单命令先冻结负载与 `command_id` 进入确认态，二次确认后才调用外部 API；同一输入重试期间
//! `command_id` 保持稳定以支撑服务端幂等。已提交命令未决时 `begin` 保留冻结负载；否则
//! 签名变化才生成新 `Uuid::new_v4()` 并清空上次失败。`submit` 成功清空状态并回调 `on_success`，
//! 失败退出确认态但保留命令与输入以便原样重试，`uncertain` 由调用方分类并在取得原结果或显式
//! 取消前保留——前置拒绝不能否定先前提交。`cancel_confirmation` 回编辑态但保留冻结命令，
//! `reset` 彻底清空。
//!
//! hook 不硬编码文案与业务字段；`ConfirmCommand` 为 `Copy`，宿主需要共享 `submitting` 时提升到
//! 创建处并整体下发。

use dioxus::core::{Runtime, current_scope_id};
use dioxus::prelude::*;
use uuid::Uuid;

/// 两阶段确认命令的共享状态。
/// `P` 为命令负载（至少携带 `command_id: Uuid`），`E` 为提交失败错误类型。
/// 手写 `Clone`/`Copy` 而不 derive：字段都是 `Signal`，derive 会给 `P`/`E` 强加 `Clone` 约束。
pub struct ConfirmCommand<P: 'static, E: 'static> {
    /// 已冻结的待执行命令；跨确认态退出与提交失败保留，成功后清空。
    pub command: Signal<Option<P>>,
    /// 当前处于确认态的命令；`Some` 时表单渲染确认视图。
    pub confirmation: Signal<Option<P>>,
    /// 提交中；弹窗宿主据此禁用 Escape / 遮罩 / 关闭按钮。
    pub submitting: Signal<bool>,
    /// 最近一次提交失败；输入变化或新一轮 `begin` 时清除。
    pub failure: Signal<Option<E>>,
    /// 已提交命令的结果仍不确定；重试期间保持置位，直到成功取得原结果或显式放弃。
    pub uncertain: Signal<bool>,
    /// 冻结的「输入签名 → command_id」；签名不变时重试复用同一 command_id。
    frozen: Signal<Option<(String, Uuid)>>,
    /// 状态创建时的宿主；子组件替换不取消提交，宿主卸载仍取消其全部任务。
    owner_scope: ScopeId,
}

impl<P: 'static, E: 'static> Clone for ConfirmCommand<P, E> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<P: 'static, E: 'static> Copy for ConfirmCommand<P, E> {}

/// 创建一套两阶段确认命令状态。
pub fn use_confirm_command<P, E>() -> ConfirmCommand<P, E> {
    ConfirmCommand {
        command: use_signal(|| None),
        confirmation: use_signal(|| None),
        submitting: use_signal(|| false),
        failure: use_signal(|| None),
        uncertain: use_signal(|| false),
        frozen: use_signal(|| None),
        owner_scope: use_hook(current_scope_id),
    }
}

impl<P: Clone + 'static, E: 'static> ConfirmCommand<P, E> {
    /// 第一阶段：冻结负载并进入确认态，返回冻结后的负载。
    /// 签名一致时复用其 `command_id`（同一输入的重试幂等），否则生成 `Uuid::new_v4()`。
    /// 提交中或结果未知时忽略迟到的编辑事件，不能替换原负载与 UUID。
    pub fn begin(&mut self, signature: String, build: impl FnOnce(Uuid) -> P) -> P {
        if (*self.submitting.peek() || *self.uncertain.peek())
            && let Some(pending) = self.command.peek().as_ref()
        {
            return pending.clone();
        }
        let command_id = match (self.frozen)() {
            Some((current, command_id)) if current == signature => command_id,
            _ => {
                let command_id = Uuid::new_v4();
                self.frozen.set(Some((signature, command_id)));
                command_id
            }
        };
        let pending = build(command_id);
        self.command.set(Some(pending.clone()));
        self.confirmation.set(Some(pending.clone()));
        self.failure.set(None);
        self.uncertain.set(false);
        pending
    }

    /// 第二阶段：提交已确认的负载。
    /// 成功清空全部状态并回调 `on_success`；失败退出确认态、记录错误，保留 `command`
    /// 与调用方输入后回调 `on_failure`。`outcome_uncertain` 由 surface 分类错误，避免
    /// 共享状态机绑定 HTTP 或业务错误类型。
    pub fn submit<R, F, Fut>(
        &mut self,
        pending: P,
        action: F,
        outcome_uncertain: impl FnOnce(&E) -> bool + 'static,
        on_success: impl FnOnce(R) + 'static,
        on_failure: impl FnOnce() + 'static,
    ) where
        R: 'static,
        F: FnOnce(P) -> Fut + 'static,
        Fut: std::future::Future<Output = Result<R, E>> + 'static,
    {
        if *self.submitting.peek() {
            return;
        }
        let pending = if *self.uncertain.peek() {
            let Some(pending) = self.command.peek().clone() else { return };
            pending
        } else {
            pending
        };
        self.failure.set(None);
        self.submitting.set(true);
        let mut state = *self;
        // in_scope 后使用正常 spawn，使任务登记到宿主 spawned_tasks；不能直接调用
        // Runtime::spawn 或使用全局任务，否则身份/lot 切换时任务不会随宿主一起取消。
        Runtime::current().in_scope(self.owner_scope, || {
            spawn(async move {
                let result = action(pending).await;
                state.submitting.set(false);
                match result {
                    Ok(value) => {
                        state.command.set(None);
                        state.confirmation.set(None);
                        state.frozen.set(None);
                        state.uncertain.set(false);
                        on_success(value);
                    }
                    Err(error) => {
                        // 失败保留输入、command_id 与错误，退出确认态以便修正或原样重试。
                        state.confirmation.set(None);
                        // 重试的 401/429 等前置拒绝并不能证明上一请求未提交；只在成功
                        // 取得原命令结果或显式取消时解除已有不确定状态。
                        let uncertain = *state.uncertain.peek() || outcome_uncertain(&error);
                        state.uncertain.set(uncertain);
                        state.failure.set(Some(error));
                        on_failure();
                    }
                }
            })
        });
    }

    /// 从确认态返回编辑态；冻结的命令与 `command_id` 保留，同签名输入仍会复用。
    pub fn cancel_confirmation(&mut self) {
        self.confirmation.set(None);
    }

    /// 彻底清空命令状态（弹窗关闭路径使用）。
    pub fn reset(&mut self) {
        if *self.submitting.peek() {
            return;
        }
        self.command.set(None);
        self.confirmation.set(None);
        self.frozen.set(None);
        self.failure.set(None);
        self.uncertain.set(false);
    }
}
