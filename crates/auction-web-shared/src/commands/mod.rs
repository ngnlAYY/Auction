//! 泛型两阶段确认命令状态机。
//!
//! 吸收自 Public 的三份确认骨架（`bid.rs`、`proxy/create.rs`、`proxy/authorization.rs`）
//! 与 Admin 的 `stable_command_id` 模式：表单类命令先在本地冻结负载与 `command_id` 进入
//! 确认态，用户二次确认后才真正调用外部 API；同一输入重试期间 `command_id` 保持稳定以
//! 支撑服务端幂等，输入一旦变更则重新生成。
//!
//! 状态机：
//! - `begin(signature, build)`：第一阶段。签名（调用方从输入字段序列化而来）与已冻结
//!   命令一致时复用其 `command_id`，否则生成新的 `Uuid::new_v4()`；随后用 `build` 构造
//!   负载，写入 `command` 与 `confirmation` 并清除上次失败。
//! - `submit(pending, action, outcome_uncertain, on_success, on_failure)`：第二阶段。置 `submitting`，
//!   调用方提供的 async 闭包执行真正的命令提交。成功时清空全部命令状态并回调
//!   `on_success`；失败时退出确认态、记录错误，但**保留** `command`（含 `command_id`）
//!   与调用方持有的输入信号，以便修正/原样重试，随后回调 `on_failure`（供调用方刷新
//!   权威数据等）。调用方通过 `outcome_uncertain` 分类是否只能先重放冻结命令，
//!   `uncertain` 在重试中保留，直到成功取得原结果或显式取消；后续前置拒绝不能否定先前提交。
//! - `cancel_confirmation()`：从确认态返回编辑态，不丢弃已冻结的命令。
//! - `reset()`：彻底清空（弹窗关闭路径使用）。
//!
//! hook 不硬编码任何文案与业务字段：签名构造、负载形状、错误类型和提交动作全部由
//! 调用方决定。`ConfirmCommand` 为 `Copy`，宿主要为弹窗 busy 守卫共享 `submitting`
//! 时，把 hook 提升到弹窗宿主创建、整体作为 prop 下发即可。

use dioxus::core::{Runtime, current_scope_id};
use dioxus::prelude::*;
use uuid::Uuid;

/// 两阶段确认命令的共享状态。
///
/// 泛型：`P` 为命令负载（至少应携带 `command_id: Uuid`，由 `begin` 的 `build` 注入）；
/// `E` 为提交失败错误类型。
///
/// 手写 `Clone`/`Copy` 而不 derive：所有字段都是 `Signal`（本身 `Copy`），derive 会
/// 给 `P`/`E` 强加不必要的 `Clone` 约束。
pub struct ConfirmCommand<P: 'static, E: 'static> {
    /// 已冻结的待执行命令；跨确认态退出与提交失败保留，成功后清空。
    pub command: Signal<Option<P>>,
    /// 当前处于确认态的命令；`Some` 时表单应渲染确认视图。
    pub confirmation: Signal<Option<P>>,
    /// 提交中；弹窗宿主据此禁用 Escape / 遮罩 / 关闭按钮（busy 守卫）。
    pub submitting: Signal<bool>,
    /// 最近一次提交失败；输入变化或新一轮 `begin` 时由调用方/hook 清除。
    pub failure: Signal<Option<E>>,
    /// 已提交命令的结果仍不确定；重试期间保持置位，直到成功取得原结果或显式放弃。
    pub uncertain: Signal<bool>,
    /// 内部冻结的「输入签名 → command_id」；签名不变时重试复用同一 command_id。
    frozen: Signal<Option<(String, Uuid)>>,
    /// 状态创建时的宿主；表单子组件替换不取消提交，宿主卸载仍取消其全部任务。
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
    ///
    /// `signature` 由调用方从当前输入字段序列化（如 `format!("{amount}:{mode}")`）；
    /// 与已冻结签名一致时复用其 `command_id`（同一输入的重试幂等），不一致时生成
    /// `Uuid::new_v4()`。`build` 用确定的 `command_id` 构造负载。
    pub fn begin(&mut self, signature: String, build: impl FnOnce(Uuid) -> P) -> P {
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
    ///
    /// `action` 是调用方提供的真正命令请求（通常携带 `command_id` 命中服务端幂等）。
    /// 成功清空全部命令状态并回调 `on_success`；失败退出确认态、记录错误，保留
    /// `command`（含 `command_id`）与调用方持有的输入，随后回调 `on_failure`。
    /// `outcome_uncertain` 由 surface 分类错误，避免共享状态机绑定 HTTP 或业务错误类型。
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

    /// 从确认态返回编辑态；已冻结的命令与 `command_id` 保留，重新提交同签名输入时
    /// 仍会复用。
    pub fn cancel_confirmation(&mut self) {
        self.confirmation.set(None);
    }

    /// 彻底清空命令状态（弹窗关闭路径使用）。
    pub fn reset(&mut self) {
        self.command.set(None);
        self.confirmation.set(None);
        self.frozen.set(None);
        self.failure.set(None);
        self.uncertain.set(false);
    }
}
