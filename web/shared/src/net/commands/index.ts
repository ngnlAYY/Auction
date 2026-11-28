// 泛型两阶段确认命令状态机。
//
// 表单命令先冻结负载与 `command_id` 进入确认态，二次确认后才调用外部 API；同一输入重试期间
// `command_id` 保持稳定以支撑服务端幂等。结果未知在取得原结果或显式取消前保留——前置拒绝
// 不能否定先前提交。各阶段语义见对应方法文档。
//
// hook 不硬编码文案与业务字段；宿主需要共享 `submitting` 时提升到创建处并整体下发。

import { createUuidV4 } from "../../platform/uuid";
import { type Cell, createCell } from "../../reactive/cell";
import { createTaskScope } from "../../reactive/tasks";
import type { Result } from "../../result";

/**
 * 两阶段确认命令的共享状态。`P` 为命令负载，且必须是可结构化克隆的纯数据；
 * `E` 为提交失败错误类型。
 */
export interface ConfirmCommand<P, E> {
  /** 已冻结的待执行命令；跨确认态退出与提交失败保留，成功后清空。 */
  readonly command: Cell<P | null>;
  /** 当前处于确认态的命令；非 `null` 时表单渲染确认视图。 */
  readonly confirmation: Cell<P | null>;
  /** 提交中；弹窗宿主据此禁用 Escape / 遮罩 / 关闭按钮。 */
  readonly submitting: Cell<boolean>;
  /** 最近一次提交失败；输入变化或新一轮 `begin` 时清除。 */
  readonly failure: Cell<E | null>;
  /** 已提交命令的结果仍不确定；重试期间保持置位，直到成功取得原结果或显式放弃。 */
  readonly uncertain: Cell<boolean>;
  /**
   * 第一阶段：冻结负载并进入确认态，返回冻结后的负载。
   * 负载必须是可结构化克隆的纯数据；内部状态、返回值和提交参数各取一份独立快照。
   * 签名一致时复用其 `command_id`；提交中或结果未知时忽略迟到的编辑事件。
   */
  begin(signature: string, build: (commandId: string) => P): P;
  /** 第二阶段：提交已确认的负载；成功清空全部状态，失败保留结果不确定标记。 */
  submit<R>(
    pending: P,
    action: (pending: P, signal: AbortSignal) => Promise<Result<R, E>>,
    outcomeUncertain: (error: E) => boolean,
    onSuccess: (value: R) => void,
    onFailure: () => void,
  ): void;
  /** 从确认态返回编辑态；冻结的命令与 `command_id` 保留，同签名输入仍会复用。 */
  cancelConfirmation(): void;
  /** 彻底清空命令状态（弹窗关闭路径使用）；提交中不执行。 */
  reset(): void;
}

/** 冻结的「输入签名 → command_id」；签名不变时重试复用同一 command_id。 */
interface FrozenCommand {
  readonly signature: string;
  readonly commandId: string;
}

/** 创建一套两阶段确认命令状态；任务归创建它的组件所有，子组件替换不取消提交。 */
export function useConfirmCommand<P, E>(): ConfirmCommand<P, E> {
  const command = createCell<P | null>(null);
  const confirmation = createCell<P | null>(null);
  const submitting = createCell(false);
  const failure = createCell<E | null>(null);
  const uncertain = createCell(false);
  const frozen = createCell<FrozenCommand | null>(null);
  // 创建时捕获宿主作用域：卸载会随组件取消全部任务，内部子组件变化不会。
  const tasks = createTaskScope();

  return {
    command,
    confirmation,
    submitting,
    failure,
    uncertain,
    begin(signature, build) {
      const existing = command.peek();
      if ((submitting.peek() || uncertain.peek()) && existing !== null) {
        return structuredClone(existing);
      }
      const remembered = frozen.peek();
      let commandId: string;
      if (remembered !== null && remembered.signature === signature) {
        commandId = remembered.commandId;
      } else {
        commandId = createUuidV4();
        frozen.set({ signature, commandId });
      }
      const pending = build(commandId);
      command.set(structuredClone(pending));
      confirmation.set(structuredClone(pending));
      failure.set(null);
      uncertain.set(false);
      return structuredClone(pending);
    },
    submit<R>(
      pending: P,
      action: (pending: P, signal: AbortSignal) => Promise<Result<R, E>>,
      outcomeUncertain: (error: E) => boolean,
      onSuccess: (value: R) => void,
      onFailure: () => void,
    ) {
      if (submitting.peek()) {
        return;
      }
      let submitted: P;
      if (uncertain.peek()) {
        const original = command.peek();
        if (original === null) {
          return;
        }
        submitted = structuredClone(original);
      } else {
        submitted = structuredClone(pending);
      }
      failure.set(null);
      submitting.set(true);
      tasks.spawn(async (context) => {
        let result: Result<R, E>;
        try {
          result = await action(submitted, context.signal);
        } catch (error) {
          // 宿主已卸载或任务被取消时的拒绝不是程序错误，与成功路径一样静默收口。
          if (!context.alive()) {
            return;
          }
          submitting.set(false);
          confirmation.set(null);
          uncertain.set(true);
          throw error;
        }
        if (!context.alive()) {
          return;
        }
        submitting.set(false);
        if (result.ok) {
          command.set(null);
          confirmation.set(null);
          frozen.set(null);
          uncertain.set(false);
          onSuccess(result.value);
          return;
        }
        confirmation.set(null);
        // 重试的 401/429 等前置拒绝并不能证明上一请求未提交；只在成功取得原命令结果
        // 或显式取消时解除已有不确定状态。
        uncertain.set(uncertain.peek() || outcomeUncertain(result.error));
        failure.set(result.error);
        onFailure();
      });
    },
    cancelConfirmation() {
      confirmation.set(null);
    },
    reset() {
      if (submitting.peek()) {
        return;
      }
      command.set(null);
      confirmation.set(null);
      frozen.set(null);
      failure.set(null);
      uncertain.set(false);
    },
  };
}
