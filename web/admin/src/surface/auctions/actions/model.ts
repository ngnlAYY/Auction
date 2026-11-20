// Admin 拍卖生命周期动作的闭集模型与文案映射。

import { err, ok, type Result, unreachable } from "@auction/web-shared/result";
import { utf8ByteLength } from "@auction/web-shared/text";
import { t } from "@auction/web-shared/ui/i18n";

import type { AdminAuctionCapabilities } from "../../../api/auctions";
import type { AdminAuctionLifecycleAction } from "../../../api/lifecycle";
import { scheduledTimeMillis } from "../presentation";

/** 拍卖列表顶部一次只显示一个成功动作，避免各行刷新后残留局部提示。 */
export type AdminAuctionNotice =
  | { readonly kind: "lifecycle"; readonly action: AdminAuctionLifecycleAction }
  | { readonly kind: "schedule" }
  | { readonly kind: "created" };

/** 按状态与能力列出可用的生命周期动作；预览场次只有在开拍时刻到达后才出现“开拍”。 */
export function lifecycleActions(
  status: string,
  capabilities: AdminAuctionCapabilities,
  scheduledOpenAt: string,
  currentTimeMillis: number,
): readonly AdminAuctionLifecycleAction[] {
  const actions: AdminAuctionLifecycleAction[] = [];
  switch (status) {
    case "draft":
      if (capabilities.manual_preview_open) {
        actions.push("preview");
      }
      if (capabilities.cancel_auction) {
        actions.push("cancel");
      }
      break;
    case "preview": {
      const scheduled = scheduledTimeMillis(scheduledOpenAt);
      if (
        capabilities.manual_early_open ||
        (scheduled !== null && scheduled <= currentTimeMillis)
      ) {
        actions.push("open");
      }
      if (capabilities.cancel_auction) {
        actions.push("cancel");
      }
      break;
    }
    case "live":
      actions.push("final-phase");
      if (capabilities.manual_early_settlement) {
        actions.push("early-settlement");
      }
      break;
    case "final":
      if (capabilities.manual_early_settlement) {
        actions.push("early-settlement");
      }
      break;
    default:
      break;
  }
  return actions;
}

/** 预览场次未开启提前开拍、且开拍时间尚未到达时返回开拍时刻。只有这类行的就绪标记与
 * “开拍”入口会随时间变化，因而只有它们挂载秒级时钟；其余状态的输出与时间无关。 */
export function pendingOpenMillis(
  status: string,
  manualEarlyOpen: boolean,
  scheduledOpenAt: string,
  currentTimeMillis: number,
): number | null {
  if (status !== "preview" || manualEarlyOpen) {
    return null;
  }
  const scheduled = scheduledTimeMillis(scheduledOpenAt);
  return scheduled !== null && scheduled > currentTimeMillis ? scheduled : null;
}

/** 动作按钮的文案。 */
export function lifecycleActionLabel(action: AdminAuctionLifecycleAction): string {
  switch (action) {
    case "preview":
      return t("admin-auctions-action-preview");
    case "open":
      return t("admin-auctions-action-open");
    case "cancel":
      return t("admin-auctions-action-cancel");
    case "final-phase":
      return t("admin-auctions-action-final");
    case "early-settlement":
      return t("admin-auctions-action-early-settlement");
    default:
      return unreachable(action);
  }
}

/** 动作按钮的样式类：开拍类动作用主按钮，取消与结算类用危险按钮。 */
export function lifecycleActionButtonClass(action: AdminAuctionLifecycleAction): string {
  switch (action) {
    case "preview":
    case "open":
      return "admin-btn-primary";
    case "cancel":
    case "final-phase":
    case "early-settlement":
      return "admin-btn-danger";
    default:
      return unreachable(action);
  }
}

/** 确认对话框里提交按钮的文案。 */
export function lifecycleActionSubmitLabel(action: AdminAuctionLifecycleAction): string {
  switch (action) {
    case "final-phase":
      return t("admin-auctions-action-confirm");
    case "cancel":
      return t("admin-auctions-action-cancel-confirm");
    case "early-settlement":
      return t("admin-auctions-action-early-settlement-confirm");
    case "preview":
    case "open":
      return lifecycleActionLabel(action);
    default:
      return unreachable(action);
  }
}

