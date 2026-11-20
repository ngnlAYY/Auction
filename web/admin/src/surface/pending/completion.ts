// 会话命令的完成判定、回执交付与读取失效通知。

import { err, ok, type Result } from "@auction/web-shared/result";

import {
  type AdminWriteFailure,
  adminWriteFailureDisplayCode,
  classifyAdminWriteFailure,
} from "../../api/failure";
import type {
  CommandDelivery,
  CommandIdentity,
  CommandNotice,
  CommandReceipt,
  PendingCommand,
  PendingCommandHost,
} from ".";
import { advanceCommandReload, type CommandSource, liveSource, reloadsFor } from "./sources";

/** 一次尝试的交付函数：页面已展示结果时报告已交付，否则返回仍需会话保留的内容。 */
export type CommandDeliver = (sourceAlive: boolean) => CommandDelivery;

/** 会话保存回执前复制并冻结，隔离调用方持有的对象。 */
export function copyCommandReceipt(receipt: CommandReceipt): CommandReceipt {
  if (receipt.kind === "generated-key") {
    return Object.freeze({
      kind: "generated-key" as const,
      key: receipt.key.copy(),
    });
  }
  return Object.freeze({ kind: "text" as const, text: receipt.text });
}

/** 回执结果与通知一起冻结，读取方拿到的内部值不可再改写。 */
function freezeCommandNotice(
  identity: CommandIdentity,
  result: Result<CommandReceipt | null, string>,
  keySource: WeakRef<CommandSource> | null,
): CommandNotice {
  const frozenResult: Result<CommandReceipt | null, string> = result.ok
    ? Object.freeze({ ok: true as const, value: result.value })
    : Object.freeze({ ok: false as const, error: result.error });
  return Object.freeze({ identity, result: frozenResult, keySource });
}

/** 冻结通知列表，避免读取方改写会话持有的记录。 */
export function freezeNotices(notices: readonly CommandNotice[]): readonly CommandNotice[] {
  return Object.freeze([...notices]);
}

function pushCommandNotice(host: PendingCommandHost, notice: CommandNotice): void {
  host.notices.update((notices) => freezeNotices([...notices, notice]));
}

/** 反馈回调的异常不能改变已经判定的命令结果，也不能把异常原文交给全局错误钩子。 */
function reportCommandFailure(entry: PendingCommand, code: string): void {
  try {
    entry.onFailure(code);
  } catch {
    // 页面反馈失败只影响展示；命令仍按已判定的结果保留或了结。
  }
}

/** 裁定本次尝试并交付回执，命令了结后通知仍匹配该实体的读取视图。 */
export function completeCommand(
  host: PendingCommandHost,
  commandId: string,
  result: Result<CommandDeliver, AdminWriteFailure>,
): void {
  if (!host.alive()) {
    return;
  }
  const current = host.entries.peek();
  const index = current.findIndex((entry) => entry.identity.commandId === commandId);
  const entry = index < 0 ? undefined : current[index];
  if (entry === undefined) {
    return;
  }
  // 只有经本 ID 的账本核对的拒绝才能清除较早的未知。其余未知保留原 UUID 与负载。
  if (!result.ok) {
    const scope = classifyAdminWriteFailure(result.error);
    if (scope === "unknown" || (entry.unknown && scope !== "replay_checked")) {
      const display = adminWriteFailureDisplayCode(result.error);
      host.entries.set(
        current.map((candidate, candidateIndex) =>
          candidateIndex === index
            ? { ...candidate, busy: false, unknown: true, error: display }
            : candidate,
        ),
      );
      const source = liveSource(host, entry.source);
      if (source !== null) {
        reportCommandFailure(entry, display);
      }
      return;
    }
  }
  host.entries.set(current.filter((_, candidateIndex) => candidateIndex !== index));
  const source = liveSource(host, entry.source);
  const reloads = reloadsFor(host, entry.identity.entity);
  if (result.ok) {
    let delivery: CommandDelivery;
    try {
      delivery = result.value(source !== null);
    } catch {
      // 页面成功反馈抛错不改变命令已经成功的结论，也不能让异常原文进入全局错误钩子。
      // 来源仍挂载时按已交付处理；外壳接管时保留一条无 Key 的成功回执，继续推进读取世代。
      delivery = source !== null ? { kind: "delivered" } : { kind: "receipt", receipt: null };
    }
    // 交付回调可能同步销毁认证宿主；清理过的宿主不能再写回执或占用门禁。
    if (!host.alive()) {
      return;
    }
    if (delivery.kind === "receipt") {
      // 已展示的 Key 随会话保留到确认。来源页卸载只解除本页门禁，外壳接管交付。
      // 回调也可能注销来源页；写入前重新查找，仍存活才把回执留给该页面。
      const currentSource = liveSource(host, entry.source);
      const receipt = delivery.receipt === null ? null : copyCommandReceipt(delivery.receipt);
      let keySource: WeakRef<CommandSource> | null = null;
      if (receipt !== null && receipt.kind === "generated-key") {
        const gate = currentSource === null ? null : currentSource.gate;
        if (gate !== null) {
          gate.holdResult();
          // 自定义门禁可以在 holdResult() 里同步销毁宿主；此时不能把 Key 写回旧宿主。
          if (!host.alive()) {
            return;
          }
          // 门禁也可能注销来源页；登记已失效时改由认证外壳接管回执。
          keySource = liveSource(host, entry.source) === null ? null : entry.source;
        }
      }
      pushCommandNotice(host, freezeCommandNotice(entry.identity, ok(receipt), keySource));
    }
  } else {
    const code = adminWriteFailureDisplayCode(result.error);
    if (source !== null) {
      reportCommandFailure(entry, code);
      // 拒绝回调也可能同步销毁宿主或注销来源页。宿主已销毁就结束交付；
      // 仅来源页卸载时，拒绝原因改由外壳持有，仍存活的旧页面不能继续接收。
      if (!host.alive()) {
        return;
      }
      const currentSource = liveSource(host, entry.source);
      if (currentSource !== null) {
        currentSource.rejections.update((notices) =>
          freezeNotices([...notices, freezeCommandNotice(entry.identity, err(code), null)]),
        );
      } else {
        pushCommandNotice(host, freezeCommandNotice(entry.identity, err(code), null));
      }
    } else {
      pushCommandNotice(host, freezeCommandNotice(entry.identity, err(code), null));
    }
  }
  // 同一轮已有回调推进的世代无需再次推进。新挂载且正在显示同一实体的视图也收到失效通知。
  // 不能只看登记表，否则父级持有、仅登记子组件卸载时会漏掉这次刷新；反过来，
  // 读取状态自身的作用域已销毁时也不能再推进（AK4b 在快照里提供这个存活判定）。
  for (const { reload, previous, alive } of reloads) {
    if (host.alive() && alive() && reload.peek() === previous) {
      advanceCommandReload(reload);
    }
  }
}
