// 对照 Rust 手工录入命令的安全回执与冻结正文。

import { expect, test } from "bun:test";
import { AdminIntakeResponseSchema } from "@auction/contracts/dto/admin/auction";
import {
  COMMAND_ID,
  checkedFixture,
  fixtureText,
  requestFixture,
  SESSION_REPLY,
  SESSION_REQUEST,
  success,
} from "./auction-contract-fixture.test";
import { addAdminIntake, parseCanonicalEquipment, prepareAdminIntakeOnce } from "./intake";
import { equipmentJson, equipmentJsonWithBase, OVERWRITTEN_INVALID_STATS } from "./intake-fixture";
import type { AdminSessionHost } from "./session";

const INTAKE_PATH = "/api/v1/admin/worlds/main/auction-intake/items";

test("intake_command_contract_fixture", async () => {
  const body = fixtureText("lot-intake-command");
  const payload = checkedFixture(body, AdminIntakeResponseSchema, "/data/equipment_level");
  expect(payload.code).toBe("created");
  const result = success(
    await requestFixture(
      [SESSION_REPLY, { body, status: 201 }],
      [SESSION_REQUEST, ["POST", INTAKE_PATH]],
      (host, signal) =>
        addAdminIntake(
          host,
          COMMAND_ID,
          "main",
          1000,
          "Player",
          prepareAdminIntakeOnce(
            "url",
            "https://hentaiverse.org/equip/123/0123456789",
            "",
            1000,
            "Player",
          ),
          signal,
        ),
    ),
  );
  expect(result).toMatchObject({
    lot_number: 1,
    start_price: 1000,
    seller_player_id: "Player",
    equipment_level: null,
  });
});

function intakeBody(equipment: string): string {
  return `{"command_id":"${COMMAND_ID}","source_kind":"json","equipment_url":null,"equipment":${equipment},"start_price":1000,"player_id":"Player"}`;
}

function jsonIntake(text: string) {
  const prepare = prepareAdminIntakeOnce("json", "", text, 1000, "Player");
  return (host: AdminSessionHost, signal: AbortSignal) =>
    addAdminIntake(host, COMMAND_ID, "main", 1000, "Player", prepare, signal);
}

async function assertIntakeDtoRejection(text: string): Promise<void> {
  const expected = { ok: false, error: "invalid_request" } as const;
  const result = await requestFixture([], [], jsonIntake(text));
  expect(result).toEqual(expected);
  expect(parseCanonicalEquipment(text)).toEqual(expected);
}

test("录入 DTO 在复核会话和发送前拒绝重复的具名字段", async () => {
  for (const text of [
    equipmentJson(undefined, '"prefix":"旧","prefix":""'),
    equipmentJson('"Defense":{"base":0,"base":1},"Defense":{"value":"2"}'),
  ]) {
    await assertIntakeDtoRejection(text);
  }
});

// Rust 在表单校验有限 base 的业务范围，Solid 的接口准备入口也会校验。
test.each(["1000000.01", "-0.1"])("录入拒绝有限越界 base %s 且不发出请求", async (base) => {
  const result = await requestFixture([], [], jsonIntake(equipmentJsonWithBase(base)), 0);
  expect(result.ok).toBe(false);
});

// 见 web/README.md 的录入差异。Rust 逐项解码同名属性，Solid 只校验末值。
test("同名属性的非法首项被覆盖时，录入 POST 只发送合法末值", async () => {
  const body = fixtureText("lot-intake-command");
  const expectedWrite = [
    "POST",
    INTAKE_PATH,
    intakeBody(equipmentJson('"Defense":{"value":"2"}')),
  ] as const;
  for (const [, stats] of OVERWRITTEN_INVALID_STATS) {
    const result = await requestFixture(
      [SESSION_REPLY, { body, status: 201 }],
      [SESSION_REQUEST, expectedWrite],
      jsonIntake(equipmentJson(stats)),
    );
    expect(success(result).lot_number).toBe(1);
  }
});

// 见 web/README.md 的录入差异。serde 发送 -0.0，缺少 JSON.rawJSON 的浏览器发送 0。
test.each([
  ["支持", true, "-0.0"],
  ["缺少", false, "0"],
] as const)("%s JSON.rawJSON 时负零录入原样重试", async (_label, available, expectedBase) => {
  const rawJson = Object.getOwnPropertyDescriptor(JSON, "rawJSON");
  if (rawJson === undefined || typeof rawJson.value !== "function") {
    throw new Error("Bun 缺少 JSON.rawJSON，无法验证两种发送路径");
  }
  const send = jsonIntake(equipmentJsonWithBase("-0.0"));
  const expectedWrite = [
    "POST",
    INTAKE_PATH,
    intakeBody(equipmentJsonWithBase(expectedBase)),
  ] as const;
  try {
    if (!available) {
      expect(Reflect.deleteProperty(JSON, "rawJSON")).toBe(true);
    }
    await requestFixture(
      [
        SESSION_REPLY,
        { body: "", status: 503 },
        SESSION_REPLY,
        { body: fixtureText("lot-intake-command"), status: 201 },
      ],
      [SESSION_REQUEST, expectedWrite, SESSION_REQUEST, expectedWrite],
      async (host, signal) => {
        expect(await send(host, signal)).toEqual({ ok: false, error: "http_503" });
        // 首发后切换能力，重新生成正文会改变 base 的词法。
        if (available) {
          expect(Reflect.deleteProperty(JSON, "rawJSON")).toBe(true);
        } else {
          Object.defineProperty(JSON, "rawJSON", rawJson);
        }
        expect(success(await send(host, signal)).lot_number).toBe(1);
      },
    );
  } finally {
    Object.defineProperty(JSON, "rawJSON", rawJson);
  }
});
