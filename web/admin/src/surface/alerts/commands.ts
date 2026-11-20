// 告警确认/静默命令：点击时冻结事件代数和表单参数，首次提交与未决重试共用同一份负载。

import type { Cell } from "@auction/web-shared/reactive/cell";
import { ok } from "@auction/web-shared/result";
import { t } from "@auction/web-shared/ui/i18n";

import {
  type AdminAlertCommandPreparation,
  acknowledgeAdminAlert,
  prepareAdminAlertAcknowledge,
  prepareAdminAlertSilence,
  silenceAdminAlert,
} from "../../api/alerts";
import type { AdminSessionHost } from "../../api/session";
import { type PendingCommands, setIfAlive } from "../pending";
import { advanceCommandReload } from "../pending/sources";
import type { AdminActionFeedback } from "../presentation";

/** 服务端允许的四档静默时长（小时）。类型收窄保证换算成秒时不会超出安全整数。 */
export type AlertSilenceHours = 1 | 8 | 24 | 72;

/** 点击时冻结本行的事件代数和表单参数。首次提交与原样重试共用这份负载，
 * 列表重读不能替换未决命令的事件代数，避免将旧意图落到复发后的告警。 */
export type AlertCommand =
  | {
      readonly kind: "acknowledge";
      readonly alertId: string;
      readonly incidentGeneration: number;
      readonly reason: string;
    }
  | {
      readonly kind: "silence";
      readonly alertId: string;
      readonly incidentGeneration: number;
      readonly reason: string;
      readonly silenceSeconds: number;
    };

export function alertCommandAcknowledge(
  alertId: string,
  incidentGeneration: number,
  reason: string,
): AlertCommand {
  return {
    kind: "acknowledge",
    alertId,
    incidentGeneration,
    reason: rustTrim(reason),
  };
}

export function alertCommandSilence(
  alertId: string,
  incidentGeneration: number,
  hours: AlertSilenceHours,
  reason: string,
): AlertCommand {
  return {
    kind: "silence",
    alertId,
    incidentGeneration,
    reason: rustTrim(reason),
    // 类型已收窄到服务端允许的四个选项，乘法不会超出安全整数；对应 Rust 的饱和乘法。
    silenceSeconds: hours * 3_600,
  };
}

function alertCommandAlertId(command: AlertCommand): string {
  return command.alertId;
}

export function alertCommandReason(command: AlertCommand): string {
  return command.reason;
}

/** 未决宿主的实体键；同一告警的确认与静默互斥。 */
export function alertCommandEntity(alertId: string): string {
  return `alert:${alertId}`;
}

/** 服务端明确拒绝、可能意味着列表快照过时（signal 已恢复或状态已变）的告警命令错误码；
 * invalid_alert_command 也可能来自参数校验，此时重读同样无害。其余失败（含结果未知）不触发重读。 */
function alertSnapshotStale(code: string): boolean {
  return code === "invalid_alert_command" || code === "alert_stale";
}

export function submitAlertCommand(
  host: AdminSessionHost,
  pending: PendingCommands,
  notice: Cell<AdminActionFeedback | null>,
  refresh: Cell<bigint>,
  pageAlive: () => boolean,
  command: AlertCommand,
): void {
  const frozenCommand: AlertCommand =
    command.kind === "acknowledge"
      ? Object.freeze({
          kind: "acknowledge" as const,
          alertId: command.alertId,
          incidentGeneration: command.incidentGeneration,
          reason: command.reason,
        })
      : Object.freeze({
          kind: "silence" as const,
          alertId: command.alertId,
          incidentGeneration: command.incidentGeneration,
          reason: command.reason,
          silenceSeconds: command.silenceSeconds,
        });
  const action =
    frozenCommand.kind === "acknowledge"
      ? t("alert-console-acknowledge")
      : t("alert-console-silence");
  // 备注、事件代数与静默时长在点击时冻结；原样重试复用同一份输入。
  let prepare: AdminAlertCommandPreparation;
  if (frozenCommand.kind === "acknowledge") {
    prepare = prepareAdminAlertAcknowledge(frozenCommand.incidentGeneration, frozenCommand.reason);
  } else {
    prepare = prepareAdminAlertSilence(
      frozenCommand.incidentGeneration,
      frozenCommand.silenceSeconds,
      frozenCommand.reason,
    );
  }
  const submitted = pending.submit(
    alertCommandEntity(alertCommandAlertId(frozenCommand)),
    action,
    async (commandId, signal) => {
      const result =
        frozenCommand.kind === "acknowledge"
          ? await acknowledgeAdminAlert(host, frozenCommand.alertId, commandId, prepare, signal)
          : await silenceAdminAlert(host, frozenCommand.alertId, commandId, prepare, signal);
      return result.ok ? ok(null) : result;
    },
    () => setIfAlive(notice, { kind: "success" }, pageAlive),
    (code) => {
      // 这两类明确拒绝说明列表快照已过时（signal 已恢复或状态已变化），且拒绝本身未写入任何
      // 事实：重读列表供操作员判断下一步。结果未知等其他失败不重读，只能原样重试。
      if (alertSnapshotStale(code)) {
        advanceCommandReload(refresh);
      }
      setIfAlive(notice, { kind: "error", code }, pageAlive);
    },
  );
  if (submitted) {
    notice.set(null);
  }
}

/** 与 Rust `str::trim` 的字符集合一致的裁剪。 */
function rustTrim(value: string): string {
  let start = 0;
  let end = value.length;
  while (start < end && isRustWhitespace(value.charCodeAt(start))) {
    start += 1;
  }
  while (end > start && isRustWhitespace(value.charCodeAt(end - 1))) {
    end -= 1;
  }
  return value.slice(start, end);
}

function isRustWhitespace(code: number): boolean {
  return (
    (code >= 0x09 && code <= 0x0d) ||
    code === 0x20 ||
    code === 0x85 ||
    code === 0xa0 ||
    code === 0x1680 ||
    (code >= 0x2000 && code <= 0x200a) ||
    code === 0x2028 ||
    code === 0x2029 ||
    code === 0x202f ||
    code === 0x205f ||
    code === 0x3000
  );
}
