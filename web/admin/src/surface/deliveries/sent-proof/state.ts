// 人工 Sent 证明的本地状态：一次性提交、可编辑草稿与形状问题码。
// 这些类型只承载浏览器侧投影。

import {
  adminDeliveryDigest32,
  type AdminDeliveryEvidenceResponse as DeliveryEvidence,
  type AdminDeliverySentEvidenceRequest as DeliverySentEvidence,
  type AdminDeliverySentProofPreviewResponse as DeliverySentProofPreview,
} from "@auction/contracts/dto/admin/delivery";
import { err, ok, type Result, unreachable } from "@auction/web-shared/result";
import { t } from "@auction/web-shared/ui/i18n";

import { type AdminApiFailure, adminApiFailureClient } from "../../../api/failure";
import { trimRustWhitespace } from "../../../api/session-time";
import { datetimeLocalValue, type LocalTimeError } from "../../../api/validation";

/** 详情页转交的一次性提交；`command_id` 由命令宿主生成并在重试间保持。 */
export interface SentProofSubmission {
  readonly evidenceReference: string;
  readonly evidence: DeliverySentEvidence;
  readonly reason: string;
  readonly expectedFencingVersion: number;
  readonly acceptCollectedAssets: boolean;
  readonly acceptAttemptHistory: boolean;
}

/** 操作员可编辑的观察草稿；预填只复制证据中已有的字段，不伪造未观测值。 */
export interface SentProofDraft {
  readonly evidenceReference: string;
  readonly senderName: string;
  readonly senderUid: string;
  readonly sentMinute: string;
  readonly sentMinuteEdited: boolean;
  readonly remainingCredits: string;
  readonly codPending: string;
  readonly collected: boolean;
  readonly receipt: string;
  readonly reason: string;
}

/** 由证据生成草稿；缺失的可选字段按源码的空值初始化。 */
export function sentProofDraftForEvidence(evidence: DeliveryEvidence | null): SentProofDraft {
  const sentAt = evidence === null ? null : evidence.sent_at;
  return {
    evidenceReference: evidence === null ? "" : evidence.reference,
    senderName: "",
    senderUid: "",
    // 本地值只供展示；时钟回拨时不能拿它反推原 UTC 分钟。
    sentMinute: sentAt === null ? "" : (datetimeLocalValue(sentAt) ?? ""),
    sentMinuteEdited: false,
    remainingCredits:
      evidence === null || evidence.remaining_credits === null
        ? ""
        : String(evidence.remaining_credits),
    codPending:
      evidence === null || evidence.cod_pending === null ? "" : String(evidence.cod_pending),
    collected: evidence === null ? false : (evidence.collected ?? false),
    receipt: evidence === null || evidence.receipt === null ? "" : evidence.receipt,
    reason: "",
  };
}

/** 切换证据时保留操作员已填写的发送者与原因，只替换由证据派生的字段。 */
export function sentProofDraftWithEvidence(
  draft: SentProofDraft,
  evidence: DeliveryEvidence | null,
): SentProofDraft {
  const next = sentProofDraftForEvidence(evidence);
  return {
    ...next,
    senderName: draft.senderName,
    senderUid: draft.senderUid,
    reason: draft.reason,
  };
}

/** 草稿形状问题；只在浏览器本地阻止提交，语义准入仍由服务端预览与事务裁决。 */
export type SentProofIssue =
  | { readonly kind: "sent-minute"; readonly error: LocalTimeError }
  | { readonly kind: "evidence" }
  | { readonly kind: "attempt" }
  | { readonly kind: "digest" }
  | { readonly kind: "recipient" }
  | { readonly kind: "subject" }
  | { readonly kind: "sender-name" }
  | { readonly kind: "sender-uid" }
  | { readonly kind: "remaining-credits" }
  | { readonly kind: "cod-pending" }
  | { readonly kind: "receipt" }
  | { readonly kind: "missing-observed-equipment-ids" };

