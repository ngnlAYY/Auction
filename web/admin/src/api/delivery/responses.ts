// 投递详情、人工 Sent 预览与确认回执的严格解码。
//
// 只接受本次请求的投递身份、正 fence、服务端闭集与自洽的跨字段约束；浏览器不补全
// blockers，也不把缺省证据键当成 `include=sent_proof` 的投影。

import type {
  AdminDeliveryAttemptResponse,
  AdminDeliveryCustodyResponse,
  AdminDeliveryDetailResponse,
  AdminDeliveryDetailWithActionsResponse,
  AdminDeliveryDispositionResponse,
  AdminDeliveryEvidenceResponse,
  AdminDeliveryObligationResponse,
  AdminDeliveryRecoveryResponse,
  AdminDeliverySentProofPreviewResponse,
  AdminDeliverySentProofResponse,
} from "@auction/contracts/dto/admin/delivery";
import {
  adminDeliveryEvidencePreCollectionEquipmentIds,
  adminDeliveryEvidenceRemainingEquipmentIds,
} from "@auction/contracts/dto/admin/delivery";
import { err, ok, type Result } from "@auction/web-shared/result";
import { compareText, utf8ByteLength } from "@auction/web-shared/text";
import { validUuid } from "@auction/web-shared/ui/validation";

import {
  MAX_JSON_SAFE_INTEGER,
  validCount,
  validSafeText,
  validSuccessCode,
  validTrimmedText,
} from "../validation";
import { decodeDelivery, parseDeliveryTime, type StrictDeliveryEnvelope } from "./decode";

const ATTEMPT_OPERATIONS: readonly string[] = ["send", "take"];

const EVIDENCE_KINDS: readonly string[] = [
  "mailbox_sweep",
  "sent_reference_sighting",
  "compose_discard",
  "sent_operator_confirmed",
];

const DISPOSITION_KINDS: readonly string[] = [
  "release",
  "retry_failed",
  "reconcile",
  "sent_operator_confirmed",
  "operator_acknowledged",
  "auto_sent_proof",
  "operator_sent_confirmation",
  "admin_failed",
  "equipment_loss",
  "cod_returned",
  "recovery_equipment_recovered",
  "legacy_self",
  "preflight_retired",
];

const PREVIEW_BLOCKERS: readonly string[] = [
  "delivery_not_found",
  "unsupported_delivery_scope",
  "requires_unclaimed_write_unknown_or_admin_failure",
  "fulfillment_identity_or_claim_conflict",
  "conflicting_recovery_disposition",
  "existing_binding_retirement_or_conflicting_attempt",
  "requires_unique_ready_sender_funding_account",
  "sender_player_name_mismatch",
  "frozen_credential_retired_or_changed",
  "sent_minute_outside_frozen_window",
  "requires_exact_unknown_seller_reservation",
  "frozen_identity_body_money_or_assets_mismatch",
  "missing_observed_equipment_ids",
  "observed_equipment_ids_mismatch",
];

/** 详情与处置准入来自同一快照；投递 ID、成功信封与完整安全投影都必须通过校验。 */
export function decodeDetailWithActions(
  payload: StrictDeliveryEnvelope<AdminDeliveryDetailWithActionsResponse>,
  deliveryId: string,
  includeSentProof: boolean,
): Result<AdminDeliveryDetailWithActionsResponse, null> {
  const data = payload.data;
  if (
    data.detail.delivery.id !== deliveryId ||
    !validSuccessCode(payload.code) ||
    payload.message.length === 0
  ) {
    return err(null);
  }
  if (!validateDetail(data.detail, includeSentProof)) {
    return err(null);
  }
  return ok(data);
}

