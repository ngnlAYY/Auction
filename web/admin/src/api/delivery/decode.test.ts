// Rust `delivery/decode.rs` 与 `delivery/tests.rs` 的投递投影/解码回归。

import { expect, test } from "bun:test";
import {
  AdminDeliveryListResponseSchema,
  AdminDeliveryResponseSchema,
  AdminDeliverySentProofResponseSchema,
  adminDeliveryActionAvailabilityBlocker,
} from "@auction/contracts/dto/admin/delivery";
import { decode } from "@auction/contracts/wire";

import {
  decodeDelivery,
  decodeDeliveryCommand,
  decodeDeliveryList,
  strictDeliveryEnvelopeSchema,
} from "./decode";
import {
  checkBeforeSendProjection,
  DELIVERY_COMMAND_ENVELOPE,
  DELIVERY_ID,
  DETAIL_WITH_ACTIONS_ENVELOPE,
  decodeDetailFixture,
  delivery,
  detail,
  jsonObject,
  OTHER_ID,
  parseFixture,
  preparationFailure,
  readAdminFixture,
  readDeliveryFixture,
  requireDecoded,
  requireElement,
  retiredBeforeSend,
  v018AcceptsMailIntakeFailure,
  withoutField,
} from "./delivery-fixture.test";
import { decodeConfirmation, decodeDetailWithActions } from "./responses";

function projectedDelivery(fixture: unknown) {
  const response = jsonObject(fixture, "详情信封");
  const data = jsonObject(response.data, "data");
  const detail = jsonObject(data.detail, "data.detail");
  return jsonObject(detail.delivery, "data.detail.delivery");
}

function deliveryProjectionDecodes(value: unknown): boolean {
  const parsed = decode(AdminDeliveryResponseSchema, value);
  if (!parsed.ok) {
    return false;
  }
  return decodeDelivery(parsed.value).ok;
}

test("delivery_origin_closed_set_covers_player_notifications", () => {
  for (const kind of [
    "settlement",
    "settlement_recovery_action",
    "settlement_return_action",
    "user_key",
    "manual",
    "mail_intake",
    "operations_alert",
    "lot_watch",
    "seller_summary",
    "season_reminder",
  ]) {
    const candidate = delivery();
    candidate.origin_kind = kind;
    expect(deliveryProjectionDecodes(candidate)).toBe(true);
  }
  for (const kind of ["", "seller_summary_x", "SELLER_SUMMARY"]) {
    const candidate = delivery();
    candidate.origin_kind = kind;
    expect(deliveryProjectionDecodes(candidate)).toBe(false);
  }
});

test("accepts_complete_detail", () => {
  const decoded = requireDecoded(decodeDetailFixture(detail()), "完整详情");
  expect(decoded.detail.attempts).toHaveLength(1);
});

test("delivery_accepts_server_preflight_retirements", () => {
  for (const [origin, reason, failure] of [
    ["lot_watch", "lot_watch_invalidated", "lot_watch_invalidated"],
    ["lot_watch", "recipient_renamed", "recipient_renamed"],
    ["seller_summary", "recipient_renamed", "recipient_renamed"],
    ["season_reminder", "recipient_renamed", "recipient_renamed"],
    ["season_reminder", "season_boundary_passed", "season_notice_invalidated"],
    ["season_reminder", "season_reminder_resolved", "season_notice_invalidated"],
    ["mail_intake", "auction_opened", "auction_notice_invalidated"],
  ] as const) {
    expect(checkBeforeSendProjection(retiredBeforeSend(origin, reason, failure))).toEqual({
      list: true,
      detail: true,
    });
  }

  const claimed = retiredBeforeSend(
    "season_reminder",
    "season_boundary_passed",
    "season_notice_invalidated",
  );
  claimed.claimed_at = "2026-11-06T10:00:01.000000Z";
  claimed.attempt_count = 1;
  claimed.fencing_version = 2;
  expect(checkBeforeSendProjection(claimed)).toEqual({ list: true, detail: true });
});

