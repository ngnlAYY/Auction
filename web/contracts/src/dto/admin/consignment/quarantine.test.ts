// 验证 Admin 隔离详情、六种解除分支、重查请求与止售回执。

import { expect, expectTypeOf, test } from "bun:test";

import * as v from "valibot";

import { decode, struct } from "../../../wire";
import { readFixture, readResponse } from "../../public/consignment/consignment-fixture";
import {
  ConsignmentDelistReceiptV2Schema,
  ConsignmentHoldReceiptSchema,
  QuarantineDecisionKindSchema,
  QuarantineDetailSchema,
  QuarantineObservationItemKindSchema,
  QuarantineReasonSchema,
  QuarantineRecheckReceiptSchema,
  type QuarantineRecheckRequest,
  QuarantineRecheckRequestSchema,
  QuarantineReleaseOutcomeSchema,
  QuarantineReleaseReceiptSchema,
  type QuarantineReleaseRequest,
  QuarantineReleaseRequestSchema,
  QuarantineStateSchema,
  ReleaseConsignmentHoldRequestSchema,
} from "./quarantine";

const ACTIVE = "admin/consignment-quarantine-active.json";
const RELEASED = "admin/consignment-quarantine-released.json";
const RECHECK_REQUEST = "admin/consignment-quarantine-recheck-request.json";
const RECHECK_RECEIPT = "admin/consignment-quarantine-recheck-receipt.json";
const RELEASE_REQUEST = "admin/consignment-quarantine-release-request.json";
const HOLD_SET = "admin/consignment-hold-set-receipt.json";
const HOLD_RELEASED = "admin/consignment-hold-released-receipt.json";
const DELIST_CLOSED = "admin/consignment-delist-closed-receipt.json";
const ENUMS = "public/consignment-enums-v2.json";

function record(value: unknown): Record<string, unknown> {
  return value as Record<string, unknown>;
}

test("活动隔离详情返回两条脱敏观察与可执行动作", async () => {
  const { data: detail } = await readResponse(ACTIVE, QuarantineDetailSchema);
  expect(detail).toMatchObject({
    version: 3,
    state: "active",
    world: "isekai",
    external_mid: "42",
    reason: "list_detail_conflict",
    observation_total: 2,
    allowed_actions: ["recheck", "release"],
    release: null,
  });
  expect(detail.observations).toHaveLength(2);
  expect(detail.observations[0]?.digest).toHaveLength(64);
  expect(detail.observations[1]?.items[1]).toEqual({
    ordinal: 2,
    kind: "unknown",
    external_equipment_id: null,
    amount: null,
  });
});

test("已解除隔离返回最终决定与原 owner 引用", async () => {
  const { data: detail } = await readResponse(RELEASED, QuarantineDetailSchema);
  expect(detail.state).toBe("released");
  expect(detail.allowed_actions).toEqual([]);
  expect(detail.release).toMatchObject({
    outcome: {
      decision_kind: "auction_return_case",
      return_mail_id: "00000000-0000-4000-8000-000000000408",
      return_evidence_id: "00000000-0000-4000-8000-000000000409",
      obligation_id: null,
    },
  });
});

test("隔离详情拒绝状态、观察与解除记录矛盾", async () => {
  const { data: active } = await readResponse(ACTIVE, QuarantineDetailSchema);
  const { data: released } = await readResponse(RELEASED, QuarantineDetailSchema);
  const activeWire = record(structuredClone(active));
  const releasedWire = record(structuredClone(released));
  expect(
    decode(QuarantineDetailSchema, {
      ...activeWire,
      release: releasedWire.release,
    }).ok,
  ).toBe(false);
  expect(decode(QuarantineDetailSchema, { ...releasedWire, release: null }).ok).toBe(false);
  expect(
    decode(QuarantineDetailSchema, {
      ...releasedWire,
      allowed_actions: ["recheck"],
    }).ok,
  ).toBe(false);
  expect(
    decode(QuarantineDetailSchema, {
      ...activeWire,
      allowed_actions: ["recheck", "recheck"],
    }).ok,
  ).toBe(false);
  expect(decode(QuarantineDetailSchema, { ...activeWire, allowed_actions: ["publish"] }).ok).toBe(
    false,
  );
  expect(decode(QuarantineDetailSchema, { ...activeWire, external_mid: "042" }).ok).toBe(false);
  expect(decode(QuarantineDetailSchema, { ...activeWire, version: 0 }).ok).toBe(false);
  expect(
    decode(QuarantineDetailSchema, {
      ...releasedWire,
      release: {
        ...record(released.release),
        decision_id: releasedWire.first_decision_id,
      },
    }).ok,
  ).toBe(false);
});

