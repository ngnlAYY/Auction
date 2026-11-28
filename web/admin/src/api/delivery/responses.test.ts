// Rust `delivery/tests.rs` 的详情、处置准入、预览与确认回执回归。

import { expect, test } from "bun:test";
import {
  AdminDeliverySentProofPreviewResponseSchema,
  AdminDeliverySentProofResponseSchema,
} from "@auction/contracts/dto/admin/delivery";
import { decode } from "@auction/contracts/wire";

import { adminApiFailureFromHttpBody } from "../failure";
import { strictDeliveryEnvelopeSchema } from "./decode";
import {
  attempt,
  checkBeforeSendProjection,
  DELIVERY_ID,
  DETAIL_WITH_ACTIONS_ENVELOPE,
  decodeDetailFixture,
  decodePlainDetail,
  delivery,
  detail,
  type JsonRecord,
  jsonObject,
  OTHER_ID,
  PLAIN_DETAIL_ENVELOPE,
  parseFixture,
  preparationFailure,
  readAdminFixture,
  requireDecoded,
  requireElement,
  retiredBeforeSend,
  time,
  withoutField,
} from "./delivery-fixture.test";
import { decodeConfirmation, decodeDetailWithActions, decodePreview } from "./responses";

const I32_MAX = 2_147_483_647;

function detailFor(deliveryValue: unknown): JsonRecord {
  return { ...detail(), delivery: deliveryValue, attempts: [] };
}

test("delivery_suppression_prefers_failure_code_and_can_use_reason_code", () => {
  const alert = preparationFailure("retry_exhausted");
  alert.origin_kind = "operations_alert";
  alert.claimed_at = null;
  alert.attempt_count = 0;
  alert.fencing_version = 0;
  alert.reason_code = "operations_alert_batch_terminal";
  expect(checkBeforeSendProjection(alert)).toEqual({ list: true, detail: true });
  alert.suppression_reason = "operations_alert_batch_terminal";
  expect(checkBeforeSendProjection(alert)).toEqual({ list: false, detail: false });

  const conflict = preparationFailure("verified_not_sent");
  conflict.origin_kind = "mail_intake";
  conflict.reason_code = "mail_disposition_conflict";
  conflict.suppression_reason = "mail_disposition_conflict";
  expect(checkBeforeSendProjection(conflict)).toEqual({ list: true, detail: true });
});

test("delivery_rejects_preparation_failure_disguised_as_disposition_conflict", () => {
  const failed = preparationFailure("task_panic");
  failed.origin_kind = "mail_intake";
  failed.reason_code = "mail_disposition_conflict";
  expect(checkBeforeSendProjection(failed)).toEqual({ list: false, detail: false });
  delete failed.reason_code;
  expect(checkBeforeSendProjection(failed)).toEqual({ list: true, detail: true });
});

test("delivery_checks_disposition_conflict_before_recovery_superseded", () => {
  const conflict = preparationFailure("recovery_superseded");
  conflict.origin_kind = "mail_intake";
  conflict.category = "intake";
  conflict.template = "auction_equipment_rejected";
  conflict.reason_code = "mail_disposition_conflict";
  conflict.suppression_reason = "mail_disposition_conflict";
  expect(checkBeforeSendProjection(conflict)).toEqual({ list: true, detail: true });

  const unattempted = structuredClone(conflict);
  unattempted.attempt_count = 0;
  expect(checkBeforeSendProjection(unattempted)).toEqual({ list: false, detail: false });

  delete conflict.suppression_reason;
  expect(checkBeforeSendProjection(conflict)).toEqual({ list: true, detail: true });
  conflict.sending_at = time(0);
  expect(checkBeforeSendProjection(conflict)).toEqual({ list: false, detail: false });
});

test("delivery_rejects_attempt_count_outside_the_storage_range", () => {
  const projected = retiredBeforeSend("seller_summary", "recipient_renamed", "recipient_renamed");
  projected.attempt_count = I32_MAX;
  expect(checkBeforeSendProjection(projected)).toEqual({ list: true, detail: true });
  projected.attempt_count = I32_MAX + 1;
  expect(checkBeforeSendProjection(projected)).toEqual({ list: false, detail: false });
});

test("delivery_requires_world_for_returned_mail_reference", () => {
  const returned = delivery();
  returned.failure_code = "recipient_not_found";
  returned.reason_code = "recipient_not_found";
  returned.take_required = true;
  returned.returned_mail_id = "3120869";
  returned.external_reference = "3120869";
  expect(checkBeforeSendProjection(returned)).toEqual({ list: true, detail: true });
  delete returned.world;
  expect(checkBeforeSendProjection(returned)).toEqual({ list: false, detail: false });
});

