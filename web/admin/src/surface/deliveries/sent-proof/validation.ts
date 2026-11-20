// 人工 Sent 证明草稿的本地形状校验与闭集请求组装。
//
// 这里只阻止明显不完整的草稿；真正的准入判定仍由服务端预览与事务完成。

import {
  adminDeliveryDigest32FromHex,
  adminDeliveryEvidenceRemainingEquipmentIds,
  type AdminDeliveryAttemptResponse as DeliveryAttempt,
  type AdminDeliveryDetailResponse as DeliveryDetail,
  type AdminDeliveryEvidenceResponse as DeliveryEvidence,
  type AdminDeliverySentEvidenceRequest as DeliverySentEvidence,
} from "@auction/contracts/dto/admin/delivery";
import { err, ok, type Result } from "@auction/web-shared/result";
import { utf8ByteLength } from "@auction/web-shared/text";

import { parseStrictRfc3339, trimRustWhitespace } from "../../../api/session-time";
import {
  type LocalTimeError,
  MAX_JSON_SAFE_INTEGER,
  parseLocalDatetime,
} from "../../../api/validation";
import { latestSendAttempt, SELECTABLE_EVIDENCE_KINDS } from "./evidence";
import type { SentProofDraft, SentProofIssue } from "./state";

const NANOS_PER_SECOND = 1_000_000_000n;
const SECONDS_PER_MINUTE = 60n;

/** 把 `datetime-local` 输入换算为带 `Z` 的整分钟；秒或小数非零时不猜测。 */
export function sentMinuteFromInput(value: string): Result<string, LocalTimeError> {
  const parsed = parseLocalDatetime(value);
  if (!parsed.ok) {
    return parsed;
  }
  const parts = rfc3339SecondAndNanosecond(parsed.value);
  if (parts === null || parts.second !== 0n || parts.nanosecond !== 0n) {
    return err("invalid");
  }
  return ok(sentMinuteText(parsed.value));
}

/** 未编辑时直接提交证据已记录的 UTC 分钟，保留服务端文本。 */
export function sentMinuteFromTimestamp(value: string): string | null {
  const parts = rfc3339SecondAndNanosecond(value);
  return parts !== null && parts.second === 0n && parts.nanosecond === 0n ? value : null;
}

/**
 * 发送分钟统一使用带 `Z` 的 UTC 整分钟文本（`YYYY-MM-DDTHH:MM:00Z`）；入参的秒与小数已为零。
 * `parseLocalDatetime` 的 AutoSi 文本在小数秒为零时已经等于 `SecondsFormat::Secs` 的写法。
 */
function sentMinuteText(minute: string): string {
  return minute;
}

function validSenderName(value: string): string | null {
  const trimmed = trimRustWhitespace(value);
  if (trimmed === "" || utf8ByteLength(trimmed) > 128 || /\p{Cc}/u.test(trimmed)) {
    return null;
  }
  return trimmed;
}

function validSenderUid(value: string): string | null {
  const trimmed = trimRustWhitespace(value);
  if (!/^[0-9]+$/u.test(trimmed) || trimmed.length > 64 || trimmed.startsWith("0")) {
    return null;
  }
  return trimmed;
}

function parseAmount(value: string): number | null {
  const trimmed = trimRustWhitespace(value);
  // Rust 的 i64 解析只接受可选的符号与十进制数字；最终边界仍是 JSON 安全整数。
  if (!/^[+-]?[0-9]+$/u.test(trimmed)) {
    return null;
  }
  const parsed = Number(trimmed);
  if (!Number.isSafeInteger(parsed) || parsed < 0 || parsed > MAX_JSON_SAFE_INTEGER) {
    return null;
  }
  // Rust 的 `-0` 解析为 0；避免把负零发到 wire 上。
  return parsed === 0 ? 0 : parsed;
}

/** 只有已领取的装备可走缺少旧清单的回执路径。 */
export function missingObservedIds(
  evidence: DeliveryEvidence,
  attempt: DeliveryAttempt,
  collected: boolean,
): boolean {
  return (
    adminDeliveryEvidenceRemainingEquipmentIds(evidence) === null &&
    (!collected || evidence.collected === false || attempt.equipment_ids.length === 0)
  );
}

