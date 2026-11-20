// Admin 首次部署接口的请求头与契约样本回归。

import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import {
  SetupErrorResponseSchema,
  type SetupRequest,
  SetupResponseSchema,
} from "@auction/contracts/setup";
import { decode } from "@auction/contracts/wire";
import { parseJson } from "@auction/web-shared/net/wire";
import { installBrowserRequest } from "./api-request-fixture";
import { jsonObject } from "./json-fixture";
import { submitFirstRun, verifyFirstRun } from "./setup";
import { spyOnBrowserFetch } from "./t1-request-fixture.test";

const fixtures = new URL(
  "../../../../crates/auction-contracts/tests/fixtures/admin/",
  import.meta.url,
);

function fixture(name: string): string {
  return readFileSync(new URL(name, fixtures), "utf8");
}

const FORM: SetupRequest = {
  database: {
    host: "localhost",
    port: 5432,
    username: "auction",
    password: "secret",
    name: "auction",
  },
  admin_actor: "admin",
  admin_secret: "admin-secret",
  public_origin: "https://public.example.test",
  admin_origin: "https://admin.example.test",
  public_listener_port: 8090,
  admin_listener_port: 8091,
  metrics_enabled: false,
  schedule_enabled: false,
  schedule: {
    timezone: "UTC",
    preview_day: "Thursday",
    preview_time: "22:00",
    open_day: "Friday",
    open_time: "00:00",
    nominal_end_day: "Saturday",
    nominal_end_time: "12:00",
  },
  start_price: 1,
  log_filter: "info",
  worker_concurrency: 1,
  worker_poll_interval_seconds: 1,
  realtime_poll_interval_millis: 100,
  confirm_initialize: false,
};

async function withSetupRequests(
  run: (fetch: ReturnType<typeof spyOnBrowserFetch>) => Promise<void>,
) {
  const restoreRequest = installBrowserRequest();
  try {
    const fetch = spyOnBrowserFetch();
    try {
      await run(fetch);
    } finally {
      fetch.restore();
    }
  } finally {
    restoreRequest();
  }
}

function signal(): AbortSignal {
  return new AbortController().signal;
}

describe("Admin 初始化接口（对应 Rust api/setup.rs）", () => {
  test("setup_database_confirmation_has_the_canonical_header_format", async () => {
    await withSetupRequests(async (fetch) => {
      const seen: { confirmation: string | null } = { confirmation: null };
      fetch.respondWith(async (_input, init) => {
        const headers = new Headers(init?.headers);
        seen.confirmation = headers.get("x-setup-confirm-target");
        return new Response('{"status":"validated","initialize_and_start":false}', { status: 200 });
      });
      const result = await submitFirstRun(
        "deploy-token",
        FORM,
        false,
        { host: "::1", port: 5432, name: "拍卖 & %" },
        signal(),
      );
      expect(result).toEqual({ ok: true, value: undefined });
      expect(seen.confirmation).toBe("host=%3A%3A1&port=5432&name=%E6%8B%8D%E5%8D%96+%26+%25");
    });
  });

  test("setup_validation_contract_fixture", async () => {
    const errorBody = fixture("setup-validate-invalid.json");
    await withSetupRequests(async (fetch) => {
      const parsedError = parseJson(errorBody);
      expect(parsedError.ok).toBe(true);
      if (!parsedError.ok) throw new Error("错误样本不是合法 JSON");
      const error = decode(SetupErrorResponseSchema, parsedError.value);
      expect(error.ok).toBe(true);
      if (!error.ok) throw new Error("错误样本未通过解码");
      expect(error.value.code).toBe("invalid_input");

      const missingCode = structuredClone(jsonObject(parsedError.value, "错误样本"));
      expect(Object.hasOwn(missingCode, "code")).toBe(true);
      delete missingCode.code;
      expect(decode(SetupErrorResponseSchema, missingCode).ok).toBe(false);

      fetch.respondWith(async () => new Response(errorBody, { status: 400 }));
      expect(await submitFirstRun("deploy-token", FORM, false, null, signal())).toEqual({
        ok: false,
        error: "invalid_input",
      });
      fetch.respondWith(async () => new Response(JSON.stringify(missingCode), { status: 400 }));
      expect(await submitFirstRun("deploy-token", FORM, false, null, signal())).toEqual({
        ok: false,
        error: "setup_unknown",
      });

      for (const [name, initialize] of [
        ["setup-verify.json", false],
        ["setup-verify-initialize.json", true],
      ] as const) {
        const body = fixture(name);
        const parsed = parseJson(body);
        expect(parsed.ok).toBe(true);
        if (!parsed.ok) throw new Error("接口样本不是合法 JSON");
        const decoded = decode(SetupResponseSchema, parsed.value);
        expect(decoded.ok).toBe(true);
        if (!decoded.ok) throw new Error("接口样本未通过解码");
        expect(decoded.value.status).toBe("verified");
        expect(decoded.value.initialize_and_start).toBe(initialize);
        const missingStatus = structuredClone(jsonObject(parsed.value, "接口样本"));
        expect(Object.hasOwn(missingStatus, "status")).toBe(true);
        delete missingStatus.status;
        expect(decode(SetupResponseSchema, missingStatus).ok).toBe(false);

        fetch.respondWith(async () => new Response(body, { status: 200 }));
        expect(await verifyFirstRun("deploy-token", signal())).toEqual({
          ok: true,
          value: initialize,
        });
        fetch.respondWith(async () => new Response(JSON.stringify(missingStatus), { status: 200 }));
        expect(await verifyFirstRun("deploy-token", signal())).toEqual({
          ok: false,
          error: "setup_unknown",
        });
      }
    });
  });
});