/** 草稿问题对应的当前语言文案。 */
export function sentProofIssueMessage(issue: SentProofIssue): string {
  switch (issue.kind) {
    case "evidence":
      return t("delivery-sent-proof-issue-evidence");
    case "attempt":
      return t("delivery-sent-proof-issue-attempt");
    case "digest":
      return t("delivery-sent-proof-issue-digest");
    case "recipient":
      return t("delivery-sent-proof-issue-recipient");
    case "subject":
      return t("delivery-sent-proof-issue-subject");
    case "sent-minute":
      switch (issue.error) {
        case "invalid":
          return t("delivery-sent-proof-issue-sent-minute");
        case "nonexistent":
          return t("admin-auctions-nonexistent-local-time");
        case "ambiguous":
          return t("admin-auctions-ambiguous-local-time");
        default:
          return unreachable(issue.error);
      }
    case "sender-name":
      return t("delivery-sent-proof-issue-sender-name");
    case "sender-uid":
      return t("delivery-sent-proof-issue-sender-uid");
    case "remaining-credits":
    case "cod-pending":
      return t("delivery-sent-proof-issue-amount");
    case "receipt":
      return t("delivery-sent-proof-issue-receipt");
    case "missing-observed-equipment-ids":
      return t("delivery-sent-proof-missing-observed-ids");
    default:
      return unreachable(issue);
  }
}

/** 草稿问题归属的控件或汇总区。 */
export function sentProofIssueFieldId(issue: SentProofIssue): string {
  switch (issue.kind) {
    case "evidence":
    case "missing-observed-equipment-ids":
      return "delivery-sent-evidence";
    case "attempt":
    case "digest":
    case "recipient":
    case "subject":
      return "delivery-sent-evidence-summary";
    case "sent-minute":
      return "delivery-sent-minute";
    case "sender-name":
      return "delivery-sent-sender-name";
    case "sender-uid":
      return "delivery-sent-sender-uid";
    case "remaining-credits":
      return "delivery-sent-remaining-credits";
    case "cod-pending":
      return "delivery-sent-cod-pending";
    case "receipt":
      return "delivery-sent-receipt";
    default:
      return unreachable(issue);
  }
}

/** 发送到预览的请求和预览时的详情 fence。 */
export interface SentProofRequest {
  readonly deliveryId: string;
  readonly fencingVersion: number;
  readonly evidenceReference: string;
  readonly evidence: DeliverySentEvidence;
}

/** 操作员已确认的历史责任。 */
export interface AcceptedResponsibilities {
  readonly collectedAssets: boolean;
  readonly priorUnboundAttempts: boolean;
  readonly sentBeforeCurrentAttempt: boolean;
}

/** 通过当前 fence 与服务端预览关联的责任勾选。 */
export interface ReviewedSentProof {
  readonly request: SentProofRequest;
  readonly preview: DeliverySentProofPreview;
  readonly accepted: AcceptedResponsibilities;
}

/** 预览失败只区分本地形状问题与服务端稳定失败。 */
export type SentProofPreviewError =
  | { readonly kind: "local"; readonly issue: SentProofIssue }
  | { readonly kind: "api"; readonly failure: AdminApiFailure };

/** 点选、编辑或详情更新都使正在进行的预览和相关勾选失效。 */
export interface SentProofReview {
  /** 保留源码的 `u64` 计数；只用于拒绝迟到回执。 */
  readonly revision: bigint;
  readonly pending: boolean;
  readonly outcome: Result<ReviewedSentProof, SentProofPreviewError> | null;
}

/** 初始预览状态。 */
export function sentProofReviewDefault(): SentProofReview {
  return { revision: 0n, pending: false, outcome: null };
}

/** 读取当前预览修订号。 */
export function sentProofReviewRevision(review: SentProofReview): bigint {
  return review.revision;
}

/** 只在修订号仍匹配且已有成功预览时更新责任勾选。 */
export function sentProofReviewAcceptance(
  review: SentProofReview,
  revision: bigint,
  change: (accepted: AcceptedResponsibilities) => AcceptedResponsibilities,
): SentProofReview {
  if (revision !== review.revision || review.outcome === null || !review.outcome.ok) {
    return review;
  }
  const proof = review.outcome.value;
  return {
    ...review,
    outcome: ok({ ...proof, accepted: change(proof.accepted) }),
  };
}

/** 作废当前预览；修订号按 `u64` 回绕。 */
export function sentProofReviewClear(review: SentProofReview): SentProofReview {
  const U64_MASK = (1n << 64n) - 1n;
  return {
    revision: (review.revision + 1n) & U64_MASK,
    pending: false,
    outcome: null,
  };
}

/** 开始预览并返回本次请求要核对的修订号。 */
export function sentProofReviewBegin(review: SentProofReview): SentProofReview {
  return { ...sentProofReviewClear(review), pending: true };
}

