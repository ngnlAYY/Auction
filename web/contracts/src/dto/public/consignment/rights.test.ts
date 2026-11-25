// 受限权利契约：解码与 Rust 共用的夹具，并验证闭集、UUIDv4 与动作-种类一致性。

import { expect, test } from "bun:test";

import { decode } from "../../../wire";
import { readFixture, readResponse } from "./consignment-fixture";
import {
  ChallengeRightRequestSchema,
  ClaimRightRequestSchema,
  LocateRightRequestSchema,
  RightChallengeAcceptedSchema,
  RightClaimReceiptSchema,
  RightVerifiedGrantSchema,
  RightViewSchema,
  VerifyRightRequestSchema,
} from "./rights";

const PREFIX = "public/consignment-rights-";

test("权利请求与响应夹具与 Rust 共用并可解码", async () => {
  await readFixture(`${PREFIX}locate-request.json`, LocateRightRequestSchema);
  await readFixture(`${PREFIX}challenge-request.json`, ChallengeRightRequestSchema);
  await readFixture(`${PREFIX}verify-request.json`, VerifyRightRequestSchema);
  await readFixture(`${PREFIX}claim-request.json`, ClaimRightRequestSchema);
  await readResponse(`${PREFIX}challenge-accepted.json`, RightChallengeAcceptedSchema);
  await readResponse(`${PREFIX}verified.json`, RightVerifiedGrantSchema);
  await readResponse(`${PREFIX}view.json`, RightViewSchema);
  await readResponse(`${PREFIX}claim-receipt.json`, RightClaimReceiptSchema);
});

test("续办请求是闭集：额外字段、非 v4 命令 ID 与未知动作都被拒绝", async () => {
  const request = await readFixture(`${PREFIX}claim-request.json`, ClaimRightRequestSchema);
  expect(decode(ClaimRightRequestSchema, { ...request, extra: 1 }).ok).toBe(false);
  expect(
    decode(ClaimRightRequestSchema, {
      ...request,
      command_id: "00000000-0000-5000-8000-000000000504",
    }).ok,
  ).toBe(false);
  expect(decode(ClaimRightRequestSchema, { ...request, action: "admin_override" }).ok).toBe(false);
});

test("权利视图拒绝与种类不符的动作", async () => {
  const { data } = await readResponse(`${PREFIX}view.json`, RightViewSchema);
  expect(decode(RightViewSchema, { ...data, allowed_actions: ["claim_equipment"] }).ok).toBe(false);
  expect(decode(RightViewSchema, { ...data, kind: "loss_case" }).ok).toBe(false);
});
