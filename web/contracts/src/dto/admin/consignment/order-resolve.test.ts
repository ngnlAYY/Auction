import { describe, expect, test } from "bun:test";
import * as v from "valibot";
import {
  OrderResolveActionSchema,
  RecheckOrderReceiptSchema,
  RedispatchQueuedReceiptSchema,
  ResolutionRecordedReceiptSchema,
  ResolveOrderRequestSchema,
} from "./order-resolve";
import { OrderEvidenceItemV2Schema } from "./order-v2";

const COMMAND = "00000000-0000-4000-8000-000000000001";
const CONTRACT = "00000000-0000-4000-8000-000000000002";
const EVIDENCE = "00000000-0000-4000-8000-000000000003";

const request = {
  command_id: COMMAND,
  expected_version: 3,
  action: "refund_lost",
  evidence_id: EVIDENCE,
};

describe("订单处置线协议", () => {
  test("动作是闭集，请求不接受金额、收件人或未知动作", () => {
    expect(OrderResolveActionSchema.options).toEqual([
      "cancel_unpaid",
      "refund_returned",
      "refund_lost",
      "refund_season",
      "retain_entitlement",
      "redispatch_original",
    ]);
    expect(v.is(ResolveOrderRequestSchema, request)).toBe(true);
    for (const action of ["refund", "REFUND_LOST", ""]) {
      expect(v.is(ResolveOrderRequestSchema, { ...request, action })).toBe(false);
    }
    for (const extra of ["amount", "recipient"]) {
      expect(v.is(ResolveOrderRequestSchema, { ...request, [extra]: 1 })).toBe(false);
    }
    expect(
      v.is(ResolveOrderRequestSchema, {
        ...request,
        command_id: "00000000-0000-1000-8000-000000000001",
      }),
    ).toBe(false);
  });

  test("回执按处置分形：补寄不带动作，处置记录不能是补寄", () => {
    const head = {
      schema_version: 2,
      operation: "resolve",
      command_id: COMMAND,
      contract_id: CONTRACT,
      contract_version: 4,
    };
    const recorded = {
      ...head,
      disposition: "resolution_recorded",
      action: "refund_lost",
      resolution_id: EVIDENCE,
    };
    expect(v.is(ResolutionRecordedReceiptSchema, recorded)).toBe(true);
    expect(
      v.is(ResolutionRecordedReceiptSchema, { ...recorded, action: "redispatch_original" }),
    ).toBe(false);
    expect(v.is(ResolutionRecordedReceiptSchema, { ...recorded, amount: 1 })).toBe(false);
    const queued = { ...head, disposition: "redispatch_queued", dispatch_id: EVIDENCE };
    expect(v.is(RedispatchQueuedReceiptSchema, queued)).toBe(true);
    expect(v.is(RedispatchQueuedReceiptSchema, { ...queued, action: "redispatch_original" })).toBe(
      false,
    );
    expect(
      v.is(RecheckOrderReceiptSchema, {
        ...head,
        operation: "recheck",
        disposition: "recheck_queued",
        work_id: EVIDENCE,
      }),
    ).toBe(true);
  });

  test("证据恢复段的三类条目只含 ID、动作码与时刻", () => {
    const at = "2026-11-24T00:00:00Z";
    const closure = { kind: "closure", closure_kind: "refund_closed", closed_at: at };
    const resolution = {
      kind: "resolution",
      resolution_id: EVIDENCE,
      action: "refund_lost",
      evidence_id: COMMAND,
      resolved_at: at,
    };
    const recheck = { kind: "recheck", work_id: EVIDENCE, requested_at: at };
    for (const item of [closure, resolution, recheck]) {
      expect(v.is(OrderEvidenceItemV2Schema, item)).toBe(true);
      expect(v.is(OrderEvidenceItemV2Schema, { ...item, amount: 1 })).toBe(false);
    }
    expect(v.is(OrderEvidenceItemV2Schema, { ...closure, closure_kind: "paid" })).toBe(false);
    expect(v.is(OrderEvidenceItemV2Schema, { ...resolution, action: "refund" })).toBe(false);
  });
});
