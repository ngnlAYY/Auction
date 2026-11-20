// 投递对话框的 fence、动作准入与对账选项判定，以及证据和失败码的展示文案。

import type { AdminDeliveryResponse as Delivery } from "@auction/contracts/dto/admin/delivery";
import {
  type AdminDeliveryActionAvailability,
  type AdminDeliveryActionBlocker,
  type AdminDeliveryActionsResponse,
  adminDeliveryActionAvailabilityBlocker,
} from "@auction/contracts/dto/admin/delivery";
import { unreachable } from "@auction/web-shared/result";
import { t } from "@auction/web-shared/ui/i18n";

import type { DeliveryAction } from "../../api/delivery/queue";
import { formatAdminTime } from "../presentation";

/** 打开弹窗时的投递与当前详情是否仍是同一 fence；投递 ID 或版本变化后不得提交。 */
export function sameFence(expected: Delivery, current: Delivery): boolean {
  return expected.id === current.id && expected.fencing_version === current.fencing_version;
}

/** 把界面动作映射到服务端返回的准入判定；判定字段与动作一一对应。 */
export function availability(
  actions: AdminDeliveryActionsResponse,
  action: DeliveryAction,
): AdminDeliveryActionAvailability {
  switch (action) {
    case "release":
      return actions.release;
    case "retry":
      return actions.retry_failed;
    case "reconcile_retry":
      return actions.reconcile_retry;
    case "reconcile_succeeded":
      return actions.reconcile_succeeded;
    case "reconcile_failed":
      return actions.reconcile_failed;
    default:
      return unreachable(action);
  }
}

/** 人工对账下拉的有效选择；未识别或服务端未准入的选择返回 null。 */
export function selectedReconcileAction(
  selection: string,
  actions: AdminDeliveryActionsResponse,
): DeliveryAction | null {
  let action: DeliveryAction;
  switch (selection) {
    case "retry":
      action = "reconcile_retry";
      break;
    case "succeeded":
      action = "reconcile_succeeded";
      break;
    case "failed":
      action = "reconcile_failed";
      break;
    default:
      return null;
  }
  return adminDeliveryActionAvailabilityBlocker(availability(actions, action)) === null
    ? action
    : null;
}

/** 人工对账下拉的一项：原始选择值、界面文案与服务端准入。 */
export interface ReconcileOption {
  readonly value: string;
  readonly label: string;
  readonly decision: AdminDeliveryActionAvailability;
}

/** 对账选项被阻断的原因；服务端准入时返回 null。 */
export function reconcileOptionBlocker(option: ReconcileOption): AdminDeliveryActionBlocker | null {
  return adminDeliveryActionAvailabilityBlocker(option.decision);
}

/** Sent 观察时间；缺失时显示统一空值占位。 */
export function observedLabel(value: string | null): string {
  return value === null ? t("value-empty") : formatAdminTime(value);
}

/** Sent 证据的托管状态；null 表示未观察到，不等同于“未收取”。 */
export function collectedLabel(value: boolean | null): string {
  if (value === true) {
    return t("delivery-detail-yes");
  }
  if (value === false) {
    return t("delivery-detail-no");
  }
  return t("delivery-detail-custody-unobserved");
}

/** 闭集失败原因码的界面译文；未知码原样显示，不猜测语义。 */
export function failureCodeLabel(code: string): string {
  switch (code) {
    case "verified_not_sent":
      return t("delivery-failure-code-verified-not-sent");
    case "recipient_invalid":
      return t("delivery-failure-code-recipient-invalid");
    case "no_longer_needed":
      return t("delivery-failure-code-no-longer-needed");
    default:
      return code;
  }
}