test("delivery_rejects_malformed_preflight_retirements", () => {
  const valid = retiredBeforeSend(
    "season_reminder",
    "season_boundary_passed",
    "season_notice_invalidated",
  );
  expect(checkBeforeSendProjection(valid)).toEqual({ list: true, detail: true });
  for (const [key, invalid] of [
    ["origin_kind", "seller_summary"],
    ["failure_code", "recipient_renamed"],
    ["completed_at", null],
    ["completed_at", "2026-11-06T10:00:06.000000Z"],
    ["claimed_at", "2026-11-06T10:00:01.000000Z"],
    ["external_reference", "3120869"],
    ["fencing_version", 0],
    ["cod", 1],
    ["retirement_reason", "season_reminder_resolved"],
    ["suppression_reason", "retry_exhausted"],
  ] as const) {
    const malformed = structuredClone(valid);
    malformed[key] = invalid;
    expect(checkBeforeSendProjection(malformed)).toEqual({ list: false, detail: false });
  }

  const sent = structuredClone(valid);
  sent.claimed_at = "2026-11-06T10:00:01.000000Z";
  sent.sending_at = "2026-11-06T10:00:02.000000Z";
  sent.attempt_count = 1;
  const missingReason = withoutField(sent, "reason_code");
  expect(checkBeforeSendProjection(missingReason)).toEqual({ list: false, detail: false });
  expect(checkBeforeSendProjection(withoutField(sent, "retirement_reason"))).toEqual({
    list: false,
    detail: false,
  });
  expect(checkBeforeSendProjection(withoutField(missingReason, "retirement_reason"))).toEqual({
    list: true,
    detail: true,
  });
});

test("delivery_decodes_actual_http_failure_and_action_projection", () => {
  const fixture = parseFixture(readDeliveryFixture("blocked_registration_reply"));
  const payload = requireDecoded(decode(DETAIL_WITH_ACTIONS_ENVELOPE, fixture), "处置准入信封");
  const decoded = requireDecoded(
    decodeDetailWithActions(payload, DELIVERY_ID, false),
    "HTTP 失败投影",
  );
  expect(adminDeliveryActionAvailabilityBlocker(decoded.actions.retry_failed)).toBe(
    "recipient_renamed",
  );
  expect(adminDeliveryActionAvailabilityBlocker(decoded.actions.acknowledge)).toBeNull();
  expect(checkBeforeSendProjection(projectedDelivery(fixture))).toEqual({
    list: true,
    detail: true,
  });
  expect(decodeDetailWithActions(payload, OTHER_ID, false).ok).toBe(false);

  for (const [field, invalid] of [
    ["retry_failed", { state: "blocked", reason: "future_reason" }],
    ["release", { state: "blocked" }],
    ["acknowledge", { state: "eligible", reason: "already_disposed" }],
    ["reconcile_retry", null],
  ] as const) {
    const malformed = jsonObject(structuredClone(fixture), "详情信封");
    const data = jsonObject(malformed.data, "data");
    jsonObject(data.actions, "data.actions")[field] = invalid;
    expect(decode(DETAIL_WITH_ACTIONS_ENVELOPE, malformed).ok).toBe(false);
  }
});

test("delivery_decodes_new_blockers_serialized_by_http", () => {
  const fixture = jsonObject(
    parseFixture(readDeliveryFixture("blocked_registration_reply")),
    "详情信封",
  );
  const blockers = parseFixture(readDeliveryFixture("disposition_action_blockers"));
  if (!Array.isArray(blockers)) {
    throw new Error("处置阻断原因夹具必须是数组");
  }
  const actions = jsonObject(jsonObject(fixture.data, "data").actions, "data.actions");
  actions.retry_failed = requireElement(blockers, 0, "处置阻断原因");
  actions.reconcile_failed = requireElement(blockers, 1, "处置阻断原因");
  actions.release = requireElement(blockers, 2, "处置阻断原因");
  const payload = requireDecoded(decode(DETAIL_WITH_ACTIONS_ENVELOPE, fixture), "处置准入信封");
  const decoded = requireDecoded(
    decodeDetailWithActions(payload, DELIVERY_ID, false),
    "HTTP 新增阻断原因",
  );
  expect(adminDeliveryActionAvailabilityBlocker(decoded.actions.retry_failed)).toBe(
    "settlement_fulfillment_state",
  );
  expect(adminDeliveryActionAvailabilityBlocker(decoded.actions.reconcile_failed)).toBe(
    "missing_sending_fence",
  );
  expect(adminDeliveryActionAvailabilityBlocker(decoded.actions.release)).toBe(
    "delivery_command_snapshot_unreadable",
  );
  expect(adminDeliveryActionAvailabilityBlocker(decoded.actions.acknowledge)).toBeNull();
});

