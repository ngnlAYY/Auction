// Admin 结算与 recovery 的标签、金额和分页辅助。

import { formatCreditAmount } from "@auction/web-shared/ui/format";
import { t } from "@auction/web-shared/ui/i18n";

import type { AdminRecoveryAction } from "../../api/recovery";
import type { AdminSettlementLot } from "../../api/settlement";

const I64_MIN = -9_223_372_036_854_775_808n;
const I64_MAX = 9_223_372_036_854_775_807n;
const MAX_SAFE_INTEGER = BigInt(Number.MAX_SAFE_INTEGER);

/** 履约阶段标签；未知阶段原样显示。 */
export function recoveryStageLabel(stage: string): string {
  switch (stage) {
    case "awaiting_payment":
      return t("admin-recovery-stage-awaiting-payment");
    case "payment_overdue":
      return t("admin-recovery-stage-payment-overdue");
    case "delivering_equipment":
      return t("admin-recovery-stage-delivering");
    case "reclaiming":
      return t("admin-recovery-stage-reclaiming");
    case "relist_pending":
      return t("admin-recovery-stage-relist-pending");
    case "refunding":
      return t("admin-recovery-stage-refunding");
    case "completed":
      return t("admin-recovery-stage-completed");
    default:
      return stage;
  }
}

/** 把恢复操作列表拼成“操作: 状态”，无操作时显示空值文案。 */
export function recoveryActionSummary(actions: readonly AdminRecoveryAction[]): string {
  if (actions.length === 0) {
    return t("value-empty");
  }
  return actions
    .map(
      (action) =>
        `${recoveryActionKindLabel(action.kind)}: ${recoveryActionStateLabel(action.state)}`,
    )
    .join(" · ");
}

/** 金额调整文案：正调整显示罚金，负调整显示最终价与标价的差额。 */
export function priceAdjustmentLabel(lot: AdminSettlementLot): string {
  if (lot.adjustment_ppm > 0) {
    return t("admin-settlement-penalty-value", {
      amount: formatCreditAmount(lot.tax_amount),
    });
  }
  if (lot.adjustment_ppm < 0) {
    const discount = lot.final_price - lot.listed_price;
    return t("admin-settlement-discount-value", {
      amount: formatCreditAmount(discount),
    });
  }
  return t("value-empty");
}

/** 恢复操作类型标签；未知值原样显示。 */
export function recoveryActionKindLabel(kind: string): string {
  switch (kind) {
    case "payment_reminder":
      return t("admin-recovery-action-payment-reminder");
    case "reclaim_equipment_take":
      return t("admin-recovery-action-reclaim");
    case "cancel_carrier_take":
      return t("admin-recovery-action-cancel-carrier");
    case "recover_equipment_take":
      return t("admin-recovery-action-recover-equipment");
    case "buyer_blacklist":
      return t("admin-recovery-action-buyer-blacklist");
    default:
      return kind;
  }
}

/** 恢复操作状态标签；未知值原样显示。 */
export function recoveryActionStateLabel(state: string): string {
  switch (state) {
    case "pending":
      return t("admin-recovery-state-pending");
    case "prepared":
      return t("admin-recovery-state-prepared");
    case "enqueued":
      return t("admin-recovery-state-enqueued");
    case "satisfied":
      return t("admin-recovery-state-satisfied");
    case "failed":
      return t("admin-recovery-state-failed");
    case "manual_reconcile":
      return t("admin-recovery-state-manual");
    default:
      return state;
  }
}

/** 发送归属不成立的具体原因说明；未知值原样显示（报告校验已按闭集拒绝）。 */
export function sendAttributionBlockerLabel(blocker: string): string {
  switch (blocker) {
    case "return_not_current":
      return t("admin-recovery-attribution-return-not-current");
    case "send_in_flight":
      return t("admin-recovery-attribution-send-in-flight");
    case "sends_exceed_returns":
      return t("admin-recovery-attribution-sends-exceed-returns");
    case "send_without_sent_mid":
      return t("admin-recovery-attribution-send-without-sent-mid");
    case "send_records_incomplete":
      return t("admin-recovery-attribution-send-records-incomplete");
    case "send_after_return":
      return t("admin-recovery-attribution-send-after-return");
    default:
      return blocker;
  }
}

/** 恢复命令标签；未知命令原样显示。 */
export function recoveryCommandLabel(action: string): string {
  switch (action) {
    case "reclaimed":
      return t("admin-recovery-command-reclaimed");
    case "cancelled":
      return t("admin-recovery-command-cancelled");
    case "equipment_recovered":
      return t("admin-recovery-command-equipment-recovered");
    case "returned_after_three_cod_returns":
      return t("admin-recovery-command-returned-after-three-cod-returns");
    default:
      return action;
  }
}

/** 只对完整权威报告切片；先做有界整数转换，不按用户数量分配内存。 */
export function pageCount(length: number, size: number): number {
  const boundedLength = clampI64(length);
  const exactLength = boundedLength < 0n ? 0n : boundedLength;
  const boundedSize = clampI64(size);
  const divisor = boundedSize < 1n ? 1n : boundedSize;
  const pages = exactLength === 0n ? 1n : 1n + (exactLength - 1n) / divisor;
  return pages > MAX_SAFE_INTEGER ? Number.MAX_SAFE_INTEGER : Number(pages);
}

/** 按页号与页大小切出报告条目；把页码与大小钳制到源码的 `i64`/`usize` 范围。 */
export function pageSlice<T>(items: readonly T[], page: number, size: number): T[] {
  const boundedPage = clampI64(page);
  const boundedSize = clampI64(size);
  const firstPage = boundedPage < 1n ? 1n : boundedPage;
  const pageSize = boundedSize < 1n ? 1n : boundedSize;
  const rawOffset = (firstPage - 1n) * pageSize;
  const offset = rawOffset > I64_MAX ? I64_MAX : rawOffset;
  if (offset > MAX_SAFE_INTEGER) {
    return [];
  }
  const start = Number(offset);
  const take = pageSize > MAX_SAFE_INTEGER ? Number.MAX_SAFE_INTEGER : Number(pageSize);
  return items.slice(start, start + take);
}

function clampI64(value: number): bigint {
  if (Number.isNaN(value)) {
    return 0n;
  }
  const truncated = Math.trunc(value);
  if (truncated === Number.POSITIVE_INFINITY) {
    return I64_MAX;
  }
  if (truncated === Number.NEGATIVE_INFINITY) {
    return I64_MIN;
  }
  const exact = BigInt(truncated);
  if (exact > I64_MAX) {
    return I64_MAX;
  }
  if (exact < I64_MIN) {
    return I64_MIN;
  }
  return exact;
}