test("观察数量、序号、金额、摘要与解析器版本严格", async () => {
  const { data: active } = await readResponse(ACTIVE, QuarantineDetailSchema);
  const activeWire = record(structuredClone(active));
  expect(decode(QuarantineDetailSchema, { ...activeWire, observation_total: 250 }).ok).toBe(true);
  expect(decode(QuarantineDetailSchema, { ...activeWire, observation_total: 1 }).ok).toBe(false);
  expect(decode(QuarantineDetailSchema, { ...activeWire, observation_total: -1 }).ok).toBe(false);
  const observations = structuredClone(active.observations) as Record<string, unknown>[];
  const first = observations[0];
  if (first === undefined) throw new Error("夹具须有观察");
  const patches: Record<string, unknown>[] = [
    { attachment_count: 2 },
    { digest: "A".repeat(64) },
    { parser_version: "" },
    { credits: -1 },
  ];
  for (const patch of patches) {
    expect(
      decode(QuarantineDetailSchema, {
        ...activeWire,
        observations: [{ ...first, ...patch }, observations[1]],
      }).ok,
    ).toBe(false);
  }
  const secondItems = structuredClone(observations[1]?.items) as Record<string, unknown>[];
  expect(
    decode(QuarantineDetailSchema, {
      ...activeWire,
      observations: [
        first,
        {
          ...observations[1],
          items: [secondItems[0], { ...secondItems[1], ordinal: 1 }],
        },
      ],
    }).ok,
  ).toBe(false);
  expect(
    decode(QuarantineDetailSchema, {
      ...activeWire,
      observations: [
        first,
        {
          ...observations[1],
          items: [secondItems[0], { ...secondItems[1], amount: 0 }],
        },
      ],
    }).ok,
  ).toBe(false);
});

test("重查请求与回执共享隔离 ID 并强制 UUIDv4", async () => {
  expectTypeOf<
    v.InferOutput<typeof QuarantineRecheckRequestSchema>
  >().toEqualTypeOf<QuarantineRecheckRequest>();
  const request = await readFixture(RECHECK_REQUEST, QuarantineRecheckRequestSchema);
  const receipt = await readResponse(RECHECK_RECEIPT, QuarantineRecheckReceiptSchema);
  expect(receipt.data).toMatchObject({
    schema_version: 2,
    operation: "recheck_quarantine",
    disposition: "recheck_queued",
    command_id: request.command_id,
    quarantine_id: "00000000-0000-4000-8000-000000000401",
  });
  for (const command_id of [
    "00000000-0000-5000-8000-000000000001",
    "00000000-0000-0000-8000-000000000001",
    "00000000-0000-4000-8000-00000000000A",
  ]) {
    expect(decode(QuarantineRecheckRequestSchema, { ...request, command_id }).ok).toBe(false);
  }
  for (const expected_version of [0, -1, 1.5, "3"]) {
    expect(decode(QuarantineRecheckRequestSchema, { ...request, expected_version }).ok).toBe(false);
  }
  for (const schema_version of [1, 3]) {
    expect(decode(QuarantineRecheckReceiptSchema, { ...receipt.data, schema_version }).ok).toBe(
      false,
    );
  }
});

