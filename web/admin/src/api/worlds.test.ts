// Admin 业务世界读取与启停命令的契约样本回归。

import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import {
  AdminWorldListResponseSchema,
  AdminWorldSetEnabledResponseSchema,
} from "@auction/contracts/dto/admin/worlds";
import { apiResponseSchema } from "@auction/contracts/dto/response";
import { decode } from "@auction/contracts/wire";
import { parseJson } from "@auction/web-shared/net/wire";
import { installBrowserRequest } from "./api-request-fixture";
import { jsonArray, jsonObject } from "./json-fixture";
import { type AdminSession, registerAdminSessionHost } from "./session";
import { spyOnBrowserFetch } from "./t1-request-fixture.test";
import {
  type AdminBusinessWorld,
  fetchAdminBusinessWorlds,
  prepareAdminBusinessWorldSetEnabledOnce,
  setAdminBusinessWorldEnabled,
} from "./worlds";

const fixtures = new URL(
  "../../../../crates/auction-contracts/tests/fixtures/admin/",
  import.meta.url,
);

function fixture(name: string): string {
  return readFileSync(new URL(name, fixtures), "utf8");
}

const COMMAND_ID = "00000000-0000-4000-8000-000000000002";
const CSRF_TOKEN = "csrf-token";
const SESSION_BODY = {
  code: "ok",
  message: "",
  data: {
    authenticated: true,
    actor: "operator",
    csrf_token: CSRF_TOKEN,
    expires_at: "2026-11-19T01:00:00Z",
    absolute_expires_at: "2026-11-20T00:00:00Z",
    server_time: "2026-11-19T00:00:00Z",
  },
};

const SESSION: AdminSession = {
  actor: "operator",
  csrfToken: CSRF_TOKEN,
  csrf_token: CSRF_TOKEN,
  expiresAt: 0n,
  absoluteExpiresAt: 0n,
  serverTime: 0n,
  receivedAtMillis: 0,
};

function signal(): AbortSignal {
  return new AbortController().signal;
}

describe("Admin 业务世界接口（对应 Rust api/worlds.rs）", () => {
  test("world_contract_fixtures", async () => {
    let listResponseBody = fixture("worlds.json");
    let commandResponseBody = fixture("world-command.json");
    const restoreRequest = installBrowserRequest();
    try {
      const requests: Array<{ url: string; method: string; body: string }> = [];
      const fetch = spyOnBrowserFetch();
      try {
        fetch.respondWith(async (input, init) => {
          if (typeof input !== "string") throw new Error("世界接口请求应使用相对 URL");
          const url = input;
          requests.push({
            url,
            method: init?.method ?? "GET",
            body: typeof init?.body === "string" ? init.body : "",
          });
          if (url.startsWith("/api/v1/admin/session")) {
            return Response.json(SESSION_BODY);
          }
          if (url === "/api/v1/admin/worlds" && (init?.method ?? "GET") === "GET") {
            return new Response(listResponseBody, { status: 200 });
          }
          if (url === "/api/v1/admin/worlds/isekai" && init?.method === "PATCH") {
            return new Response(commandResponseBody, { status: 200 });
          }
          return new Response("not found", { status: 404 });
        });

        const registration = registerAdminSessionHost({
          check() {},
          writesAllowed: () => true,
          confirmed: () => SESSION,
        });
        try {
          const list = await fetchAdminBusinessWorlds(registration.host, signal());
          expect(list.ok).toBe(true);
          if (!list.ok) throw new Error("世界列表读取失败");
          expect(list.value.map((world: AdminBusinessWorld) => [world.id, world.enabled])).toEqual([
            ["main", true],
            ["isekai", false],
          ]);

          const parsedList = parseJson(listResponseBody);
          expect(parsedList.ok).toBe(true);
          if (!parsedList.ok) throw new Error("世界列表样本不是合法 JSON");
          const missingPolicy = structuredClone(jsonObject(parsedList.value, "世界列表样本"));
          const listData = jsonObject(missingPolicy.data, "世界列表样本 data");
          const worlds = jsonArray(listData.worlds, "世界列表样本 worlds");
          const isekai = jsonObject(worlds[1], "世界列表第二项");
          expect(Object.hasOwn(isekai, "policy_revision")).toBe(true);
          delete isekai.policy_revision;
          expect(decode(apiResponseSchema(AdminWorldListResponseSchema), missingPolicy).ok).toBe(
            false,
          );
          listResponseBody = JSON.stringify(missingPolicy);
          expect(await fetchAdminBusinessWorlds(registration.host, signal())).toEqual({
            ok: false,
            error: "invalid_response",
          });

          const prepared = prepareAdminBusinessWorldSetEnabledOnce("isekai", false, 2)(COMMAND_ID);
          expect(prepared.ok).toBe(true);
          if (!prepared.ok) throw new Error("世界启停命令未冻结");
          const command = await setAdminBusinessWorldEnabled(
            registration.host,
            prepared.value,
            signal(),
          );
          expect(command.ok).toBe(true);
          if (!command.ok) throw new Error("世界启停命令失败");
          expect([command.value.id, command.value.enabled, command.value.policyRevision]).toEqual([
            "isekai",
            false,
            2,
          ]);

          const parsedCommand = parseJson(commandResponseBody);
          expect(parsedCommand.ok).toBe(true);
          if (!parsedCommand.ok) throw new Error("世界命令样本不是合法 JSON");
          const missingEnabled = structuredClone(jsonObject(parsedCommand.value, "世界命令样本"));
          const commandData = jsonObject(missingEnabled.data, "世界命令样本 data");
          const missingWorld = jsonObject(commandData.world, "世界命令样本 world");
          expect(Object.hasOwn(missingWorld, "enabled")).toBe(true);
          delete missingWorld.enabled;
          expect(
            decode(apiResponseSchema(AdminWorldSetEnabledResponseSchema), missingEnabled).ok,
          ).toBe(false);
          commandResponseBody = JSON.stringify(missingEnabled);
          const beforeNegative = requests.length;
          expect(
            await setAdminBusinessWorldEnabled(registration.host, prepared.value, signal()),
          ).toEqual({ ok: false, error: { status: null, code: "invalid_response" } });
          // 负例必须真的发出写请求；先前的会话 GET 成功才可能走到 PATCH，排除前置解码失败顶替。
          const negativeRequests = requests.slice(beforeNegative);
          expect(negativeRequests.map(({ url, method }) => [url, method])).toEqual([
            ["/api/v1/admin/session?include=server_time", "GET"],
            ["/api/v1/admin/worlds/isekai", "PATCH"],
          ]);
          const negativePatch = negativeRequests[1];
          if (negativePatch === undefined) throw new Error("世界写入请求缺失");
          const parsedRequest = parseJson(negativePatch.body);
          expect(parsedRequest).toEqual({
            ok: true,
            value: { command_id: COMMAND_ID, enabled: false, expected_policy_revision: 2 },
          });
        } finally {
          registration.release();
        }
      } finally {
        fetch.restore();
      }
    } finally {
      restoreRequest();
    }
  });
});
