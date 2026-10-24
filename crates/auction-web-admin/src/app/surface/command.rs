//! Admin 幂等命令与动作执行骨架的统一 helper。
//! - [`stable_scoped_command_id`]：多目标并行形态，按签名分别冻结 `command_id`，互不覆盖。
//! - [`run_admin_action`] / [`run_admin_feedback_action`]：动作执行骨架，统一 busy
//!   置位/复位与 spawn 边界。

use std::collections::HashMap;

use dioxus::prelude::*;
use uuid::Uuid;

use super::AdminActionFeedback;

/// 按签名分别冻结 `command_id` 的多目标形态；不同签名互不覆盖，成功后由调用方
/// `remove` 对应条目。
pub fn stable_scoped_command_id(
    mut command_ids: Signal<HashMap<String, Uuid>>,
    signature: &str,
) -> Uuid {
    if let Some(command_id) = command_ids.read().get(signature).copied() {
        return command_id;
    }
    let command_id = Uuid::new_v4();
    command_ids.write().insert(signature.to_owned(), command_id);
    command_id
}

/// 管理动作统一执行骨架：`busy` 置位 → spawn 执行 → 按结果回调 → `busy` 复位。
/// 只负责 busy 边界与 spawn，正常结束或取消均复位 busy；反馈、命令状态清理、权威数据
/// 刷新交给 `on_success` / `on_failure`。
pub fn run_admin_action<T, E, F>(
    mut busy: Signal<bool>,
    action: F,
    on_success: impl FnOnce(T) + 'static,
    on_failure: impl FnOnce(E) + 'static,
) where
    T: 'static,
    E: 'static,
    F: std::future::Future<Output = Result<T, E>> + 'static,
{
    busy.set(true);
    // 在 spawn 前建立 guard，任务尚未首次 poll 就被取消时也能释放宿主的 busy 状态。
    let guard = AdminActionBusyGuard(busy);
    spawn(async move {
        let _guard = guard;
        match action.await {
            Ok(value) => on_success(value),
            Err(error) => on_failure(error),
        }
    });
}

struct AdminActionBusyGuard(Signal<bool>);

impl Drop for AdminActionBusyGuard {
    fn drop(&mut self) {
        // 页面销毁也可能释放信号；清理不得因为失效 Signal 再次 panic。
        if let Ok(mut busy) = self.0.try_write() {
            *busy = false;
        }
    }
}

/// 在 [`run_admin_action`] 上叠加 [`AdminActionFeedback`] 反馈写入的常见形态：
/// 动作开始前清空旧反馈，成功时先写入 `Success` 再回调 `on_success`，失败时把稳定
/// code 写入 `Error`。
pub fn run_admin_feedback_action<T, F>(
    busy: Signal<bool>,
    mut feedback: Signal<Option<AdminActionFeedback>>,
    action: F,
    on_success: impl FnOnce(T) + 'static,
) where
    T: 'static,
    F: std::future::Future<Output = Result<T, String>> + 'static,
{
    feedback.set(None);
    run_admin_action(
        busy,
        action,
        move |value| {
            feedback.set(Some(AdminActionFeedback::Success));
            on_success(value);
        },
        move |code| feedback.set(Some(AdminActionFeedback::Error(code))),
    );
}