test("login_key_reply_failure_shapes_keep_v018_list_compatibility", () => {
  const readHttpProjection = (name: string) => {
    const fixture = parseFixture(readDeliveryFixture(name));
    const row = projectedDelivery(fixture);
    expect(checkBeforeSendProjection(row)).toEqual({ list: true, detail: true });
    const payload = requireDecoded(decode(DETAIL_WITH_ACTIONS_ENVELOPE, fixture), "处置准入信封");
    expect(decodeDetailWithActions(payload, DELIVERY_ID, false).ok).toBe(true);
    expect(v018AcceptsMailIntakeFailure(row)).toBe(true);
    return row;
  };
  const afterFence = readHttpProjection("blocked_registration_reply");
  const beforeFence = readHttpProjection("blocked_registration_reply_before_fence");
  afterFence.sending_at = null;
  expect(v018AcceptsMailIntakeFailure(afterFence)).toBe(false);

  beforeFence.retirement_reason = "recipient_renamed";
  beforeFence.reason_code = "recipient_renamed";
  delete beforeFence.suppression_reason;
  beforeFence.failure_code = "recipient_renamed";
  expect(v018AcceptsMailIntakeFailure(beforeFence)).toBe(false);
  expect(checkBeforeSendProjection(beforeFence)).toEqual({ list: false, detail: false });
});

test("v018_login_key_reply_projection_rejects_metadata_outside_release_boundaries", () => {
  const fixture = parseFixture(readDeliveryFixture("blocked_registration_reply_before_fence"));
  const original = projectedDelivery(fixture);
  const boundary = structuredClone(original);
  boundary.template = "a".repeat(64);
  boundary.cod = 9_007_199_254_740_991;
  expect(v018AcceptsMailIntakeFailure(boundary)).toBe(true);

  for (const [field, invalid] of [
    ["template", "界".repeat(22)],
    ["category", "future_category"],
    ["world", "future_world"],
    ["cod", -1],
    ["cod", 9_007_199_254_740_992],
  ] as const) {
    const row = structuredClone(original);
    row[field] = invalid;
    expect(v018AcceptsMailIntakeFailure(row)).toBe(false);
  }
});

test("v018_login_key_reply_projection_rejects_zero_times_at_the_release_boundaries", () => {
  const fixture = parseFixture(readDeliveryFixture("blocked_registration_reply_before_fence"));
  const original = projectedDelivery(fixture);
  const beforeZero = "0000-12-31T23:59:59Z";
  const zero = "0001-01-01T00:00:00Z";
  for (const [field, earlierFields] of [
    ["created_at", []],
    ["updated_at", ["created_at", "claimed_at", "completed_at"]],
    ["claimed_at", ["created_at"]],
    ["completed_at", ["created_at", "claimed_at"]],
  ] as const) {
    const row = structuredClone(original);
    for (const earlier of earlierFields) {
      row[earlier] = beforeZero;
    }
    expect(v018AcceptsMailIntakeFailure(row)).toBe(true);
    row[field] = zero;
    expect(v018AcceptsMailIntakeFailure(row)).toBe(false);
  }
});

test("delivery_rejects_suppression_outside_its_projected_reason_and_state", () => {
  const prepared = preparationFailure("invalid_payload");
  expect(checkBeforeSendProjection(prepared)).toEqual({ list: true, detail: true });
  for (const label of ["unknown_reason", "retry_exhausted"]) {
    const mislabeled = structuredClone(prepared);
    mislabeled.suppression_reason = label;
    expect(checkBeforeSendProjection(mislabeled)).toEqual({ list: false, detail: false });
  }

  const sent = delivery();
  sent.status = "failed";
  sent.failure_code = "retry_exhausted";
  expect(checkBeforeSendProjection(sent)).toEqual({ list: true, detail: true });
  sent.suppression_reason = "retry_exhausted";
  expect(checkBeforeSendProjection(sent)).toEqual({ list: false, detail: false });

  const pending = structuredClone(prepared);
  pending.status = "pending";
  pending.failure_code = null;
  pending.claimed_at = null;
  pending.completed_at = null;
  pending.reason_code = "invalid_payload";
  expect(checkBeforeSendProjection(withoutField(pending, "suppression_reason"))).toEqual({
    list: true,
    detail: true,
  });
  expect(checkBeforeSendProjection(pending)).toEqual({ list: false, detail: false });
});