test("delivery_accepts_recovery_quarantine_before_send", () => {
  const quarantined = retiredBeforeSend("settlement", "recipient_renamed", "recipient_renamed");
  quarantined.status = "manual_reconcile";
  delete quarantined.reason_code;
  delete quarantined.retirement_reason;
  for (const failure of [
    "external_recovery_sent_match",
    "external_recovery_payload_match",
    "external_recovery_unresolved_send",
  ]) {
    quarantined.failure_code = failure;
    expect(checkBeforeSendProjection(quarantined)).toEqual({ list: true, detail: true });
  }
  const unknown = structuredClone(quarantined);
  unknown.failure_code = "other_failure";
  expect(checkBeforeSendProjection(unknown)).toEqual({ list: false, detail: false });
  quarantined.completed_at = null;
  expect(checkBeforeSendProjection(quarantined)).toEqual({ list: false, detail: false });
});

test("delivery_rejects_recovery_quarantine_with_send_facts", () => {
  const sent = delivery();
  expect(checkBeforeSendProjection(sent)).toEqual({ list: true, detail: true });
  for (const failure of [
    "external_recovery_sent_match",
    "external_recovery_payload_match",
    "external_recovery_unresolved_send",
  ]) {
    const quarantined = structuredClone(sent);
    quarantined.failure_code = failure;
    expect(checkBeforeSendProjection(quarantined)).toEqual({ list: false, detail: false });
  }
});

test("accepts_recovery_superseded_settlement_delivery_only_without_send_facts", () => {
  const superseded = delivery();
  superseded.status = "failed";
  superseded.failure_code = "recovery_superseded";
  superseded.claimed_at = null;
  superseded.sending_at = null;
  superseded.attempt_count = 0;
  expect(decodeDetailFixture(detailFor(superseded)).ok).toBe(true);

  const claimed = structuredClone(superseded);
  claimed.claimed_at = time(0);
  expect(decodeDetailFixture(detailFor(claimed)).ok).toBe(true);

  const sent = structuredClone(superseded);
  sent.claimed_at = time(0);
  sent.sending_at = time(0);
  sent.attempt_count = 1;
  expect(decodeDetailFixture(detailFor(sent)).ok).toBe(false);

  const otherOrigin = structuredClone(superseded);
  otherOrigin.origin_kind = "lot_watch";
  expect(decodeDetailFixture(detailFor(otherOrigin)).ok).toBe(false);
});

test("accepts_operator_acknowledged_disposition_in_detail", () => {
  const data = detail();
  data.dispositions.push({
    command_id: OTHER_ID,
    kind: "operator_acknowledged",
    occurred_at: time(2),
    fencing_version: 3,
    actor: "admin",
    reason: "handled outside the outbox",
  });
  const decoded = requireDecoded(decodeDetailFixture(data), "人工解除告警详情");
  expect(requireElement(decoded.detail.dispositions, 0, "处置记录").kind).toBe(
    "operator_acknowledged",
  );
});

test("accepts_unified_disposition_kinds_in_detail", () => {
  const data = detail();
  data.dispositions.push({
    command_id: OTHER_ID,
    kind: "auto_sent_proof",
    occurred_at: time(2),
    fencing_version: 3,
    failure_code: "sent_recovered_frozen_v1",
    actor: "delivery-worker",
  });
  expect(decodeDetailFixture(data).ok).toBe(true);
});

// 季末关闭处置（迁移 000137）随扩展投影输出，页面解码必须认识它，不能整页失败。
test("accepts_season_closed_disposition_in_detail", () => {
  const data = detail();
  data.dispositions.push({
    command_id: OTHER_ID,
    kind: "season_closed",
    occurred_at: time(2),
    fencing_version: 3,
    failure_code: "season_ended",
    actor: "consignment-worker",
  });
  const decoded = requireDecoded(decodeDetailFixture(data), "季末关闭处置详情");
  expect(requireElement(decoded.detail.dispositions, 0, "处置记录").kind).toBe("season_closed");
});

test("rejects_send_attempt_without_frozen_fields", () => {
  const data = detail();
  delete requireElement(data.attempts, 0, "Send 尝试").subject;
  expect(decodeDetailFixture(data).ok).toBe(false);
});

test("rejects_unknown_evidence_kind_and_uppercase_digest", () => {
  const unknownKind = detail();
  requireElement(unknownKind.evidence, 0, "Sent 证据").kind = "draft_discard";
  expect(decodeDetailFixture(unknownKind).ok).toBe(false);

  const uppercaseDigest = detail();
  requireElement(uppercaseDigest.evidence, 0, "Sent 证据").body_digest = "AB".repeat(32);
  expect(decodeDetailFixture(uppercaseDigest).ok).toBe(false);
});