/** 确认对话框标题：场次序号与世界。 */
export function lifecycleActionConfirmTitle(
  action: AdminAuctionLifecycleAction,
  auctionSequence: number,
  world: string,
): string {
  switch (action) {
    case "preview":
      return t("admin-auctions-confirm-preview", {
        number: auctionSequence,
        world,
      });
    case "open":
      return t("admin-auctions-confirm-open", {
        number: auctionSequence,
        world,
      });
    case "cancel":
      return t("admin-auctions-confirm-cancel", {
        number: auctionSequence,
        world,
      });
    case "final-phase":
      return t("admin-auctions-confirm-final", {
        number: auctionSequence,
        world,
      });
    case "early-settlement":
      return t("admin-auctions-confirm-early-settlement", {
        number: auctionSequence,
        world,
      });
    default:
      return unreachable(action);
  }
}

/** 确认对话框的说明文案。 */
export function lifecycleActionConfirmNote(action: AdminAuctionLifecycleAction): string {
  switch (action) {
    case "preview":
      return t("admin-auctions-confirm-preview-note");
    case "open":
      return t("admin-auctions-confirm-open-note");
    case "cancel":
      return t("admin-auctions-confirm-cancel-note");
    case "final-phase":
      return t("admin-auctions-confirm-final-note");
    case "early-settlement":
      return t("admin-auctions-confirm-early-settlement-note");
    default:
      return unreachable(action);
  }
}

/** 写入准备阶段的同名错误不能作为已拒绝的版本冲突回执。 */
export function staleAuctionFailure(code: string): boolean {
  return code === "stale_auction_state";
}

// 镜像 Admin 生命周期 HTTP 入口的原因约束，长度按 UTF-8 字节计算。
const MAX_LIFECYCLE_REASON_BYTES = 1_024;

/** 生命周期原因校验失败的原因，界面文案由 `lifecycleReasonIssueMessage` 映射。 */
export type LifecycleReasonIssue = "required" | "too_long" | "control_character";

/** 校验原因对应的界面文案。 */
export function lifecycleReasonIssueMessage(issue: LifecycleReasonIssue): string {
  switch (issue) {
    case "required":
      return t("admin-field-required");
    case "too_long":
      return t("admin-field-max-utf8-bytes", {
        max: MAX_LIFECYCLE_REASON_BYTES,
      });
    case "control_character":
      return t("admin-field-no-control-characters");
    default:
      return unreachable(issue);
  }
}

/** 校验并返回去掉首尾空白的生命周期原因；长度按 UTF-8 字节，控制字符一律拒绝。 */
export function validateLifecycleReason(reason: string): Result<string, LifecycleReasonIssue> {
  const trimmed = rustTrim(reason);
  if (trimmed.length === 0) {
    return err("required");
  }
  if (utf8ByteLength(reason) > MAX_LIFECYCLE_REASON_BYTES) {
    return err("too_long");
  }
  if (hasControlCharacter(reason)) {
    return err("control_character");
  }
  return ok(trimmed);
}

/** 动作成功后的顶部提示文案。 */
export function lifecycleActionSuccessLabel(action: AdminAuctionLifecycleAction): string {
  switch (action) {
    case "preview":
      return t("admin-auctions-action-preview-success");
    case "open":
      return t("admin-auctions-action-open-success");
    case "cancel":
      return t("admin-auctions-action-cancel-success");
    case "final-phase":
      return t("admin-auctions-action-final-success");
    case "early-settlement":
      return t("admin-auctions-action-early-settlement-success");
    default:
      return unreachable(action);
  }
}

/** 列表顶部唯一成功提示的文案。 */
export function auctionNoticeLabel(notice: AdminAuctionNotice): string {
  switch (notice.kind) {
    case "lifecycle":
      return lifecycleActionSuccessLabel(notice.action);
    case "schedule":
      return t("admin-auctions-schedule-success");
    case "created":
      return t("admin-auctions-create-success");
    default:
      return unreachable(notice);
  }
}

/** Rust `char::is_control` 对应 Unicode Cc；字符串里的换行与 NEL 也属于该类别。 */
function hasControlCharacter(value: string): boolean {
  return /\p{Cc}/u.test(value);
}

/** Rust `str::trim` 的空白集合；JS 的 `trim` 与之不同，不能替换。 */
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
    code === 0x09 ||
    code === 0x0a ||
    code === 0x0b ||
    code === 0x0c ||
    code === 0x0d ||
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
