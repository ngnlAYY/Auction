// 竞价命令的冻结负载、目标与结果分类。

import { unreachable } from "@auction/web-shared/result";
import { t } from "@auction/web-shared/ui/i18n";
import {
  type CommandError,
  commandErrorOutcomeUncertain,
  commandNotSent,
  createProxyAuthorization,
  lowerProxyAuthorization,
  placeBid,
} from "../../api/commands";
import type { PendingBidCommand, PendingProxyCommand } from "./pending";

/** 命令发送时的目录位置与拍品展示名。 */
export interface CommandTarget {
  readonly auctionId: string;
  readonly world: string;
  readonly lotId: string;
  readonly lotCode: string;
  readonly lotName: string;
}

/** 完整的写入负载，包括降低上限时的授权路径。 */
export type CommandPayload = (PendingBidCommand & { readonly kind: "bid" }) | PendingProxyCommand;

/** 已交接的首次提交，或可原样恢复的未知结果。 */
export type CommandStatus =
  | { readonly kind: "submitting" }
  | { readonly kind: "unknown"; readonly retrying: boolean; readonly error: CommandError };

/** 同一身份只允许重放这一份目标和负载。 */
export interface SentCommand {
  readonly target: CommandTarget;
  readonly payload: CommandPayload;
  readonly sentAt: number | null;
  readonly status: CommandStatus;
}

/** 只用于回显的回执，竞价状态另从权威读取加载。 */
export interface CommandReceipt {
  readonly commandId: string;
  readonly target: CommandTarget;
  readonly kind: CommandPayload["kind"];
  readonly error: CommandError | null;
}

/** 两次尝试用相同的冻结字段与串行化入口生成同一 URL、用户头和正文字节。 */
export function sendCommand({ target, payload }: SentCommand, signal: AbortSignal) {
  const { userId, amount, commandId } = payload;
  switch (payload.kind) {
    case "bid":
      return placeBid(userId, target.lotId, amount, commandId, signal);
    case "create":
      return createProxyAuthorization(
        userId,
        target.lotId,
        amount,
        payload.mode,
        commandId,
        signal,
      );
    case "lower":
      return lowerProxyAuthorization(
        userId,
        target.lotId,
        payload.authorizationId,
        amount,
        commandId,
        signal,
      );
    default:
      return unreachable(payload);
  }
}

/** 回放冲突无法区分账本摘要不符与已回滚的唯一约束冲突。 */
export function replayConflict(error: CommandError): boolean {
  return error.status === 409 && error.code === "command_conflict";
}

const REPLAY_REJECTIONS = new Set([
  "bid_too_low",
  "already_leading",
  "active_proxy_exists",
  "auction_not_open",
  "proxy_authorization_not_active",
  "proxy_maximum_not_lower",
  "proxy_maximum_below_binding",
  "proxy_reentry_maximum_not_higher",
  "proxy_disabled",
  "proxy_outbid_limit",
  "proxy_slow_final",
  "invalid_policy",
]);

/** 重试的前置拒绝不作原结果的证据，结案只接受账本查询后的明确业务拒绝。 */
export function outcomeUnknown(error: CommandError, retry: boolean): boolean {
  if (commandNotSent(error) || replayConflict(error)) {
    return retry;
  }
  return (
    commandErrorOutcomeUncertain(error) ||
    error.status < 400 ||
    error.status >= 500 ||
    (retry &&
      !(error.status === 403 && error.code === "bidder_ineligible") &&
      !(error.status === 409 && REPLAY_REJECTIONS.has(error.code)))
  );
}

/** 提交期间锁住同拍品；重试期间仍归未知结果。 */
export function commandBusy(command: SentCommand): boolean {
  return command.status.kind === "submitting" || command.status.retrying;
}

/** 目录行与抽屉共用未结命令的说明。 */
export function commandEntryMessage(command: SentCommand): string {
  return t(command.status.kind === "unknown" ? "command-lot-unresolved" : "bid-working");
}

/** 冻结命令类型的展示名。 */
export function commandKindLabel(kind: CommandPayload["kind"]): string {
  switch (kind) {
    case "bid":
      return t("bid-tab-direct");
    case "create":
      return t("command-create-proxy");
    case "lower":
      return t("bid-type-proxy-lower");
    default:
      return unreachable(kind);
  }
}
