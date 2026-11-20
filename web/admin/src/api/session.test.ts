// Admin 会话读取的时间投影、401 判定与重试等待回归。

import { afterEach, describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { AdminSessionResponseSchema } from "@auction/contracts/dto/admin/session";
import { apiResponseSchema } from "@auction/contracts/dto/response";
import { decode } from "@auction/contracts/wire";
import { parseJson } from "@auction/web-shared/net/wire";

import { jsonObject } from "./json-fixture";
import {
  type AdminSession,
  type AdminSessionHostRegistration,
  fetchAdminSession,
  registerAdminSessionHost,
} from "./session";
import { retryAfterMillis } from "./session-time";

const fixtures = new URL(
  "../../../../crates/auction-contracts/tests/fixtures/admin/",
  import.meta.url,
);

function fixture(name: string): string {
  return readFileSync(new URL(name, fixtures), "utf8");
}

const hosts: AdminSessionHostRegistration[] = [];
const restorers: Array<() => void> = [];

function host(): AdminSessionHostRegistration {
  const registration = registerAdminSessionHost({
    check() {},
    writesAllowed: () => true,
    confirmed: () => null,
  });
  hosts.push(registration);
  return registration;
}

function installResponse(response: Response): void {
  const originalFetch = globalThis.fetch;
  const fake: typeof fetch = Object.assign(async () => response, {
    preconnect: originalFetch.preconnect,
  });
  globalThis.fetch = fake;
  restorers.push(() => {
    globalThis.fetch = originalFetch;
  });
}

function installBody(status: number, body: string | Uint8Array): void {
  const payload = typeof body === "string" ? body : new Uint8Array(body).buffer;
  installResponse(new Response(payload, { status }));
}

function signal(): AbortSignal {
  return new AbortController().signal;
}

async function readSession(
  status: number,
  body: string | Uint8Array,
): Promise<Awaited<ReturnType<typeof fetchAdminSession>>> {
  installBody(status, body);
  return fetchAdminSession(host().host, signal());
}

afterEach(() => {
  while (restorers.length > 0) restorers.pop()?.();
  while (hosts.length > 0) hosts.pop()?.release();
});

describe("Admin 会话接口（对应 Rust api/session.rs）", () => {
  test("only_a_complete_utf8_401_with_unauthenticated_confirms_session_end", async () => {
    const valid = '{"code":"unauthenticated","message":"expired","data":null,"request_id":null}';
    const badUtf8 = Uint8Array.from([
      ...new TextEncoder().encode('{"code":"unauthenticated","message":"'),
      0xff,
      ...new TextEncoder().encode('","data":null,"request_id":null}'),
    ]);
    const incomplete = '{"code":"unauthenticated","message":"expired","data":null}';
    const wrongCode = '{"code":"unknown_code","message":"expired","data":null,"request_id":null}';

    expect(await readSession(401, valid)).toEqual({ ok: true, value: null });
    expect(await readSession(401, badUtf8)).toEqual({ ok: false, error: "http_401" });
    expect(await readSession(401, incomplete)).toEqual({ ok: false, error: "http_401" });
    expect(await readSession(401, wrongCode)).toEqual({ ok: false, error: "unknown_code" });
    expect(await readSession(503, valid)).toEqual({ ok: false, error: "http_503" });
  });

  test("decodes_server_session_projection_with_server_time", async () => {
    installResponse(new Response(fixture("session.json"), { status: 200 }));
    const session = await fetchAdminSession(host().host, signal());
    expect(session.ok).toBe(true);
    if (!session.ok) throw new Error("会话样本读取失败");
    expect(session.value).not.toBeNull();
    if (session.value === null) throw new Error("会话样本未解码出会话");
    // 2026-11-18T01:02:03.456789Z 的独立纪元纳秒值，不复用被测解析器。
    expect(session.value.serverTime).toBe(1_794_963_723_456_789_000n);
    expect(session.value.expiresAt - session.value.serverTime).toBe(15n * 60n * 1_000_000_000n);
  });

  test("timed_session_projection_rejects_missing_or_invalid_time_and_unknown_fields", async () => {
    const parsed = parseJson(fixture("session.json"));
    expect(parsed.ok).toBe(true);
    if (!parsed.ok) throw new Error("会话样本不是合法 JSON");
    const original = structuredClone(jsonObject(parsed.value, "会话样本"));
    for (const value of [null, 42, "invalid"]) {
      const wrongTime = structuredClone(original);
      jsonObject(wrongTime.data, "会话样本 data").server_time = value;
      installBody(200, JSON.stringify(wrongTime));
      expect(await fetchAdminSession(host().host, signal())).toEqual({
        ok: false,
        error: "invalid_response",
      });
    }
    const missing = structuredClone(original);
    const missingData = jsonObject(missing.data, "会话样本 data");
    expect(Object.hasOwn(missingData, "server_time")).toBe(true);
    delete missingData.server_time;
    installBody(200, JSON.stringify(missing));
    expect(await fetchAdminSession(host().host, signal())).toEqual({
      ok: false,
      error: "invalid_response",
    });

    const unknown = structuredClone(original);
    jsonObject(unknown.data, "会话样本 data").extra = true;
    installBody(200, JSON.stringify(unknown));
    expect(await fetchAdminSession(host().host, signal())).toEqual({
      ok: false,
      error: "invalid_response",
    });
  });

  test("retry_after_uses_delay_seconds_or_the_response_date", () => {
    expect(retryAfterMillis("12", null)).toBe(12_000);
    const deadline = "Sun, 06 Nov 1994 08:49:37 GMT";
    const sent = "Sun, 06 Nov 1994 08:49:07 GMT";
    expect(retryAfterMillis(deadline, sent)).toBe(30_000);
    expect(retryAfterMillis(deadline, null)).toBeNull();
    for (const value of [null, "", "-1", "1, 2"]) {
      expect(retryAfterMillis(value, sent)).toBeNull();
    }
  });

  test("default_admin_session_fixture_keeps_the_published_fields", async () => {
    const body = fixture("session-default.json");
    const parsed = parseJson(body);
    expect(parsed.ok).toBe(true);
    if (!parsed.ok) throw new Error("缺省会话样本不是合法 JSON");
    const legacy = decode(apiResponseSchema(AdminSessionResponseSchema), parsed.value);
    expect(legacy.ok).toBe(true);
    if (!legacy.ok) throw new Error("缺省会话样本未通过解码");
    expect(legacy.value.data.server_time ?? null).toBeNull();
    installBody(200, body);
    expect(await fetchAdminSession(host().host, signal())).toEqual({
      ok: false,
      error: "invalid_response",
    });
  });

  test("session_activity_contract_fixture", async () => {
    const body = fixture("session-activity.json");
    installBody(200, body);
    const session = await fetchAdminSession(host().host, signal());
    expect(session.ok).toBe(true);
    if (!session.ok) throw new Error("活动续期样本读取失败");
    expect(session.value).not.toBeNull();
    if (session.value === null) throw new Error("活动续期样本未解码出会话");
    const value: AdminSession = session.value;
    expect(value.actor).toBe("operator");
    expect(value.expiresAt > value.serverTime).toBe(true);

    const parsed = parseJson(body);
    expect(parsed.ok).toBe(true);
    if (!parsed.ok) throw new Error("活动续期样本不是合法 JSON");
    const invalid = structuredClone(jsonObject(parsed.value, "活动续期样本"));
    const invalidData = jsonObject(invalid.data, "活动续期样本 data");
    expect(Object.hasOwn(invalidData, "server_time")).toBe(true);
    delete invalidData.server_time;
    installBody(200, JSON.stringify(invalid));
    expect(await fetchAdminSession(host().host, signal())).toEqual({
      ok: false,
      error: "invalid_response",
    });
  });
});