test("解除请求不允许 force 或未知决定种类", async () => {
  expectTypeOf<
    v.InferOutput<typeof QuarantineReleaseRequestSchema>
  >().toEqualTypeOf<QuarantineReleaseRequest>();
  const request = await readFixture(RELEASE_REQUEST, QuarantineReleaseRequestSchema);
  expect(request.proposed_kind).toBe("auction_return_case");
  for (const patch of [
    { proposed_kind: "force" },
    { force: true },
    { observation_id: "00000000-0000-0000-0000-000000000000" },
    { command_id: "00000000-0000-5000-8000-000000000001" },
  ]) {
    expect(decode(QuarantineReleaseRequestSchema, { ...request, ...patch }).ok).toBe(false);
  }
});

test("六种解除回执分支都能从共享夹具解码", async () => {
  for (const file of [
    "admin/consignment-quarantine-release-accept.json",
    "admin/consignment-quarantine-release-return.json",
    "admin/consignment-quarantine-release-archive-empty.json",
    "admin/consignment-quarantine-release-acquire-return.json",
    "admin/consignment-quarantine-release-auction-return-case-unbound.json",
    "admin/consignment-quarantine-release-auction-return-case-bound.json",
    "admin/consignment-quarantine-release-auction-recovery-return.json",
  ]) {
    const response = await readResponse(file, QuarantineReleaseReceiptSchema);
    expect(response.data.schema_version).toBe(2);
    expect(response.data.operation).toBe("release_quarantine");
    expect(response.data.disposition).toBe("quarantine_released");
  }
});

test("Admin v2 回执必须携带固定 schema 2", async () => {
  const release = await readResponse(
    "admin/consignment-quarantine-release-return.json",
    QuarantineReleaseReceiptSchema,
  );
  const recheck = await readResponse(RECHECK_RECEIPT, QuarantineRecheckReceiptSchema);
  const hold = await readResponse(HOLD_SET, ConsignmentHoldReceiptSchema);
  for (const [schema, wire] of [
    [QuarantineReleaseReceiptSchema, release.data],
    [QuarantineRecheckReceiptSchema, recheck.data],
    [ConsignmentHoldReceiptSchema, hold.data],
  ] as const) {
    for (const schemaVersion of [1, 3]) {
      expect(decode(schema, { ...record(wire), schema_version: schemaVersion }).ok).toBe(false);
    }
    const missing = record(structuredClone(wire));
    delete missing.schema_version;
    expect(decode(schema, missing).ok).toBe(false);
  }
});

test("拍卖案件分支的 obligation_id 可空且两种形状都接受", async () => {
  const unbound = await readResponse(
    "admin/consignment-quarantine-release-auction-return-case-unbound.json",
    QuarantineReleaseReceiptSchema,
  );
  const bound = await readResponse(
    "admin/consignment-quarantine-release-auction-return-case-bound.json",
    QuarantineReleaseReceiptSchema,
  );
  const unboundData = record(structuredClone(unbound.data));
  const boundData = record(structuredClone(bound.data));
  expect(unboundData.obligation_id).toBeNull();
  expect(boundData.obligation_id).toBe("00000000-0000-4000-8000-000000000410");
  expect(
    decode(QuarantineReleaseReceiptSchema, {
      ...unboundData,
      obligation_id: "00000000-0000-4000-8000-000000000410",
    }).ok,
  ).toBe(true);
});