/** 人工 Sent 预览只接受本次投递、正 fence 与服务端 blocker 闭集。 */
export function decodePreview(
  payload: StrictDeliveryEnvelope<AdminDeliverySentProofPreviewResponse>,
  deliveryId: string,
): Result<AdminDeliverySentProofPreviewResponse, null> {
  if (!validSuccessCode(payload.code) || payload.message.length === 0) {
    return err(null);
  }
  const preview = payload.data;
  const priorAttempts = preview.prior_unbound_attempts ?? null;
  const beforeCurrent = preview.sent_before_current_attempt ?? null;
  if (priorAttempts === null || beforeCurrent === null) {
    return err(null);
  }
  if (
    preview.delivery_id !== deliveryId ||
    !validFencingVersion(preview.expected_fencing_version, 1) ||
    !validTrimmedText(preview.external_reference) ||
    preview.blockers.length > 32 ||
    !preview.blockers.every((blocker) => PREVIEW_BLOCKERS.includes(blocker)) ||
    preview.eligible !== (preview.blockers.length === 0) ||
    preview.requires_attempt_history_acceptance !== (priorAttempts || beforeCurrent) ||
    (beforeCurrent && !preview.corrects_admin_failure)
  ) {
    return err(null);
  }
  return ok(preview);
}

/** 确认回执必须回显请求的稳定命令 ID、投递 ID 与非空外部引用。 */
export function decodeConfirmation(
  payload: StrictDeliveryEnvelope<AdminDeliverySentProofResponse>,
  commandId: string,
  deliveryId: string,
): Result<AdminDeliverySentProofResponse, null> {
  if (!validSuccessCode(payload.code) || payload.message.length === 0) {
    return err(null);
  }
  const result = payload.data;
  if (
    result.command_id !== commandId ||
    result.delivery_id !== deliveryId ||
    !validTrimmedText(result.external_reference)
  ) {
    return err(null);
  }
  return ok(result);
}

function validateDetail(data: AdminDeliveryDetailResponse, includeSentProof: boolean): boolean {
  if (!decodeDelivery(data.delivery).ok) {
    return false;
  }
  for (const attempt of data.attempts) {
    if (!validAttempt(attempt)) {
      return false;
    }
  }
  for (const evidence of data.evidence) {
    const hasRemainingIds = evidence.remaining_equipment_ids !== undefined;
    const hasPreCollectionIds = evidence.pre_collection_equipment_ids !== undefined;
    if (
      !validEvidence(evidence) ||
      hasRemainingIds !== includeSentProof ||
      hasPreCollectionIds !== includeSentProof
    ) {
      return false;
    }
  }
  if (!validCustody(data.custody)) {
    return false;
  }
  for (const disposition of data.dispositions) {
    if (!validDisposition(disposition)) {
      return false;
    }
  }
  if (
    data.obligation !== null &&
    data.obligation !== undefined &&
    !validObligation(data.obligation)
  ) {
    return false;
  }
  if (data.recovery !== null && data.recovery !== undefined && !validRecovery(data.recovery)) {
    return false;
  }
  return true;
}

function validAttempt(value: AdminDeliveryAttemptResponse): boolean {
  const credential = value.credential ?? null;
  const recipient = value.recipient ?? null;
  const subject = value.subject ?? null;
  const credits = value.credits ?? null;
  const cod = value.cod ?? null;
  if (
    !ATTEMPT_OPERATIONS.includes(value.operation) ||
    !validFencingVersion(value.fencing_version, 0) ||
    !validTrimmedText(value.world_id) ||
    parseDeliveryTime(value.sending_at) === null ||
    !optionalValueIs(value.recipient, validTrimmedText) ||
    !optionalValueIs(value.subject, (text) => validSafeText(text, 512)) ||
    !optionalValueIs(value.body_digest, validHexDigest) ||
    !optionalValueIs(value.credits, validCount) ||
    !optionalValueIs(value.cod, validCount) ||
    !value.equipment_ids.every(validPositiveDecimal) ||
    !optionalValueIs(value.external_mail_id, validMailId) ||
    !optionalValueIs(value.bound_sent_mail_id, validMailId) ||
    (credential !== null &&
      (!validUuid(credential.credential_id) ||
        !validFencingVersion(credential.credential_version, 0) ||
        !validFencingVersion(credential.runtime_generation, 0)))
  ) {
    return false;
  }
  if (value.operation === "send") {
    return recipient !== null && subject !== null && credits !== null && cod !== null;
  }
  return value.external_mail_id !== null && value.external_mail_id !== undefined;
}

