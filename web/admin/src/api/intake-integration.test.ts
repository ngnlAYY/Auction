// 对照 Rust 录入请求的浮点词法、末值覆盖与发送前拒绝。

import { expect, test } from "bun:test";

import { createAdminRequestHarness, TEST_SESSION_BODY } from "./api-request-fixture";
import { fixtureText, success } from "./auction-contract-fixture.test";
import { addAdminIntake, prepareAdminIntakeOnce } from "./intake";
import { equipmentJson, equipmentJsonWithBase } from "./intake-fixture";

const COMMAND_ID = "166426e9-b2ec-4731-8262-82858fa67d38";

function prepare(text: string) {
  return prepareAdminIntakeOnce("json", "", text, 1000, "Player");
}

async function expectRejectedBeforeFetch(text: string): Promise<void> {
  const harness = createAdminRequestHarness();
  try {
    harness.installBrowserRequest();
    const { host } = harness.registerHost();
    const requests = harness.installFetch([]);
    const command = prepare(text);
    const result = await addAdminIntake(
      host,
      COMMAND_ID,
      "main",
      1000,
      "Player",
      command,
      new AbortController().signal,
    );
    expect(command(COMMAND_ID)).toEqual<unknown>({
      ok: false,
      error: { kind: "not-sent", code: "invalid_request" },
    });
    expect(result).toEqual<unknown>({
      ok: false,
      error: { kind: "not-sent", code: "invalid_request" },
    });
    expect(requests).toEqual([]);
  } finally {
    harness.cleanup();
  }
}

test.each([
  ["0.1", "0.1"],
  ["0.10000000000000001", "0.1"],
  ["0.1234567890123456789", "0.12345678901234568"],
  ["-0", "-0.0"],
])("冻结正文将 base %s 写成 %s", (input, expected) => {
  const result = prepare(equipmentJsonWithBase(input))(COMMAND_ID);
  expect(result.ok).toBe(true);
  if (result.ok) {
    expect(result.value.body).toContain(`"stats":{"Defense":{"value":"2","base":${expected}}}`);
  }
});

test("同名属性中合法的负零可被后续属性覆盖", () => {
  const result = prepare(
    equipmentJson('"Defense":{"value":"1","base":-0},"Defense":{"value":"2","base":0.1}'),
  )(COMMAND_ID);
  expect(result.ok).toBe(true);
  if (result.ok) {
    expect(result.value.body).toContain('"stats":{"Defense":{"value":"2","base":0.1}}');
  }
});

test.each([
  ["支持 JSON.rawJSON", true, "-0.0"],
  ["缺少 JSON.rawJSON", false, "0"],
] as const)("%s 时首次录入与重试复用冻结的负零正文", async (_label, available, base) => {
  const rawJson = Object.getOwnPropertyDescriptor(JSON, "rawJSON");
  const harness = createAdminRequestHarness();
  const controller = new AbortController();
  try {
    if (!available) {
      expect(Reflect.deleteProperty(JSON, "rawJSON")).toBe(true);
    }
    harness.installBrowserRequest();
    const { host } = harness.registerHost();
    const session = { body: JSON.stringify(TEST_SESSION_BODY) };
    const requests = harness.installFetch([
      session,
      { body: "", status: 503 },
      session,
      { body: fixtureText("lot-intake-command"), status: 201 },
    ]);
    const command = prepare(equipmentJsonWithBase("-0"));
    const send = () =>
      addAdminIntake(host, COMMAND_ID, "main", 1000, "Player", command, controller.signal);
    expect(await send()).toEqual<unknown>({
      ok: false,
      error: { kind: "outcome-unknown", code: "http_503", responseStatus: 503 },
    });
    const frozen = success(command(COMMAND_ID));
    expect(frozen.body).toContain(`"stats":{"Defense":{"value":"2","base":${base}}}`);
    expect(frozen.body).toContain(`"command_id":"${COMMAND_ID}"`);
    expect((await send()).ok).toBe(true);
    expect(success(command(COMMAND_ID))).toBe(frozen);
    expect(
      requests.filter((request) => request.method === "POST").map((request) => request.body),
    ).toEqual([frozen.body, frozen.body]);
  } finally {
    if (rawJson !== undefined) {
      Object.defineProperty(JSON, "rawJSON", rawJson);
    }
    controller.abort();
    harness.cleanup();
  }
});

test.each(["1e400", "1000000.01", "-0.1"])("无效 base %s 在发请求前被拒绝", async (base) => {
  await expectRejectedBeforeFetch(equipmentJsonWithBase(base));
});

test("在发送前拒绝被覆盖的属性结构体里的重复字段", async () => {
  await expectRejectedBeforeFetch(
    equipmentJson(String.raw`"Defense":{"value":"1","\u0076alue":"2"},"Defense":{"value":"2"}`),
  );
});

test("在发送前拒绝被覆盖的孤立代理项", async () => {
  await expectRejectedBeforeFetch(
    equipmentJson(undefined, String.raw`"prefix":"\uD800","prefix":""`),
  );
});

test("带 65000 个零的合法浮点在 200 毫秒内完成正文准备", () => {
  const text = equipmentJsonWithBase(`1.${"0".repeat(65_000)}1`);
  expect(text.length).toBeLessThan(65_536);
  const start = performance.now();
  const result = prepare(text)(COMMAND_ID);
  expect(performance.now() - start).toBeLessThan(200);
  expect(result.ok).toBe(true);
  if (result.ok) {
    expect(result.value.body).toContain('"stats":{"Defense":{"value":"2","base":1}}');
  }
});
