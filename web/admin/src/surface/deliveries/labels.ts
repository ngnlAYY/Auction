// Delivery 状态和来源辅助。

import type { AdminDeliveryActionBlocker } from "@auction/contracts/dto/admin/delivery";
import { unreachable } from "@auction/web-shared/result";
import { utf8ByteLength } from "@auction/web-shared/text";
import { t } from "@auction/web-shared/ui/i18n";

import { MAX_DELIVERY_REASON_BYTES, validDeliveryReason } from "../../api/delivery";
import type { Delivery } from "../../api/delivery/decode";
import { trimRustWhitespace } from "../../api/session-time";
import { type AdminActionFeedback, adminActionFeedbackMessage } from "../presentation";

/** 服务端准入阻断原因的本地化文案。 */
export function actionBlockerLabel(reason: AdminDeliveryActionBlocker): string {
  switch (reason) {
    case "settlement_return_action_owned":
      return t("delivery-action-blocked-return-owned");
    case "settlement_ineligible":
      return t("delivery-action-blocked-settlement");
    case "funded_delivery":
      return t("delivery-action-blocked-funded");
    case "retired_delivery":
      return t("delivery-action-blocked-retired");
    case "stale_fencing_version":
      return t("delivery-action-blocked-stale");
    case "sent_sighted":
      return t("delivery-action-blocked-sent-sighted");
    case "reconcile_succeeded_for_send":
      return t("delivery-reconcile-send-proof-required");
    case "delivery_not_eligible":
      return t("delivery-action-blocked-ineligible");
    case "status_mismatch":
      return t("delivery-action-blocked-status");
    case "quarantined_mail_action_not_allowed":
      return t("delivery-mail-disposition-conflict");
    case "missing_reconcile_reference":
      return t("delivery-action-blocked-reference");
    case "recipient_renamed":
      return t("delivery-action-blocked-recipient-renamed");
    case "already_disposed":
      return t("delivery-action-blocked-disposed");
    case "not_acknowledgeable":
      return t("delivery-action-blocked-acknowledge");
    case "recovery_retry_ineligible":
      return t("delivery-action-blocked-recovery-retry");
    case "settlement_fulfillment_state":
      return t("delivery-action-blocked-fulfillment-state");
    case "missing_sending_fence":
      return t("delivery-action-blocked-missing-sending-fence");
    case "delivery_command_snapshot_unreadable":
      return t("delivery-action-blocked-command-snapshot-unreadable");
    default:
      return unreachable(reason);
  }
}

/** 两个投递专属错误码改用阻断原因文案，其余反馈沿用通用消息。 */
export function actionFeedbackMessage(feedback: AdminActionFeedback, success: string): string {
  if (feedback.kind === "error") {
    if (feedback.code === "delivery_recipient_renamed") {
      return actionBlockerLabel("recipient_renamed");
    }
    if (feedback.code === "delivery_command_snapshot_unreadable") {
      return actionBlockerLabel("delivery_command_snapshot_unreadable");
    }
  }
  return adminActionFeedbackMessage(feedback, success);
}

/** 把接口校验拒绝的理由草稿映射为字段提示。 */
export function deliveryReasonError(reason: string): string | null {
  const trimmed = trimRustWhitespace(reason);
  if (validDeliveryReason(trimmed)) {
    return null;
  }
  if (trimmed === "") {
    return t("admin-field-required");
  }
  if (utf8ByteLength(trimmed) > MAX_DELIVERY_REASON_BYTES) {
    return t("admin-field-max-utf8-bytes", { max: MAX_DELIVERY_REASON_BYTES });
  }
  return t("admin-field-no-control-characters");
}

export const DELIVERY_STATUS_VALUES: readonly string[] = [
  "prepared",
  "pending",
  "claimed",
  "sending",
  "succeeded",
  "failed",
  "manual_reconcile",
];

/** 投递状态标签；未知状态返回原文。 */
export function deliveryStatusLabel(status: string): string {
  switch (status) {
    case "prepared":
      return t("delivery-status-prepared");
    case "pending":
      return t("delivery-status-pending");
    case "claimed":
      return t("delivery-status-claimed");
    case "sending":
      return t("delivery-status-sending");
    case "succeeded":
      return t("delivery-status-succeeded");
    case "failed":
      return t("delivery-status-failed");
    case "manual_reconcile":
      return t("delivery-status-manual");
    default:
      return status;
  }
}

/** 恢复旧 Admin 表格的短 ID：优先展示来源末尾的数字，否则回退 delivery UUID 前八位。 */
export function deliveryDisplayId(delivery: Delivery): string {
  const originId = delivery.origin_id;
  let numericSuffix: string | null = null;
  if (originId !== "" && asciiDigits(originId)) {
    numericSuffix = originId;
  } else {
    const separator = Math.max(originId.lastIndexOf("-"), originId.lastIndexOf("_"));
    if (separator >= 0) {
      const suffix = originId.slice(separator + 1);
      if (suffix !== "" && asciiDigits(suffix)) {
        numericSuffix = suffix;
      }
    }
  }
  return numericSuffix ?? Array.from(delivery.id).slice(0, 8).join("");
}

function asciiDigits(value: string): boolean {
  for (let index = 0; index < value.length; index += 1) {
    const code = value.charCodeAt(index);
    if (code < 0x30 || code > 0x39) {
      return false;
    }
  }
  return true;
}

/** 旧页面把 settlement 显示为 equipment；新版合法扩展使用五语言安全标签，避免泄漏内部 token。 */
export function deliveryDisplayType(originKind: string): string {
  switch (originKind) {
    case "settlement":
      return "equipment";
    case "settlement_recovery_action":
      return t("delivery-type-settlement-recovery");
    case "settlement_return_action":
      return t("delivery-type-settlement-return");
    case "operations_alert":
      return t("delivery-type-operations-alert");
    case "lot_watch":
      return t("delivery-type-lot-watch");
    case "seller_summary":
      return t("delivery-type-seller-summary");
    case "season_reminder":
      return t("delivery-type-season-reminder");
    default:
      return originKind;
  }
}

/** 发送类别标签；未知类别返回原文，闭集由服务端保证。 */
export function deliveryCategoryLabel(category: string): string {
  switch (category) {
    case "outbid":
      return t("delivery-category-outbid");
    case "account":
      return t("delivery-category-account");
    case "intake":
      return t("delivery-category-intake");
    case "settlement":
      return t("delivery-category-settlement");
    case "equipment":
      return t("delivery-category-equipment");
    case "operations":
      return t("delivery-category-operations");
    case "manual":
      return t("delivery-category-manual");
    default:
      return category;
  }
}

/** 行内终态说明：退役/抑制原因优先于普通失败码，二者都只在确定未写入时出现。 */
export function deliveryOutcomeLabel(delivery: Delivery): string {
  const retirementReason = delivery.retirement_reason;
  if (retirementReason !== null) {
    return t("delivery-outcome-retired", { reason: retirementReason });
  }
  const suppressionReason = delivery.suppression_reason;
  if (suppressionReason !== null) {
    return t("delivery-outcome-suppressed", { reason: suppressionReason });
  }
  return delivery.failure_code ?? "";
}
