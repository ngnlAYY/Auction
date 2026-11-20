// Sent 证明单元测试的共享输入；构造与 Rust `sent_proof/tests.rs` 的辅助函数逐项对应。

import type {
  AdminDeliveryDetailResponse,
  AdminDeliveryEvidenceResponse,
  AdminDeliverySentProofPreviewResponse,
} from "@auction/contracts/dto/admin/delivery";

import { selectableEvidence } from "./evidence";
import { type SentProofDraft, type SentProofRequest, sentProofDraftForEvidence } from "./state";
import { buildEvidence } from "./validation";

// Rust 测试的 UTC RFC 3339 文本使用显式偏移。
function time(minute: number): string {
  return `2026-11-02T01:${String(minute).padStart(2, "0")}:00+00:00`;
}

/** Rust `evidence()` 的规范观察。 */
export function evidence(): AdminDeliveryEvidenceResponse {
  return {
    reference: "mailbox_sweep:7000001",
    kind: "mailbox_sweep",
    external_mail_id: "7000001",
    observed_at: time(2),
    sent_at: time(1),
    recipient: "Sample",
    subject: "Auction settlement",
    body_digest: "cd".repeat(32),
    collected: false,
    cod_pending: 0,
    remaining_credits: 300_000,
    remaining_equipment_count: 0,
    remaining_equipment_ids: [],
    pre_collection_equipment_ids: null,
    receipt: null,
  };
}

/** Rust `detail()` 的完整详情。 */
export function detail(): AdminDeliveryDetailResponse {
  return {
    delivery: {
      id: "01010101-0101-0101-0101-010101010101",
      origin_kind: "settlement",
      origin_id: "42",
      recipient: "Sample",
      cod: 0,
      status: "manual_reconcile",
      template: "settlement_seller_payout",
      category: "settlement",
      world: "main",
      retirement_reason: null,
      suppression_reason: null,
      lease_expires_at: null,
      fencing_version: 3,
      attempt_count: 1,
      claimed_at: time(0),
      sending_at: time(0),
      completed_at: time(2),
      external_reference: null,
      failure_code: "write_unknown",
      reason_code: null,
      take_required: false,
      returned_mail_id: null,
      created_at: time(0),
      updated_at: time(2),
    },
    attempts: [
      {
        operation: "send",
        fencing_version: 3,
        world_id: "main",
        sending_at: time(0),
        recipient: "Sample",
        subject: "Auction settlement",
        body_digest: "ab".repeat(32),
        credits: 300_000,
        cod: 0,
        equipment_ids: [],
        external_mail_id: null,
        credential: null,
        bound_sent_mail_id: null,
      },
    ],
    evidence: [evidence()],
    custody: {
      collected: false,
      cod_pending: 0,
      remaining_credits: 300_000,
      receipt: null,
      observed_at: time(2),
      operator_confirmed_collected: false,
    },
    dispositions: [],
    obligation: null,
    recovery: null,
  };
}

/** Rust `filled_draft(detail)`：从首条可选证据预填并补上操作员字段。 */
export function filledDraft(value: AdminDeliveryDetailResponse): SentProofDraft {
  const draft = sentProofDraftForEvidence(selectableEvidence(value)[0] ?? null);
  return {
    ...draft,
    senderName: "Sample",
    senderUid: "12345",
    reason: "verified in Sent",
  };
}

/** Rust `request_and_preview(mid, …)` 的请求与预览组合。 */
export function requestAndPreview(
  mid: string,
  priorUnboundAttempts: boolean,
  sentBeforeCurrentAttempt: boolean,
): {
  readonly request: SentProofRequest;
  readonly preview: AdminDeliverySentProofPreviewResponse;
} {
  const value = detail();
  const reference = `mailbox_sweep:${mid}`;
  const first = firstOf(value.evidence);
  first.external_mail_id = mid;
  first.reference = reference;
  return {
    request: {
      deliveryId: value.delivery.id,
      fencingVersion: value.delivery.fencing_version,
      evidenceReference: reference,
      evidence: builtEvidence(value),
    },
    preview: {
      eligible: true,
      blockers: [],
      delivery_id: value.delivery.id,
      expected_fencing_version: value.delivery.fencing_version,
      external_reference: `mooglemail:main:send:${mid}`,
      requires_collected_asset_acceptance: false,
      requires_attempt_history_acceptance: priorUnboundAttempts || sentBeforeCurrentAttempt,
      prior_unbound_attempts: priorUnboundAttempts,
      sent_before_current_attempt: sentBeforeCurrentAttempt,
      corrects_admin_failure: sentBeforeCurrentAttempt,
    },
  };
}

/** 提取 Rust 辅助函数所用的首项；夹具不完整时立即失败。 */
export function firstOf<T>(entries: readonly T[]): T {
  const first = entries[0];
  if (first === undefined) throw new Error("缺少测试夹具首项");
  return first;
}

/** 取出应当成功的证据组装结果。 */
export function builtEvidence(value: AdminDeliveryDetailResponse, draft = filledDraft(value)) {
  const built = buildEvidence(value, draft);
  if (!built.ok) throw new Error(`证据组装失败：${built.error.kind}`);
  return built.value;
}