test("delivery_lists_decode_backend_contract_fixtures", () => {
  for (const [name, count] of [
    ["deliveries", 2],
    ["deliveries-settlement-returns", 4],
  ] as const) {
    const envelope = requireDecoded(
      decode(
        strictDeliveryEnvelopeSchema(AdminDeliveryListResponseSchema),
        parseFixture(readAdminFixture(name)),
      ),
      name,
    );
    const { deliveries } = requireDecoded(decodeDeliveryList(envelope, 1, 50), name);
    expect(deliveries).toHaveLength(count);
    expect(deliveries.some((row) => row.failure_code === "preflight_blocked")).toBe(true);
    expect(deliveries.some((row) => row.failure_code === "invalid_payload")).toBe(true);
    expect(deliveries.some((row) => row.failure_code === "recovery_superseded")).toBe(count === 4);
    expect(deliveries.some((row) => row.origin_kind === "settlement_return_action")).toBe(
      count === 4,
    );
  }
});

test("delivery_release_reconcile_and_sent_proof_contract_fixtures", () => {
  for (const [name, id, action, status, externalReference, failureCode] of [
    [
      "delivery-release-command",
      "00000000-0000-4000-8000-000000000031",
      "release",
      "pending",
      null,
      null,
    ],
    [
      "delivery-reconcile-succeeded-command",
      "00000000-0000-4000-8000-000000000032",
      "reconcile_succeeded",
      "succeeded",
      "mooglemail:main:take:4244",
      null,
    ],
    [
      "delivery-reconcile-failed-command",
      "00000000-0000-4000-8000-000000000032",
      "reconcile_failed",
      "failed",
      null,
      "verified_not_sent",
    ],
  ] as const) {
    const fixture = parseFixture(readAdminFixture(name));
    const decodeReceipt = (value: unknown) => {
      const parsed = decode(DELIVERY_COMMAND_ENVELOPE, value);
      return parsed.ok ? decodeDeliveryCommand(parsed.value, id, action, 4) : parsed;
    };
    const receipt = requireDecoded(decodeReceipt(fixture), name);
    expect(receipt.status).toBe(status);
    expect(receipt.fencing_version).toBe(5);
    expect(receipt.external_reference).toBe(externalReference);
    expect(receipt.failure_code).toBe(failureCode);
    const missingFence = withoutField(fixture, "data", "delivery", "fencing_version");
    expect(decode(DELIVERY_COMMAND_ENVELOPE, missingFence).ok).toBe(false);
    expect(decodeReceipt(missingFence).ok).toBe(false);

    const parsed = requireDecoded(decode(DELIVERY_COMMAND_ENVELOPE, fixture), name);
    expect(decodeDeliveryCommand(parsed, id, action, 5).ok).toBe(false);
    const wrongAction =
      action === "reconcile_succeeded" ? "reconcile_failed" : "reconcile_succeeded";
    expect(decodeDeliveryCommand(parsed, id, wrongAction, 4).ok).toBe(false);
  }

  const commandId = "00000000-0000-4000-8000-000000000007";
  const fixture = parseFixture(readAdminFixture("delivery-sent-proof-command"));
  const schema = strictDeliveryEnvelopeSchema(AdminDeliverySentProofResponseSchema);
  const decodeReceipt = (value: unknown) => {
    const payload = decode(schema, value);
    return payload.ok ? decodeConfirmation(payload.value, commandId, DELIVERY_ID) : payload;
  };
  const confirmation = requireDecoded(decodeReceipt(fixture), "Sent 确认回执");
  expect(confirmation.external_reference).toBe("mooglemail:main:send:7000001");
  expect(confirmation.replayed).toBe(false);
  const missingCommand = withoutField(fixture, "data", "command_id");
  expect(decode(schema, missingCommand).ok).toBe(false);
  expect(decodeReceipt(missingCommand).ok).toBe(false);
});
