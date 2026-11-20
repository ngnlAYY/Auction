// 投递状态机字段的 fail-closed 校验。
// 与 `decode.ts` 共用同一入口与校验顺序；这里只放各终态专用谓词与时间链检查。

import type { AdminDeliveryResponse } from "@auction/contracts/dto/admin/delivery";

import { validTrimmedText } from "../validation";

const ZERO_DELIVERY_TIME = -62_135_596_800n * 1_000_000_000n;
const SUPPRESSION_REASONS: readonly string[] = [
  "mail_disposition_conflict",
  "invalid_payload",
  "task_panic",
  "dependency_mismatch",
  "retry_exhausted",
  "operations_alert_batch_terminal",
];

export function validReturnedMailId(value: string | null): boolean {
  if (value === null) {
    return true;
  }
  if (value.length < 1 || value.length > 32) {
    return false;
  }
  // Rust 用 `bytes().all(is_ascii_digit)`；逐个检查可避免 `$` 接受结尾换行。
  for (let index = 0; index < value.length; index += 1) {
    const code = value.charCodeAt(index);
    if (code < 0x30 || code > 0x39) {
      return false;
    }
  }
  return true;
}

function preparationRejected(value: AdminDeliveryResponse): boolean {
  return (
    value.status === "failed" &&
    (value.failure_code === "invalid_payload" || value.failure_code === "task_panic") &&
    value.sending_at === null
  );
}

export function validSuppressionProjection(value: AdminDeliveryResponse): boolean {
  const reason = value.suppression_reason;
  if (reason === null) {
    return true;
  }
  // 存储投影只在围栏前失败时派生抑制原因，闭集内的失败码优先于原因码。
  const projected =
    value.failure_code !== null && SUPPRESSION_REASONS.includes(value.failure_code)
      ? value.failure_code
      : value.reason_code !== null && SUPPRESSION_REASONS.includes(value.reason_code)
        ? value.reason_code
        : null;
  return (
    value.status === "failed" &&
    value.sending_at === null &&
    value.retirement_reason === null &&
    projected === reason
  );
}

function recoverySuperseded(value: AdminDeliveryResponse): boolean {
  return value.status === "failed" && value.failure_code === "recovery_superseded";
}

function recoveryQuarantine(value: AdminDeliveryResponse): boolean {
  return (
    value.status === "manual_reconcile" &&
    (value.failure_code === "external_recovery_sent_match" ||
      value.failure_code === "external_recovery_payload_match" ||
      value.failure_code === "external_recovery_unresolved_send")
  );
}

/** 返回已知退役原因的来源和失败码是否相符；已知原因即使组合无效也须进入退役校验。 */
export function preflightRetirementReasonValid(value: AdminDeliveryResponse): boolean | null {
  const reason = value.reason_code;
  if (reason === null) {
    return null;
  }
  let originAllowed: boolean;
  let expectedFailure: string;
  switch (reason) {
    case "lot_watch_invalidated":
      originAllowed = value.origin_kind === "lot_watch";
      expectedFailure = reason;
      break;
    case "recipient_renamed":
      originAllowed =
        value.origin_kind === "lot_watch" ||
        value.origin_kind === "seller_summary" ||
        value.origin_kind === "season_reminder";
      expectedFailure = reason;
      break;
    case "season_boundary_passed":
    case "season_reminder_resolved":
      originAllowed = value.origin_kind === "season_reminder";
      expectedFailure = "season_notice_invalidated";
      break;
    case "auction_opened":
      originAllowed = value.origin_kind === "mail_intake";
      expectedFailure = "auction_notice_invalidated";
      break;
    default:
      return null;
  }
  return originAllowed && value.failure_code === expectedFailure;
}

export function isZeroDeliveryTime(value: bigint): boolean {
  return value === ZERO_DELIVERY_TIME;
}

function validTimeline(
  createdAt: bigint,
  claimedAt: bigint | null,
  sendingAt: bigint | null,
  completedAt: bigint | null,
): boolean {
  return (
    (claimedAt === null || claimedAt >= createdAt) &&
    (sendingAt === null || (claimedAt !== null && sendingAt >= claimedAt)) &&
    (completedAt === null || (sendingAt !== null && completedAt >= sendingAt))
  );
}

