// 验证寄件指引、预约请求/回执与第 4 期拒绝码共享矩阵。

import { expect, expectTypeOf, test } from "bun:test";

import * as v from "valibot";

import { decode, integer, strictStruct } from "../../../wire";
import { readFixture, readResponse } from "./consignment-fixture";
import {
  DepositInstructionsSchema,
  DepositReservationCodeSchema,
  DepositReservationReceiptSchema,
  type IssueDepositReservation,
  IssueDepositReservationSchema,
} from "./deposit";
import {
  REQUEST_ERROR_HTTP_STATUS,
  RequestErrorCodeV2Schema,
  WRITE_REJECTION_HTTP_STATUS,
  WRITE_REJECTION_OUTCOME_UNKNOWN,
  WRITE_REJECTION_TRUSTED_BEFORE_WRITE,
  WriteRejectionCodeV2Schema,
} from "./receipt";

const NONE = "public/consignment-deposit-instructions-none.json";
const ACTIVE = "public/consignment-deposit-instructions-active.json";
const REQUEST = "public/consignment-deposit-reservation-request.json";
const RECEIPT = "public/consignment-deposit-reservation-receipt.json";
const REJECTIONS = "public/consignment-write-rejections-v2.json";

const VALID_CODE = "C1-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

test("寄件指引只允许 none 或带全部字段的 active", async () => {
  const none = await readResponse(NONE, DepositInstructionsSchema);
  const active = await readResponse(ACTIVE, DepositInstructionsSchema);
  expect(none.data).toEqual({ state: "none" });
  expect(active.data).toMatchObject({
    state: "active",
    world: "main",
    bot: "FixtureBot",
    subject: "consign",
    reservation_id: "00000000-0000-4000-8000-000000000301",
  });
  expect((active.data as { body: string }).body).toHaveLength(46);
  expect(decode(DepositInstructionsSchema, { ...none.data, body: VALID_CODE }).ok).toBe(false);
  expect(decode(DepositInstructionsSchema, { ...none.data, world: "main" }).ok).toBe(false);
});

test("active 缺少任一字段即失败", async () => {
  const { data: active } = await readResponse(ACTIVE, DepositInstructionsSchema);
  for (const missing of [
    "world",
    "bot",
    "subject",
    "body",
    "reservation_id",
    "expires_at",
  ] as const) {
    const incomplete: Record<string, unknown> = { ...active };
    delete incomplete[missing];
    expect(decode(DepositInstructionsSchema, incomplete).ok).toBe(false);
  }
});

test("预约码要求 C1- 加 43 个规范 base64url 字符", () => {
  expect(decode(DepositReservationCodeSchema, VALID_CODE).ok).toBe(true);
  for (const invalid of [
    `${VALID_CODE}A`,
    "C1-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
    "c1-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
    "C1-BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB",
    "C1-===========================================",
  ]) {
    expect(decode(DepositReservationCodeSchema, invalid).ok).toBe(false);
  }
});

test("active 指引拒绝非法世界、主题、Bot、UUID 与时刻", async () => {
  const { data: active } = await readResponse(ACTIVE, DepositInstructionsSchema);
  for (const patch of [
    { world: "other" },
    { bot: "" },
    { subject: "Consign" },
    { subject: "consign " },
    { reservation_id: "00000000-0000-0000-0000-000000000000" },
    { reservation_id: "00000000-0000-4000-8000-00000000030A" },
    { expires_at: "2026-11-27T18:10:00-08:00" },
    { expires_at: null },
    { unexpected: true },
  ]) {
    expect(decode(DepositInstructionsSchema, { ...active, ...patch }).ok).toBe(false);
  }
});