/** 组合冻结尝试与已采集证据；任一必需字段缺失都返回本地问题码，不提交半成品。 */
export function buildEvidence(
  detail: DeliveryDetail,
  draft: SentProofDraft,
): Result<DeliverySentEvidence, SentProofIssue> {
  const reference = trimRustWhitespace(draft.evidenceReference);
  const evidence = detail.evidence.find(
    (item) =>
      item.reference === reference &&
      SELECTABLE_EVIDENCE_KINDS.some((kind: string) => kind === item.kind),
  );
  if (evidence === undefined) {
    return err({ kind: "evidence" });
  }
  const attempt = latestSendAttempt(detail);
  if (attempt === null) {
    return err({ kind: "attempt" });
  }
  if (missingObservedIds(evidence, attempt, draft.collected)) {
    return err({ kind: "missing-observed-equipment-ids" });
  }
  const externalMailId = evidence.external_mail_id;
  if (externalMailId === null) {
    return err({ kind: "evidence" });
  }
  const observedAt = evidence.observed_at;
  if (observedAt === null) {
    return err({ kind: "evidence" });
  }
  const observedBodyDigest =
    evidence.body_digest === null ? null : adminDeliveryDigest32FromHex(evidence.body_digest);
  if (observedBodyDigest === null) {
    return err({ kind: "digest" });
  }
  const originalBodyDigest =
    attempt.body_digest === null ? null : adminDeliveryDigest32FromHex(attempt.body_digest);
  if (originalBodyDigest === null) {
    return err({ kind: "digest" });
  }
  const recipient = evidence.recipient ?? attempt.recipient;
  if (recipient === null) {
    return err({ kind: "recipient" });
  }
  const subject = evidence.subject ?? attempt.subject;
  if (subject === null) {
    return err({ kind: "subject" });
  }
  const sentMinute = sentMinuteForDraft(evidence, draft);
  if (!sentMinute.ok) {
    return err({ kind: "sent-minute", error: sentMinute.error });
  }
  const senderName = validSenderName(draft.senderName);
  if (senderName === null) {
    return err({ kind: "sender-name" });
  }
  const senderUid = validSenderUid(draft.senderUid);
  if (senderUid === null) {
    return err({ kind: "sender-uid" });
  }
  const remainingCredits = parseAmount(draft.remainingCredits);
  if (remainingCredits === null) {
    return err({ kind: "remaining-credits" });
  }
  const codPending = parseAmount(draft.codPending);
  if (codPending === null) {
    return err({ kind: "cod-pending" });
  }
  let receipt = "";
  if (draft.collected) {
    if (utf8ByteLength(draft.receipt) > 2048) {
      return err({ kind: "receipt" });
    }
    receipt = trimRustWhitespace(draft.receipt);
  }
  let equipmentIds: readonly string[] = [];
  if (!draft.collected) {
    const remainingIds = adminDeliveryEvidenceRemainingEquipmentIds(evidence);
    if (remainingIds === null) {
      return err({ kind: "missing-observed-equipment-ids" });
    }
    equipmentIds = [...remainingIds];
  }
  return ok({
    schema_version: 1,
    world_id: attempt.world_id,
    external_mail_id: externalMailId,
    sender_uid: senderUid,
    sender_name: senderName,
    recipient,
    subject,
    sent_minute: sentMinute.value,
    observed_at: observedAt,
    original_body_digest: originalBodyDigest,
    observed_body_digest: observedBodyDigest,
    remaining_credits: remainingCredits,
    cod_pending: codPending,
    equipment_ids: equipmentIds,
    collected: draft.collected,
    receipt,
  });
}

function sentMinuteForDraft(
  evidence: DeliveryEvidence,
  draft: SentProofDraft,
): Result<string, LocalTimeError> {
  if (draft.sentMinuteEdited) {
    return sentMinuteFromInput(trimRustWhitespace(draft.sentMinute));
  }
  const sentAt = evidence.sent_at;
  if (sentAt === null) {
    return err("invalid");
  }
  const minute = sentMinuteFromTimestamp(sentAt);
  return minute === null ? err("invalid") : ok(minute);
}

interface Rfc3339Parts {
  readonly second: bigint;
  readonly nanosecond: bigint;
}

/** 取 UTC 的秒内序号与秒内纳秒；与源码 `to_utc_datetime().second()` 及小数部分一致。 */
function rfc3339SecondAndNanosecond(value: string): Rfc3339Parts | null {
  const nanos = parseStrictRfc3339(value);
  if (nanos === null) {
    return null;
  }
  let seconds = nanos / NANOS_PER_SECOND;
  let nanosecond = nanos % NANOS_PER_SECOND;
  if (nanosecond < 0n) {
    seconds -= 1n;
    nanosecond += NANOS_PER_SECOND;
  }
  const second = ((seconds % SECONDS_PER_MINUTE) + SECONDS_PER_MINUTE) % SECONDS_PER_MINUTE;
  return { second, nanosecond };
}