export function validStateFields(
  value: AdminDeliveryResponse,
  createdAt: bigint,
  updatedAt: bigint,
  claimedAt: bigint | null,
  sendingAt: bigint | null,
  completedAt: bigint | null,
  retirementReasonValid: boolean | null,
): boolean {
  const hasLease = value.lease_expires_at !== null;
  const validBeforeSend = (): boolean =>
    !hasLease &&
    sendingAt === null &&
    value.external_reference === null &&
    !value.take_required &&
    value.returned_mail_id === null &&
    (claimedAt === null || (!isZeroDeliveryTime(claimedAt) && claimedAt >= createdAt)) &&
    completedAt !== null &&
    !isZeroDeliveryTime(completedAt) &&
    completedAt >= createdAt &&
    (claimedAt === null || completedAt >= claimedAt) &&
    completedAt <= updatedAt;
  const claimedHasAttempt = claimedAt === null || value.attempt_count > 0;

  // 专用分支按存储层 valid_snapshot 的顺序选定，匹配后一起校验时间链和状态。
  // 准备失败须保留领取与完成事实，并优先校验这一终态以拒绝相冲突的原因码。
  if (preparationRejected(value)) {
    return (
      value.reason_code === null &&
      claimedAt !== null &&
      value.fencing_version > 0 &&
      value.attempt_count > 0 &&
      validBeforeSend()
    );
  }
  // 资产处置冲突只能在发送前隔离；人工失败后仍保留原因，不能混入普通可重试失败。
  if (value.reason_code === "mail_disposition_conflict") {
    return (
      value.origin_kind === "mail_intake" &&
      (value.status === "manual_reconcile"
        ? value.failure_code === "mail_disposition_conflict"
        : value.status === "failed" &&
          value.failure_code !== null &&
          validTrimmedText(value.failure_code)) &&
      value.fencing_version > 0 &&
      claimedHasAttempt &&
      validBeforeSend()
    );
  }
  // 退役须有确定的未发送事实，围栏版本推进后才能让浏览器接受该终态。
  if (retirementReasonValid !== null) {
    return (
      retirementReasonValid &&
      value.status === "failed" &&
      value.cod === 0 &&
      value.suppression_reason === null &&
      value.fencing_version > 0 &&
      claimedHasAttempt &&
      validBeforeSend()
    );
  }
  // 未激活的告警从创建直接到完成，未获得发送围栏。
  if (
    value.status === "failed" &&
    value.origin_kind === "operations_alert" &&
    value.reason_code === "operations_alert_batch_terminal"
  ) {
    return (
      claimedAt === null &&
      (value.failure_code === "dependency_mismatch" || value.failure_code === "retry_exhausted") &&
      value.fencing_version === 0 &&
      value.attempt_count === 0 &&
      validBeforeSend()
    );
  }
  // 恢复隔离或终结可从本地直接完成，领取时间本身不能作为发送尝试的证据。
  if (recoveryQuarantine(value)) {
    return (
      value.fencing_version > 0 &&
      value.retirement_reason === null &&
      value.suppression_reason === null &&
      validBeforeSend()
    );
  }
  if (recoverySuperseded(value)) {
    return (
      value.origin_kind === "settlement" &&
      value.reason_code === null &&
      value.fencing_version > 0 &&
      validBeforeSend()
    );
  }
  if (!validTimeline(createdAt, claimedAt, sendingAt, completedAt)) {
    return false;
  }
  switch (value.status) {
    case "prepared":
    case "pending":
      return (
        !hasLease &&
        claimedAt === null &&
        sendingAt === null &&
        completedAt === null &&
        value.external_reference === null &&
        value.failure_code === null
      );
    case "claimed":
      return (
        hasLease &&
        claimedAt !== null &&
        sendingAt === null &&
        completedAt === null &&
        value.external_reference === null &&
        value.failure_code === null &&
        value.fencing_version > 0 &&
        value.attempt_count > 0
      );
    case "sending":
      return (
        hasLease &&
        claimedAt !== null &&
        sendingAt !== null &&
        completedAt === null &&
        value.external_reference === null &&
        value.failure_code === null &&
        value.fencing_version > 0 &&
        value.attempt_count > 0
      );
    case "succeeded":
      return (
        !hasLease &&
        claimedAt !== null &&
        sendingAt !== null &&
        completedAt !== null &&
        value.external_reference !== null &&
        validTrimmedText(value.external_reference) &&
        value.failure_code === null &&
        value.fencing_version > 0 &&
        value.attempt_count > 0
      );
    case "failed":
    case "manual_reconcile": {
      // 已证明收件人不存在的退信 MID 绑定到对应世界的回收邮箱。
      const returnedToSender =
        value.status === "manual_reconcile" &&
        value.failure_code === "recipient_not_found" &&
        value.reason_code === "recipient_not_found" &&
        value.world !== null &&
        value.cod !== null &&
        value.take_required &&
        value.returned_mail_id !== null &&
        value.external_reference === value.returned_mail_id &&
        validReturnedMailId(value.returned_mail_id);
      return (
        !hasLease &&
        claimedAt !== null &&
        sendingAt !== null &&
        completedAt !== null &&
        (value.external_reference === null || returnedToSender) &&
        value.failure_code !== null &&
        validTrimmedText(value.failure_code) &&
        value.fencing_version > 0 &&
        value.attempt_count > 0
      );
    }
    default:
      return false;
  }
}