test("active 指引的 bot 使用与 Rust 相同的字节与单行规则", async () => {
  const { data: active } = await readResponse(ACTIVE, DepositInstructionsSchema);
  expect(decode(DepositInstructionsSchema, { ...active, bot: "FixtureBot" }).ok).toBe(true);
  for (const bot of [" FixtureBot", "FixtureBot ", "Fixture\nBot", "é".repeat(128)]) {
    expect(decode(DepositInstructionsSchema, { ...active, bot }).ok).toBe(false);
  }
});

test("预约请求只接受 UUIDv4 与正版本", async () => {
  expectTypeOf<
    v.InferOutput<typeof IssueDepositReservationSchema>
  >().toEqualTypeOf<IssueDepositReservation>();
  const request = await readFixture(REQUEST, IssueDepositReservationSchema);
  for (const command_id of [
    "00000000-0000-5000-8000-000000000001",
    "00000000-0000-0000-8000-000000000001",
    "00000000-0000-4000-8000-00000000000A",
    "00000000-0000-0000-0000-000000000000",
  ]) {
    expect(decode(IssueDepositReservationSchema, { ...request, command_id }).ok).toBe(false);
  }
  for (const expected_version of [0, -1, 1.5, "1", Number.MAX_SAFE_INTEGER + 1]) {
    expect(decode(IssueDepositReservationSchema, { ...request, expected_version }).ok).toBe(false);
  }
});

test("预约回执与请求命令 ID 对齐", async () => {
  const request = await readFixture(REQUEST, IssueDepositReservationSchema);
  const receipt = await readResponse(RECEIPT, DepositReservationReceiptSchema);
  expect(receipt.data).toMatchObject({
    schema_version: 2,
    operation: "issue_reservation",
    disposition: "reservation_issued",
    command_id: request.command_id,
    state: "awaiting_deposit",
  });
  for (const schema_version of [1, 3]) {
    expect(decode(DepositReservationReceiptSchema, { ...receipt.data, schema_version }).ok).toBe(
      false,
    );
  }
  const missing = structuredClone(receipt.data) as Record<string, unknown>;
  delete missing.schema_version;
  expect(decode(DepositReservationReceiptSchema, missing).ok).toBe(false);
});

test("拒绝码共享矩阵与状态类一致", async () => {
  const matrix = await readFixture(
    REJECTIONS,
    v.array(
      strictStruct({
        code: v.string(),
        http_status: integer,
        outcome_unknown: v.boolean(),
        trusted_before_write: v.boolean(),
      }),
    ),
  );
  expect(matrix).toHaveLength(21);
  for (const entry of matrix) {
    expect(v.is(WriteRejectionCodeV2Schema, entry.code)).toBe(true);
    if (!v.is(WriteRejectionCodeV2Schema, entry.code)) {
      throw new Error("拒绝码矩阵含未知码");
    }
    const code = entry.code;
    expect(Number(WRITE_REJECTION_HTTP_STATUS[code])).toBe(entry.http_status);
    expect(WRITE_REJECTION_OUTCOME_UNKNOWN.has(code)).toBe(entry.outcome_unknown);
    expect(WRITE_REJECTION_TRUSTED_BEFORE_WRITE.has(code)).toBe(entry.trusted_before_write);
  }
  expect(matrix.some(({ code }) => code === "fence_mismatch")).toBe(true);
  expect(decode(WriteRejectionCodeV2Schema, "replay_maybe").ok).toBe(false);
});

test("请求错误是确定未写入的独立闭集", () => {
  expect(RequestErrorCodeV2Schema.options).toEqual([
    "invalid_request",
    "invalid_command_id",
    "invalid_listing_id",
    "invalid_listing_request",
    "invalid_equipment_url",
    "invalid_price",
    "invalid_reason",
    "invalid_query",
    "command_not_found",
  ]);
  for (const code of RequestErrorCodeV2Schema.options) {
    expect([400, 404]).toContain(REQUEST_ERROR_HTTP_STATUS[code]);
  }
  expect(RequestErrorCodeV2Schema.options).not.toContain("consignment_unavailable");
});
