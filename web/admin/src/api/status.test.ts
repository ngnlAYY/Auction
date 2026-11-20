// Admin 就绪探针的公开布尔投影与闭集字段回归。

import { afterEach, describe, expect, spyOn, test } from "bun:test";
import { readFileSync } from "node:fs";
import { parseJson } from "@auction/web-shared/net/wire";
import { jsonObject } from "./json-fixture";
import { type AdminSessionHostRegistration, registerAdminSessionHost } from "./session";
import { fetchAdminReadiness } from "./status";

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

function installBody(status: number, body: string): void {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = Object.assign(async () => new Response(body, { status }), {
    preconnect: originalFetch.preconnect,
  });
  restorers.push(() => {
    globalThis.fetch = originalFetch;
  });
}

function signal(): AbortSignal {
  return new AbortController().signal;
}

afterEach(() => {
  while (restorers.length > 0) restorers.pop()?.();
  while (hosts.length > 0) hosts.pop()?.release();
});

describe("Admin 就绪探针（对应 Rust api/status.rs）", () => {
  test("admin_readiness_contract_fixtures", async () => {
    // TS 生产入口把 Rust 的 not_ready 与 unavailable 都投影为 false；三态细分在当前公开边界外。
    for (const [name, status, expected] of [
      ["readiness.json", 200, true],
      ["readiness-unavailable.json", 503, false],
    ] as const) {
      const body = fixture(name);
      installBody(status, body);
      expect(await fetchAdminReadiness(host().host, signal())).toEqual({
        ok: true,
        value: expected,
      });

      const parsed = parseJson(body);
      expect(parsed.ok).toBe(true);
      if (!parsed.ok) throw new Error("就绪样本不是合法 JSON");
      const invalid = structuredClone(jsonObject(parsed.value, "就绪样本"));
      const data = jsonObject(invalid.data, "就绪样本 data");
      expect(Object.hasOwn(data, "ready")).toBe(true);
      delete data.ready;
      installBody(status, JSON.stringify(invalid));
      expect(await fetchAdminReadiness(host().host, signal())).toEqual({
        ok: false,
        error: "invalid_response",
      });
    }
  });

  test("readiness_rejects_unknown_fields", async () => {
    for (const [name, status] of [
      ["readiness.json", 200],
      ["readiness-unavailable.json", 503],
    ] as const) {
      const body = fixture(name);
      for (const path of ["", "data"] as const) {
        const parsed = parseJson(body);
        expect(parsed.ok).toBe(true);
        if (!parsed.ok) throw new Error("就绪样本不是合法 JSON");
        const invalid = structuredClone(jsonObject(parsed.value, "就绪样本"));
        if (path === "") {
          invalid.extra = true;
        } else {
          jsonObject(invalid.data, "就绪样本 data").extra = true;
        }
        installBody(status, JSON.stringify(invalid));
        expect(await fetchAdminReadiness(host().host, signal())).toEqual({
          ok: false,
          error: "invalid_response",
        });
      }
    }
  });

  test("readiness_not_ready_is_classified_before_the_public_boolean_projection", async () => {
    // 公开入口把 not_ready 折叠为 false；经已导出的 ok 观察内部三态，证明 503 样本确实归为未就绪。
    const result = await import("@auction/web-shared/result");
    const ok = spyOn(result, "ok");
    try {
      installBody(503, fixture("readiness-unavailable.json"));
      expect(await fetchAdminReadiness(host().host, signal())).toEqual({ ok: true, value: false });
      expect(ok.mock.calls.some(([value]) => value === "not_ready")).toBe(true);
    } finally {
      ok.mockRestore();
    }
  });
});
