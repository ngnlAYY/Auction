// 验证首屏能力解码与既有契约导入路径使用同一个 schema。

import { expect, test } from "bun:test";

import { decode } from "../../../wire";
import { ConsignmentStatusSchema as FacadeStatusSchema } from ".";
import { ConsignmentStatusSchema as CatalogStatusSchema } from "./catalog";
import { readResponse } from "./consignment-fixture";
import { ConsignmentStatusSchema } from "./status";

test("既有门面与目录路径导出同一份状态 schema", () => {
  expect(FacadeStatusSchema).toBe(ConsignmentStatusSchema);
  expect(CatalogStatusSchema).toBe(ConsignmentStatusSchema);
});

test("状态夹具分别表示可登记、关闭与依赖不可用", async () => {
  const ready = await readResponse("public/consignment-status.json", ConsignmentStatusSchema);
  const disabled = await readResponse(
    "public/consignment-status-disabled.json",
    ConsignmentStatusSchema,
  );
  const unavailable = await readResponse(
    "public/consignment-status-unavailable.json",
    ConsignmentStatusSchema,
  );
  expect(ready.data).toMatchObject({
    enabled: true,
    registration_ready: true,
    registration_modes: ["direct", "consigned"],
    registration_worlds: ["main", "isekai"],
    accepts_mail: false,
    purchasable: false,
  });
  expect(disabled.data).toMatchObject({
    enabled: false,
    registration_ready: false,
    registration_modes: [],
    registration_worlds: [],
  });
  expect(unavailable.data).toMatchObject({
    enabled: true,
    registration_ready: false,
    registration_modes: [],
    registration_worlds: [],
  });
});

test.each([
  "enabled",
  "registration_ready",
  "registration_modes",
  "registration_worlds",
  "accepts_mail",
  "purchasable",
  "fee_bps",
  "min_price",
  "max_price",
] as const)("状态缺少必填键 %s 时拒绝解码", async (missing) => {
  const { data: status } = await readResponse(
    "public/consignment-status.json",
    ConsignmentStatusSchema,
  );
  const incomplete: Partial<typeof status> = { ...status };
  delete incomplete[missing];
  expect(decode(ConsignmentStatusSchema, incomplete).ok).toBe(false);
});

test("状态拒绝矛盾能力、未知枚举和非法协议常量", async () => {
  const { data: status } = await readResponse(
    "public/consignment-status.json",
    ConsignmentStatusSchema,
  );
  for (const patch of [
    { enabled: false },
    { registration_ready: false },
    { registration_modes: ["display_only"] },
    { registration_worlds: ["unknown"] },
    { registration_worlds: [{ main: null }] },
    { accepts_mail: true },
    { purchasable: true },
    { min_price: 0 },
    { max_price: 1_000_000_001 },
    { unexpected: true },
  ]) {
    expect(decode(ConsignmentStatusSchema, { ...status, ...patch }).ok).toBe(false);
  }
});

test("登记未就绪时模式与世界数组都必须为空", async () => {
  const { data: status } = await readResponse(
    "public/consignment-status-unavailable.json",
    ConsignmentStatusSchema,
  );
  expect(decode(ConsignmentStatusSchema, status)).toEqual({ ok: true, value: status });
  for (const patch of [{ registration_modes: ["direct"] }, { registration_worlds: ["main"] }]) {
    expect(decode(ConsignmentStatusSchema, { ...status, ...patch }).ok).toBe(false);
  }
});

for (const feeBps of [0, 9_999]) {
  test(`状态接受边界费率 ${feeBps}`, async () => {
    const { data: status } = await readResponse(
      "public/consignment-status.json",
      ConsignmentStatusSchema,
    );
    const value = { ...status, fee_bps: feeBps };
    expect(decode(ConsignmentStatusSchema, value)).toEqual({ ok: true, value });
  });
}

for (const feeBps of [-1, 500.5, 10_000, Number.MAX_SAFE_INTEGER + 1]) {
  test(`状态拒绝非法费率 ${feeBps}`, async () => {
    const { data: status } = await readResponse(
      "public/consignment-status.json",
      ConsignmentStatusSchema,
    );
    expect(decode(ConsignmentStatusSchema, { ...status, fee_bps: feeBps }).ok).toBe(false);
  });
}
