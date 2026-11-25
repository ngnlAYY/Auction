// 投递详情分区共用的展示格式化：空值占位、托管摘要、处置状态与种类文案。

import type {
  AdminDeliveryAttemptResponse as DeliveryAttempt,
  AdminDeliveryDispositionResponse as DeliveryDisposition,
  AdminDeliveryEvidenceResponse as DeliveryEvidence,
} from "@auction/contracts/dto/admin/delivery";
import { t } from "@auction/web-shared/ui/i18n";

import { deliveryStatusLabel } from "../labels";

/** 可空文本的统一空值占位。 */
export function optionalText(value: string | null): string {
  return value ?? t("value-empty");
}

/** 可空整数的统一空值占位；0 保留为实际值。 */
export function optionalInteger(value: number | null): string {
  return value === null ? t("value-empty") : String(value);
}

/** 冻结尝试的 credits 与 COD，按 Rust 的“x / y”格式展示。 */
export function attemptAmounts(attempt: DeliveryAttempt): string {
  const credits = optionalInteger(attempt.credits);
  const cod = optionalInteger(attempt.cod);
  return `${credits} / ${cod}`;
}

/** 尝试的外部邮件 ID 与绑定 Sent 邮件 ID；两者都空时显示统一空值占位。 */
export function attemptMailId(attempt: DeliveryAttempt): string {
  const external = attempt.external_mail_id ?? "";
  const bound = attempt.bound_sent_mail_id ?? "";
  if (external === "" && bound === "") {
    return t("value-empty");
  }
  if (external !== "" && bound === "") {
    return external;
  }
  if (external === "") {
    return `${bound} (${t("delivery-detail-attempt-bound")})`;
  }
  return `${external} (${bound})`;
}

/** 证据的托管三态与剩余 credits/COD，按 Rust 的“状态 · x / y”格式展示。 */
export function evidenceCustody(item: DeliveryEvidence): string {
  let collected: string;
  if (item.collected === true) {
    collected = t("delivery-detail-yes");
  } else if (item.collected === false) {
    collected = t("delivery-detail-no");
  } else {
    collected = t("delivery-detail-custody-unobserved");
  }
  const credits = optionalInteger(item.remaining_credits);
  const cod = optionalInteger(item.cod_pending);
  return `${collected} · ${credits} / ${cod}`;
}

/** 处置种类文案；未知种类原样显示，不猜测语义。 */
export function dispositionKindLabel(kind: string): string {
  switch (kind) {
    case "release":
      return t("delivery-detail-disposition-kind-release");
    case "retry_failed":
      return t("delivery-detail-disposition-kind-retry");
    case "reconcile":
      return t("delivery-detail-disposition-kind-reconcile");
    case "sent_operator_confirmed":
      return t("delivery-detail-disposition-kind-sent-confirmed");
    case "operator_acknowledged":
      return t("delivery-detail-disposition-kind-operator-acknowledged");
    case "auto_sent_proof":
      return t("delivery-detail-disposition-kind-auto-sent-proof");
    case "operator_sent_confirmation":
      return t("delivery-detail-disposition-kind-operator-sent-confirmation");
    case "admin_failed":
      return t("delivery-detail-disposition-kind-admin-failed");
    case "equipment_loss":
      return t("delivery-detail-disposition-kind-equipment-loss");
    case "cod_returned":
      return t("delivery-detail-disposition-kind-cod-returned");
    case "recovery_equipment_recovered":
      return t("delivery-detail-disposition-kind-recovery-equipment-recovered");
    case "legacy_self":
      return t("delivery-detail-disposition-kind-legacy-self");
    case "preflight_retired":
      return t("delivery-detail-disposition-kind-preflight-retired");
    case "season_closed":
      return t("delivery-detail-disposition-kind-season-closed");
    default:
      return kind;
  }
}

/** 处置状态与 fence 版本；版本缺失时只显示状态。 */
export function dispositionStatus(item: DeliveryDisposition): string {
  const status = item.status === null ? t("value-empty") : deliveryStatusLabel(item.status);
  return item.fencing_version === null ? status : `${status} · v${item.fencing_version}`;
}

/** 处置操作者与原因；原因非空时用“ · ”连接。 */
export function dispositionActor(item: DeliveryDisposition): string {
  const actor = item.actor ?? t("value-empty");
  return item.reason !== null && item.reason !== "" ? `${actor} · ${item.reason}` : actor;
}
