// Admin 只读校验与页面动作的 busy 边界；持久命令及重试由 pending 宿主拥有。

import type { Cell } from "@auction/web-shared/reactive/cell";
import type { TaskContext, TaskScope } from "@auction/web-shared/reactive/tasks";
import type { Result } from "@auction/web-shared/result";

import type { AdminActionFeedback } from "./presentation";

/** 管理动作统一执行骨架：`busy` 置位 → spawn 执行 → 按结果回调 → `busy` 复位。
 * 只负责 busy 边界与 spawn，正常结束或取消均复位 busy；反馈、命令状态清理、权威数据
 * 刷新交给 `onSuccess` / `onFailure`。
 * `action` 收到任务上下文，接口函数用它的 `signal` 绑定取消。 */
export function runAdminAction<T, E>(
  tasks: TaskScope,
  busy: Cell<boolean>,
  action: (context: TaskContext) => Promise<Result<T, E>>,
  onSuccess: (value: T) => void,
  onFailure: (error: E) => void,
): void {
  busy.set(true);
  // 在 spawn 前建立释放入口，任务尚未首次执行就被取消时也能释放宿主的 busy 状态。
  let released = false;
  const releaseBusy = (): void => {
    if (released) {
      return;
    }
    released = true;
    // 页面销毁也可能释放信号；复位只影响已卸载页面的状态，不需要再检查宿主是否存活。
    busy.set(false);
  };
  const handle = tasks.spawn(async (context) => {
    // 取消边界就是 Rust guard 被丢弃的时机：signal 中止时立即释放 busy，不再等 action 结束。
    context.signal.addEventListener("abort", releaseBusy, { once: true });
    try {
      // Dioxus 的 spawn 先把任务入队；让出一次微任务，保留首次执行前取消的窗口。
      await Promise.resolve();
      if (!context.alive()) {
        return;
      }
      const result = await action(context);
      if (!context.alive()) {
        return;
      }
      if (result.ok) {
        onSuccess(result.value);
      } else {
        onFailure(result.error);
      }
    } finally {
      context.signal.removeEventListener("abort", releaseBusy);
      releaseBusy();
    }
  });
  // 宿主已销毁时 spawn 不执行任务体，释放入口不会被调用；这里补一次，避免 busy 永久置位。
  if (handle.finished()) {
    releaseBusy();
  }
}

/** 在 {@link runAdminAction} 上叠加 {@link AdminActionFeedback} 反馈写入的常见形态：
 * 动作开始前清空旧反馈，成功时先写入成功态再回调 `onSuccess`，失败时把稳定 code
 * 写入错误态。 */
export function runAdminFeedbackAction<T>(
  tasks: TaskScope,
  busy: Cell<boolean>,
  feedback: Cell<AdminActionFeedback | null>,
  action: (context: TaskContext) => Promise<Result<T, string>>,
  onSuccess: (value: T) => void,
): void {
  feedback.set(null);
  runAdminAction(
    tasks,
    busy,
    action,
    (value) => {
      feedback.set({ kind: "success" });
      onSuccess(value);
    },
    (code) => feedback.set({ kind: "error", code }),
  );
}