test("rejects_take_attempt_with_send_fields_only", () => {
  const data = detail();
  data.attempts[0] = {
    ...attempt(),
    operation: "take",
  };
  // Send 内容与 Take 目标同时出现是矛盾投影，整页失败关闭。
  expect(decodeDetailFixture(data).ok).toBe(false);
  // 合法 Take 只携带目标 MID；装备、收件人、正文摘要与 Sent 绑定都必须为空。
  data.attempts[0] = {
    operation: "take",
    fencing_version: 3,
    world_id: "main",
    sending_at: time(0),
    external_mail_id: "7000001",
  };
  expect(decodeDetailFixture(data).ok).toBe(true);
  const take = requireElement(data.attempts, 0, "Take 尝试");
  take.bound_sent_mail_id = "7000002";
  expect(decodeDetailFixture(data).ok).toBe(false);
  take.bound_sent_mail_id = null;
  take.equipment_ids = ["300753957"];
  expect(decodeDetailFixture(data).ok).toBe(false);
  take.equipment_ids = [];
  delete take.external_mail_id;
  expect(decodeDetailFixture(data).ok).toBe(false);
});

function firstEvidenceInFixture(fixture: unknown): JsonRecord {
  const data = jsonObject(jsonObject(fixture, "详情信封").data, "data");
  const evidence = jsonObject(data.detail, "data.detail").evidence;
  if (!Array.isArray(evidence)) {
    throw new Error("Sent 证据夹具必须是数组");
  }
  return jsonObject(requireElement(evidence, 0, "Sent 证据夹具"), "首条 Sent 证据");
}

test("decodes_http_sent_proof_details_with_required_nullable_id_lists", () => {
  const fixtures = parseFixture(readAdminFixture("delivery-sent-proof-detail"));
  if (!Array.isArray(fixtures)) {
    throw new Error("Sent 详情夹具必须是数组");
  }
  expect(fixtures).toHaveLength(3);
  const attached = requireElement(fixtures, 0, "Sent 详情");
  const historical = requireElement(fixtures, 1, "Sent 详情");
  const collected = requireElement(fixtures, 2, "Sent 详情");
  const decodeSent = (fixture: unknown) =>
    decodeDetailWithActions(
      requireDecoded(decode(DETAIL_WITH_ACTIONS_ENVELOPE, fixture), "Sent 详情信封"),
      DELIVERY_ID,
      true,
    );
  const firstEvidence = (fixture: unknown) => {
    const { detail } = requireDecoded(decodeSent(fixture), "后端实际详情投影");
    return requireElement(detail.evidence, 0, "后端实际详情投影的首条证据");
  };
  const attachedEvidence = firstEvidence(attached);
  const historicalEvidence = firstEvidence(historical);
  const collectedEvidence = firstEvidence(collected);
  expect(attachedEvidence.remaining_equipment_ids).toEqual(["300753957"]);
  expect(attachedEvidence.pre_collection_equipment_ids).toBeNull();
  expect(historicalEvidence.remaining_equipment_ids).toBeNull();
  expect(historicalEvidence.pre_collection_equipment_ids).toBeNull();
  expect(collectedEvidence.remaining_equipment_ids).toEqual([]);
  expect(collectedEvidence.pre_collection_equipment_ids).toEqual(["300753957"]);

  for (const field of ["remaining_equipment_ids", "pre_collection_equipment_ids"]) {
    const missing = structuredClone(historical);
    const evidence = firstEvidenceInFixture(missing);
    expect(Object.hasOwn(evidence, field)).toBe(true);
    delete evidence[field];
    expect(decodeSent(missing).ok).toBe(false);
  }

  const malformed = structuredClone(attached);
  firstEvidenceInFixture(malformed).remaining_equipment_ids = ["0300753957"];
  expect(decodeSent(malformed).ok).toBe(false);

  const extra = structuredClone(attached);
  firstEvidenceInFixture(extra).future_field = null;
  expect(decode(DETAIL_WITH_ACTIONS_ENVELOPE, extra).ok).toBe(false);

  const standard = requireDecoded(decode(DETAIL_WITH_ACTIONS_ENVELOPE, attached), "Sent 详情信封");
  expect(decodeDetailWithActions(standard, DELIVERY_ID, false).ok).toBe(false);
});

test("decodes_http_sent_proof_preview_with_both_responsibility_reasons", () => {
  const fixture = parseFixture(readAdminFixture("delivery-sent-proof-preview"));
  const schema = strictDeliveryEnvelopeSchema(AdminDeliverySentProofPreviewResponseSchema);
  const decodeSent = (value: unknown) =>
    decodePreview(requireDecoded(decode(schema, value), "Sent 预览信封"), DELIVERY_ID);
  const decoded = requireDecoded(decodeSent(fixture), "后端实际预览投影");
  expect(decoded.prior_unbound_attempts).toBe(true);
  expect(decoded.sent_before_current_attempt).toBe(true);

  for (const field of ["prior_unbound_attempts", "sent_before_current_attempt"]) {
    expect(decodeSent(withoutField(fixture, "data", field)).ok).toBe(false);
  }
  const inconsistent = jsonObject(structuredClone(fixture), "预览信封");
  jsonObject(inconsistent.data, "data").requires_attempt_history_acceptance = false;
  expect(decodeSent(inconsistent).ok).toBe(false);
});