function validEvidence(value: AdminDeliveryEvidenceResponse): boolean {
  const remainingIds = adminDeliveryEvidenceRemainingEquipmentIds(value);
  const preCollectionIds = adminDeliveryEvidencePreCollectionEquipmentIds(value);
  return (
    validSafeText(value.reference, 128) &&
    EVIDENCE_KINDS.includes(value.kind) &&
    optionalValueIs(value.external_mail_id, validMailId) &&
    optionalValueIs(value.observed_at, (time) => parseDeliveryTime(time) !== null) &&
    optionalValueIs(value.sent_at, (time) => parseDeliveryTime(time) !== null) &&
    optionalValueIs(value.recipient, (text) => validSafeText(text, 512)) &&
    optionalValueIs(value.subject, (text) => validSafeText(text, 512)) &&
    optionalValueIs(value.body_digest, validHexDigest) &&
    optionalValueIs(value.cod_pending, validCount) &&
    optionalValueIs(value.remaining_credits, validCount) &&
    optionalValueIs(value.remaining_equipment_count, validCount) &&
    (remainingIds === null ||
      (validObservedIds(remainingIds) &&
        optionalValueIs(
          value.remaining_equipment_count,
          (count) => count === remainingIds.length,
        ))) &&
    (preCollectionIds === null ||
      (preCollectionIds.length > 0 && validObservedIds(preCollectionIds))) &&
    optionalValueIs(value.receipt, (text) => utf8ByteLength(text) <= 2048)
  );
}

function validObservedIds(ids: readonly string[]): boolean {
  let previous: string | null = null;
  for (const id of ids) {
    if (!validPositiveDecimal(id) || (previous !== null && compareText(previous, id) >= 0)) {
      return false;
    }
    previous = id;
  }
  return true;
}

function validCustody(value: AdminDeliveryCustodyResponse): boolean {
  return (
    optionalValueIs(value.cod_pending, validCount) &&
    optionalValueIs(value.remaining_credits, validCount) &&
    optionalValueIs(value.receipt, (text) => utf8ByteLength(text) <= 2048) &&
    optionalValueIs(value.observed_at, (time) => parseDeliveryTime(time) !== null)
  );
}

function validDisposition(value: AdminDeliveryDispositionResponse): boolean {
  return (
    validUuid(value.command_id) &&
    DISPOSITION_KINDS.includes(value.kind) &&
    parseDeliveryTime(value.occurred_at) !== null &&
    optionalValueIs(value.status, validTrimmedText) &&
    optionalValueIs(value.fencing_version, (version) => validFencingVersion(version, 0)) &&
    optionalValueIs(value.failure_code, validTrimmedText) &&
    optionalValueIs(value.external_reference, validTrimmedText) &&
    optionalValueIs(value.actor, (text) => validSafeText(text, 128)) &&
    optionalValueIs(value.reason, (text) => utf8ByteLength(text) <= 2048)
  );
}

function validObligation(value: AdminDeliveryObligationResponse): boolean {
  return (
    validUuid(value.obligation_id) &&
    validUuid(value.auction_id) &&
    validUuid(value.lot_id) &&
    validTrimmedText(value.world_id) &&
    validTrimmedText(value.obligation_kind) &&
    validTrimmedText(value.business_state) &&
    validCount(value.amount) &&
    optionalValueIs(value.fulfillment_state, validTrimmedText) &&
    optionalValueIs(value.fulfillment_error_code, validTrimmedText)
  );
}

function validRecovery(value: AdminDeliveryRecoveryResponse): boolean {
  return (
    validUuid(value.recovery_id) &&
    validUuid(value.payment_obligation_id) &&
    validTrimmedText(value.stage) &&
    parseDeliveryTime(value.payment_deadline_at) !== null &&
    optionalValueIs(value.resolution, validTrimmedText) &&
    optionalValueIs(value.blocking_code, validTrimmedText) &&
    optionalValueIs(value.manual_reconcile_delivery_id, validUuid)
  );
}

function validFencingVersion(value: number, minimum: number): boolean {
  return Number.isSafeInteger(value) && value >= minimum && value <= MAX_JSON_SAFE_INTEGER;
}

function validPositiveDecimal(value: string): boolean {
  return /^[1-9][0-9]{0,63}$/.test(value);
}

function validMailId(value: string): boolean {
  return /^[0-9]{1,32}$/.test(value);
}

function validHexDigest(value: string): boolean {
  return /^[0-9a-f]{64}$/.test(value);
}

function optionalValueIs<T>(value: T | null | undefined, predicate: (item: T) => boolean): boolean {
  return value === null || value === undefined || predicate(value);
}
