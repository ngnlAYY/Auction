// 验证 v2 能力的双世界形状、季界与未来购买能力。

import { expect, test } from "bun:test";

import { decode } from "../../../wire";
import { readResponse } from "./consignment-fixture";
import { type ConsignmentStatusV2, ConsignmentStatusV2Schema } from "./status-v2";

const READY = "public/consignment-status-v2.json";
const UNKNOWN = "public/consignment-status-v2-unknown.json";
const DISABLED = "public/consignment-status-v2-disabled.json";

test("v2 状态夹具覆盖可登记、季历未知与首次禁用", async () => {
  const ready = await readResponse(READY, ConsignmentStatusV2Schema);
  const unknown = await readResponse(UNKNOWN, ConsignmentStatusV2Schema);
  const disabled = await readResponse(DISABLED, ConsignmentStatusV2Schema);
  expect(ready.data).toMatchObject({
    enabled: true,
    registration_ready: true,
    accepts_mail: true,
    purchasable: false,
  });
  expect(ready.data.worlds.main).toMatchObject({
    season_state: "not_seasonal",
    cutoff_at: null,
    season_end_at: null,
  });
  expect(ready.data.worlds.isekai).toMatchObject({
    season_state: "open",
    mail_ready: true,
    purchase_ready: false,
  });
  expect(unknown.data.worlds.isekai).toMatchObject({
    season_state: "unknown",
    mail_ready: false,
    purchase_ready: false,
    cutoff_at: null,
    season_end_at: null,
  });
  expect(disabled.data).toMatchObject({
    enabled: false,
    registration_ready: false,
    registration_modes: [],
    registration_worlds: [],
    accepts_mail: false,
    purchasable: false,
  });
});

test("v2 世界对象必须恰有 main 与 isekai 两个键", async () => {
  const { data: status } = await readResponse(READY, ConsignmentStatusV2Schema);
  const extra = {
    ...status,
    worlds: { ...status.worlds, other: status.worlds.main },
  };
  expect(decode(ConsignmentStatusV2Schema, extra).ok).toBe(false);
  for (const missing of ["main", "isekai"] as const) {
    const worlds =
      missing === "main" ? { isekai: status.worlds.isekai } : { main: status.worlds.main };
    expect(decode(ConsignmentStatusV2Schema, { ...status, worlds }).ok).toBe(false);
  }
});

test("main 无赛季且两个时刻必须为 null", async () => {
  const { data: status } = await readResponse(READY, ConsignmentStatusV2Schema);
  for (const patch of [
    {
      main: { ...status.worlds.main, cutoff_at: "2026-12-01T00:00:00Z" },
    },
    {
      main: { ...status.worlds.main, season_end_at: "2026-12-15T00:00:00Z" },
    },
    {
      main: {
        ...status.worlds.main,
        season_state: "open",
        cutoff_at: "2026-12-01T00:00:00Z",
        season_end_at: "2026-12-15T00:00:00Z",
      },
    },
    { isekai: status.worlds.main },
  ]) {
    expect(
      decode(ConsignmentStatusV2Schema, {
        ...status,
        worlds: { ...status.worlds, ...patch },
      }).ok,
    ).toBe(false);
  }
});

test("isekai 未知季历必须关能力并清空两个时刻", async () => {
  const { data: status } = await readResponse(READY, ConsignmentStatusV2Schema);
  const unknown = {
    ...status.worlds.isekai,
    season_state: "unknown",
    mail_ready: false,
    purchase_ready: false,
    cutoff_at: null,
    season_end_at: null,
  };
  expect(
    decode(ConsignmentStatusV2Schema, {
      ...status,
      worlds: { ...status.worlds, isekai: unknown },
    }).ok,
  ).toBe(true);
  for (const patch of [
    { cutoff_at: "2026-12-01T00:00:00Z" },
    { season_end_at: "2026-12-15T00:00:00Z" },
    { mail_ready: true },
    { purchase_ready: true },
  ]) {
    expect(
      decode(ConsignmentStatusV2Schema, {
        ...status,
        worlds: { ...status.worlds, isekai: { ...unknown, ...patch } },
      }).ok,
    ).toBe(false);
  }
});

test("汇总能力必须等于两世界 OR，登记选项与开关一致", async () => {
  const { data: status } = await readResponse(READY, ConsignmentStatusV2Schema);
  for (const patch of [
    { accepts_mail: false },
    { purchasable: true },
    { registration_modes: [] },
    { registration_worlds: [] },
    { enabled: false },
    { min_price: 0 },
    { max_price: 1_000_000_001 },
    { registration_modes: ["display_only"] },
  ]) {
    expect(decode(ConsignmentStatusV2Schema, { ...status, ...patch }).ok).toBe(false);
  }
});

test("购买能力保持布尔类型，可在同一 v2 形状内放开", async () => {
  const { data: status } = await readResponse(READY, ConsignmentStatusV2Schema);
  const opened: ConsignmentStatusV2 = {
    ...status,
    purchasable: true,
    worlds: {
      ...status.worlds,
      isekai: { ...status.worlds.isekai, purchase_ready: true },
    },
  };
  const decoded = decode(ConsignmentStatusV2Schema, opened);
  expect(decoded.ok).toBe(true);
  if (!decoded.ok) throw new Error("放开购买能力应被接受");
  expect(decoded.value.worlds.isekai.purchase_ready).toBe(true);
  expect(decoded.value.purchasable).toBe(true);
});

test("登记未就绪时模式与世界都必须为空", async () => {
  const { data: status } = await readResponse(DISABLED, ConsignmentStatusV2Schema);
  expect(decode(ConsignmentStatusV2Schema, status).ok).toBe(true);
  for (const patch of [{ registration_modes: ["direct"] }, { registration_worlds: ["main"] }]) {
    expect(decode(ConsignmentStatusV2Schema, { ...status, ...patch }).ok).toBe(false);
  }
});