test("解除回执拒绝分支字段错配与缺失", async () => {
  const accept = await readResponse(
    "admin/consignment-quarantine-release-accept.json",
    QuarantineReleaseReceiptSchema,
  );
  const caseUnbound = await readResponse(
    "admin/consignment-quarantine-release-auction-return-case-unbound.json",
    QuarantineReleaseReceiptSchema,
  );
  const recovery = await readResponse(
    "admin/consignment-quarantine-release-auction-recovery-return.json",
    QuarantineReleaseReceiptSchema,
  );
  expect(
    decode(QuarantineReleaseReceiptSchema, {
      ...record(structuredClone(recovery.data)),
      obligation_id: null,
    }).ok,
  ).toBe(false);
  expect(
    decode(QuarantineReleaseOutcomeSchema, {
      decision_kind: "auction_recovery_return",
      obligation_id: null,
      return_mail_id: "00000000-0000-4000-8000-000000000408",
      recovery_id: "00000000-0000-4000-8000-000000000411",
      recovery_handoff_id: "00000000-0000-4000-8000-000000000412",
    }).ok,
  ).toBe(false);
  const acquire = await readResponse(
    "admin/consignment-quarantine-release-acquire-return.json",
    QuarantineReleaseReceiptSchema,
  );
  const acceptWire = record(structuredClone(accept.data));
  expect(
    decode(QuarantineReleaseReceiptSchema, {
      ...acceptWire,
      return_mail_id: "00000000-0000-4000-8000-000000000408",
    }).ok,
  ).toBe(false);
  expect(
    decode(QuarantineReleaseReceiptSchema, {
      ...record(structuredClone(caseUnbound.data)),
      return_intake_id: "00000000-0000-4000-8000-000000000413",
    }).ok,
  ).toBe(false);
  expect(
    decode(QuarantineReleaseReceiptSchema, {
      ...record(structuredClone(recovery.data)),
      take_work_id: "00000000-0000-4000-8000-000000000414",
    }).ok,
  ).toBe(false);

  const acceptListings = acceptWire.listing_changes as Record<string, unknown>[];
  expect(decode(QuarantineReleaseReceiptSchema, { ...acceptWire, listing_changes: [] }).ok).toBe(
    false,
  );
  expect(
    decode(QuarantineReleaseReceiptSchema, {
      ...acceptWire,
      listing_changes: [{ ...acceptListings[0], state: "available" }],
    }).ok,
  ).toBe(false);
  expect(
    decode(QuarantineReleaseReceiptSchema, {
      ...acceptWire,
      listing_changes: [
        acceptListings[0],
        { ...acceptListings[1], listing_id: acceptListings[0]?.listing_id },
      ],
    }).ok,
  ).toBe(false);
  for (const missing of [
    "return_intake_id",
    "original_round",
    "take_work_id",
    "take_delivery_id",
  ]) {
    const incomplete: Record<string, unknown> = { ...record(acquire.data) };
    delete incomplete[missing];
    expect(decode(QuarantineReleaseReceiptSchema, incomplete).ok).toBe(false);
  }
  for (const missing of ["return_mail_id", "return_evidence_id", "obligation_id"]) {
    const incomplete: Record<string, unknown> = { ...record(caseUnbound.data) };
    delete incomplete[missing];
    expect(decode(QuarantineReleaseReceiptSchema, incomplete).ok).toBe(false);
  }
  for (const missing of ["obligation_id", "return_mail_id", "recovery_id", "recovery_handoff_id"]) {
    const incomplete: Record<string, unknown> = { ...record(recovery.data) };
    delete incomplete[missing];
    expect(decode(QuarantineReleaseReceiptSchema, incomplete).ok).toBe(false);
  }
  expect(
    decode(QuarantineReleaseReceiptSchema, {
      ...acceptWire,
      disposition: "hold_set",
    }).ok,
  ).toBe(false);
  expect(
    decode(QuarantineReleaseReceiptSchema, {
      ...acceptWire,
      quarantine_id: "00000000-0000-0000-0000-000000000000",
    }).ok,
  ).toBe(false);
});