/** 预览完成的原子结果：新状态与是否要求重读详情。 */
export interface SentProofReviewCompletion {
  readonly review: SentProofReview;
  readonly stale: boolean;
}

/** 返回当前预览是否要求重读详情。只有仍被等待的请求可以使投递详情失效。 */
export function sentProofReviewComplete(
  review: SentProofReview,
  revision: bigint,
  request: SentProofRequest,
  result: Result<DeliverySentProofPreview, AdminApiFailure>,
): SentProofReviewCompletion {
  // 迟到响应不能恢复旧责任，也不能清除另一份证据正在请求的加载状态。
  if (revision !== review.revision || !review.pending) {
    return { review, stale: false };
  }
  const outcome = previewOutcome(request, result);
  const next: SentProofReview = { ...review, pending: false, outcome };
  return {
    review: next,
    stale:
      outcome !== null &&
      !outcome.ok &&
      outcome.error.kind === "api" &&
      outcome.error.failure.code === "stale_delivery_state",
  };
}

function previewOutcome(
  request: SentProofRequest,
  result: Result<DeliverySentProofPreview, AdminApiFailure>,
): Result<ReviewedSentProof, SentProofPreviewError> {
  if (!result.ok) {
    return err({ kind: "api", failure: result.error });
  }
  const preview = result.value;
  if (
    preview.expected_fencing_version !== request.fencingVersion ||
    preview.delivery_id !== request.deliveryId ||
    preview.external_reference !==
      `mooglemail:${request.evidence.world_id}:send:${request.evidence.external_mail_id}`
  ) {
    return err({
      kind: "api",
      failure: adminApiFailureClient("stale_delivery_state"),
    });
  }
  return ok({
    request,
    preview,
    accepted: {
      collectedAssets: false,
      priorUnboundAttempts: false,
      sentBeforeCurrentAttempt: false,
    },
  });
}

/** 预览仍对应当前投递与 fence，且所有实际触发的责任位已确认。 */
export function reviewedSentProofCanConfirm(
  proof: ReviewedSentProof,
  deliveryId: string,
  fencingVersion: number,
): boolean {
  return (
    proof.request.deliveryId === deliveryId &&
    proof.request.fencingVersion === fencingVersion &&
    proof.preview.eligible &&
    (!proof.preview.requires_collected_asset_acceptance || proof.accepted.collectedAssets) &&
    (proof.preview.prior_unbound_attempts === false || proof.accepted.priorUnboundAttempts) &&
    (proof.preview.sent_before_current_attempt === false || proof.accepted.sentBeforeCurrentAttempt)
  );
}

/** 已发布命令用一个历史责任位；分别确认实际触发的所有理由后才可组装。 */
export function reviewedSentProofSubmission(
  proof: ReviewedSentProof,
  reason: string,
): SentProofSubmission {
  return {
    evidenceReference: proof.request.evidenceReference,
    evidence: copySentEvidence(proof.request.evidence),
    reason: trimRustWhitespace(reason),
    expectedFencingVersion: proof.request.fencingVersion,
    acceptCollectedAssets:
      proof.preview.requires_collected_asset_acceptance && proof.accepted.collectedAssets,
    acceptAttemptHistory:
      proof.preview.requires_attempt_history_acceptance &&
      (proof.preview.prior_unbound_attempts !== true || proof.accepted.priorUnboundAttempts) &&
      (proof.preview.sent_before_current_attempt !== true ||
        proof.accepted.sentBeforeCurrentAttempt),
  };
}

/** 复制提交证据；与 Rust 的 `Clone` 一致，摘要与装备 ID 数组都换成新实例，
 * 后续修改提交对象不会回写预览快照。 */
function copySentEvidence(evidence: DeliverySentEvidence): DeliverySentEvidence {
  return {
    ...evidence,
    original_body_digest: copyDigest(evidence.original_body_digest),
    observed_body_digest: copyDigest(evidence.observed_body_digest),
    equipment_ids: [...evidence.equipment_ids],
  };
}

/** 复制 32 字节摘要。品牌值只由已校验的构造函数产出（恰好 32 项、逐项 0..=255），
 * 重新构造必然成功；`??` 分支不可达，只为消除空值类型。 */
function copyDigest(
  bytes: DeliverySentEvidence["original_body_digest"],
): DeliverySentEvidence["original_body_digest"] {
  return adminDeliveryDigest32(bytes) ?? bytes;
}