test("rejects_preview_for_other_delivery_or_eligible_with_blockers", () => {
  const preview = (deliveryId: string, eligible: boolean, blockers: string[]): JsonRecord => ({
    eligible,
    blockers,
    delivery_id: deliveryId,
    expected_fencing_version: 3,
    external_reference: "mooglemail:main:send:7000001",
    requires_collected_asset_acceptance: false,
    requires_attempt_history_acceptance: false,
    prior_unbound_attempts: false,
    sent_before_current_attempt: false,
    corrects_admin_failure: true,
  });
  const schema = strictDeliveryEnvelopeSchema(AdminDeliverySentProofPreviewResponseSchema);
  const decodeSent = (data: JsonRecord) =>
    decodePreview(
      requireDecoded(decode(schema, { code: "ok", message: "preview", data }), "Sent 预览信封"),
      DELIVERY_ID,
    );
  expect(decodeSent(preview(DELIVERY_ID, true, [])).ok).toBe(true);
  expect(decodeSent(preview(OTHER_ID, true, [])).ok).toBe(false);
  const blockers = ["sender_player_name_mismatch"];
  expect(decodeSent(preview(DELIVERY_ID, false, blockers)).ok).toBe(true);
  expect(decodeSent(preview(DELIVERY_ID, true, blockers)).ok).toBe(false);
});

test("confirmation_must_echo_command_and_delivery", () => {
  const commandId = "09090909-0909-0909-0909-090909090909";
  const data = {
    command_id: commandId,
    delivery_id: DELIVERY_ID,
    external_reference: "mooglemail:main:send:7000001",
    replayed: false,
  };
  const payload = requireDecoded(
    decode(strictDeliveryEnvelopeSchema(AdminDeliverySentProofResponseSchema), {
      code: "ok",
      message: "confirmed",
      data,
    }),
    "Sent 确认信封",
  );
  expect(decodeConfirmation(payload, commandId, DELIVERY_ID).ok).toBe(true);
  expect(decodeConfirmation(payload, "08080808-0808-0808-0808-080808080808", DELIVERY_ID).ok).toBe(
    false,
  );
  expect(decodeConfirmation(payload, commandId, OTHER_ID).ok).toBe(false);
});

test("delivery_details_decode_backend_contract_fixtures", () => {
  const decodeFixture = (name: string) => {
    const fixture = parseFixture(readAdminFixture(name));
    const envelope = requireDecoded(decode(PLAIN_DETAIL_ENVELOPE, fixture), name);
    return requireDecoded(decodePlainDetail(envelope), name).detail;
  };
  for (const [name, failure, hasFence] of [
    ["delivery-invalid-payload", "invalid_payload", false],
    ["delivery-preflight-blocked", "preflight_blocked", true],
    ["delivery-settlement-returns", "recovery_superseded", false],
  ] as const) {
    const decoded = decodeFixture(name);
    expect(decoded.delivery.failure_code).toBe(failure);
    expect(decoded.delivery.sending_at !== null).toBe(hasFence);
  }

  const take = decodeFixture("delivery-return-take");
  expect(take.delivery.origin_kind).toBe("settlement_return_action");
  expect(take.delivery.status).toBe("pending");

  const ordinary = jsonObject(
    parseFixture(readAdminFixture("delivery-invalid-payload")),
    "普通详情信封",
  );
  const invalidCode = requireDecoded(
    decode(PLAIN_DETAIL_ENVELOPE, { ...ordinary, code: "future_code" }),
    "普通详情信封",
  );
  expect(decodePlainDetail(invalidCode).ok).toBe(false);
  expect(decode(PLAIN_DETAIL_ENVELOPE, withoutField(ordinary, "message")).ok).toBe(false);

  const excludedText = readAdminFixture("error-delivery-projection-excluded");
  expect(adminApiFailureFromHttpBody(404, excludedText).code).toBe("delivery_projection_excluded");

  const body = parseFixture(readAdminFixture("delivery-disposition-actions"));
  expect(decode(PLAIN_DETAIL_ENVELOPE, body).ok).toBe(false);
  const payload = requireDecoded(decode(DETAIL_WITH_ACTIONS_ENVELOPE, body), "处置准入信封");
  expect(decodeDetailWithActions(payload, "00000000-0000-4000-8000-00000000002b", false).ok).toBe(
    true,
  );
  expect(decodeDetailWithActions(payload, OTHER_ID, false).ok).toBe(false);
});
