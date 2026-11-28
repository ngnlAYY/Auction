// 可被人工证明引用的证据与冻结发送尝试的选取。
//
// 只有维护流程写入的两类观察可被引用；既有确认与草稿丢弃不构成新证据。

import type {
  AdminDeliveryAttemptResponse as DeliveryAttempt,
  AdminDeliveryDetailResponse as DeliveryDetail,
  AdminDeliveryEvidenceResponse as DeliveryEvidence,
} from "@auction/contracts/dto/admin/delivery";

/** 人工证明可以引用的证据种类。 */
export const SELECTABLE_EVIDENCE_KINDS: readonly string[] = [
  "mailbox_sweep",
  "sent_reference_sighting",
];

/** 详情中可被人工证明引用的证据，保持服务端顺序。 */
export function selectableEvidence(detail: DeliveryDetail): readonly DeliveryEvidence[] {
  return detail.evidence.filter((item) => SELECTABLE_EVIDENCE_KINDS.includes(item.kind));
}

/** 冻结的发送尝试按 fence 取最新一条；缺失或只有 Take 尝试时不得引用。 */
export function latestSendAttempt(detail: DeliveryDetail): DeliveryAttempt | null {
  let latest: DeliveryAttempt | null = null;
  for (const attempt of detail.attempts) {
    if (attempt.operation !== "send") {
      continue;
    }
    // Rust `max_by_key` 在并列最大值时返回最后一个匹配项。
    if (latest === null || attempt.fencing_version >= latest.fencing_version) {
      latest = attempt;
    }
  }
  return latest;
}