test("止售回执绑定 operation、处置、状态与 hold ID", async () => {
  const set = await readResponse(HOLD_SET, ConsignmentHoldReceiptSchema);
  const released = await readResponse(HOLD_RELEASED, ConsignmentHoldReceiptSchema);
  expect(set.data).toMatchObject({
    operation: "admin_delist",
    disposition: "hold_set",
    state: "held_off_market",
  });
  expect(released.data).toMatchObject({
    operation: "release_hold",
    disposition: "hold_released",
    state: "held_off_market",
  });
  const setWire = record(structuredClone(set.data));
  for (const patch of [
    { state: "available" },
    { disposition: "hold_released" },
    { hold_id: "00000000-0000-0000-0000-000000000000" },
  ]) {
    expect(decode(ConsignmentHoldReceiptSchema, { ...setWire, ...patch }).ok).toBe(false);
  }
  expect(
    decode(ConsignmentHoldReceiptSchema, {
      ...record(structuredClone(released.data)),
      state: "available",
    }).ok,
  ).toBe(false);
});

test("Admin 下架回执覆盖未代管关闭与代管件止售", async () => {
  const closed = await readResponse(DELIST_CLOSED, ConsignmentDelistReceiptV2Schema);
  expect(closed.data).toMatchObject({
    schema_version: 2,
    operation: "admin_delist",
    disposition: "closed",
    state: "closed",
  });
  const hold = await readResponse(HOLD_SET, ConsignmentDelistReceiptV2Schema);
  expect(hold.data).toMatchObject({
    schema_version: 2,
    operation: "admin_delist",
    disposition: "hold_set",
    state: "held_off_market",
  });
  const closedWire = record(structuredClone(closed.data));
  for (const patch of [
    { state: "available" },
    { disposition: "hold_set" },
    { hold_id: "00000000-0000-4000-8000-000000000416" },
    { schema_version: 1 },
  ]) {
    expect(decode(ConsignmentDelistReceiptV2Schema, { ...closedWire, ...patch }).ok).toBe(false);
  }
  const holdWire = record(structuredClone(hold.data));
  for (const patch of [
    { state: "closed" },
    { hold_id: "00000000-0000-0000-0000-000000000000" },
    { operation: "release_hold" },
  ]) {
    expect(decode(ConsignmentDelistReceiptV2Schema, { ...holdWire, ...patch }).ok).toBe(false);
  }
});

test("隔离相关枚举与共享夹具逐字一致", async () => {
  const enums = await readFixture(
    ENUMS,
    struct({
      quarantine_states: v.array(v.string()),
      quarantine_reasons: v.array(v.string()),
      observation_item_kinds: v.array(v.string()),
      decision_kinds: v.array(v.string()),
    }),
  );
  for (const value of enums.quarantine_states) {
    expect(v.is(QuarantineStateSchema, value)).toBe(true);
  }
  for (const value of enums.quarantine_reasons) {
    expect(v.is(QuarantineReasonSchema, value)).toBe(true);
  }
  expect(QuarantineReasonSchema.options).toHaveLength(12);
  expect(v.is(QuarantineReasonSchema, "manifest_conflict")).toBe(false);
  for (const value of enums.observation_item_kinds) {
    expect(v.is(QuarantineObservationItemKindSchema, value)).toBe(true);
  }
  for (const value of enums.decision_kinds) {
    expect(v.is(QuarantineDecisionKindSchema, value)).toBe(true);
  }
});

test("解除止售请求要求 UUIDv4、正版本与非零 hold", () => {
  const valid = {
    command_id: "00000000-0000-4000-8000-000000000001",
    expected_version: 2,
    hold_id: "00000000-0000-4000-8000-000000000002",
  };
  expect(decode(ReleaseConsignmentHoldRequestSchema, valid).ok).toBe(true);
  expect(
    decode(ReleaseConsignmentHoldRequestSchema, {
      ...valid,
      command_id: "00000000-0000-0000-0000-000000000001",
    }).ok,
  ).toBe(false);
  expect(
    decode(ReleaseConsignmentHoldRequestSchema, {
      ...valid,
      hold_id: "00000000-0000-0000-0000-000000000000",
    }).ok,
  ).toBe(false);
  expect(
    decode(ReleaseConsignmentHoldRequestSchema, {
      ...valid,
      force: true,
    }).ok,
  ).toBe(false);
});
