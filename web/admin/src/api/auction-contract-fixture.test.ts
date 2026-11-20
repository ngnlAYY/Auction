// 场次、拍品与录入接口测试共用的后端样本、解码和请求替身。

import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { apiResponseSchema } from "@auction/contracts/dto/response";
import { decode } from "@auction/contracts/wire";
import { parseJson } from "@auction/web-shared/net/wire";
import { installFakeWindow } from "@auction/web-shared/router/test-window";
import type { GenericSchema } from "valibot";
import {
  createAdminRequestHarness,
  type Reply,
  removeAt,
  TEST_SESSION_BODY,
} from "./api-request-fixture";
import type { AdminSessionHost } from "./session";

const FIXTURES = new URL(
  "../../../../crates/auction-contracts/tests/fixtures/admin/",
  import.meta.url,
);

/** Rust 契约测试用的场次 ID。 */
export const AUCTION_ID = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2b";
/** Rust 契约测试用的代理历史所属拍品 ID。 */
export const LOT_ID = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2c";
/** 测试公开写入口所需的有效命令 ID。 */
export const COMMAND_ID = "00000000-0000-4000-8000-000000000002";

/** 直接读取 Rust 测试引用的生产契约样本。 */
export function fixtureText(name: string): string {
  return readFileSync(new URL(`${name}.json`, FIXTURES), "utf8");
}

/** 断言成功后取值，即使失败结果也含字段时也不会继续误测。 */
export function success<T>(
  result:
    | { readonly ok: true; readonly value: T }
    | { readonly ok: false; readonly error?: unknown },
): T {
  expect(result).toMatchObject({ ok: true });
  if (!result.ok) {
    throw new Error(`预期成功：${JSON.stringify(result.error)}`);
  }
  return result.value;
}

/** 解析样本并删除原有字段，与 Rust 的 checked 助手使用同一份正例生成负例。 */
export function withoutField(body: string, path: string): string {
  const invalid = success(parseJson(body));
  removeAt(invalid, path);
  return JSON.stringify(invalid);
}

/** 在 DTO 解码层逐项完成 Rust decode 助手中的成功及缺字段断言。 */
export function checkedFixture<S extends GenericSchema>(body: string, schema: S, path: string) {
  const envelope = apiResponseSchema(schema);
  const decoded = success(decode(envelope, success(parseJson(body))));
  expect(decode(envelope, success(parseJson(withoutField(body, path))))).toEqual({ ok: false });
  return decoded;
}

type ExpectedRequest = readonly [method: string, path: string, body?: string];

/** 使用真实的 Admin 收据解码路径，顺序消费内存回执并核对实际 HTTP 请求。 */
export async function requestFixture<T>(
  replies: readonly Reply[],
  expected: readonly ExpectedRequest[],
  run: (host: AdminSessionHost, signal: AbortSignal) => Promise<T>,
  expectedDecodeBeacons?: number,
): Promise<T> {
  const harness = createAdminRequestHarness();
  const controller = new AbortController();
  const beacons: (string | null)[] = [];
  let restoreWindow: (() => void) | undefined;
  try {
    if (expectedDecodeBeacons !== undefined) {
      restoreWindow = installFakeWindow("/").restore;
    }
    harness.installBrowserRequest();
    const requests = harness.installFetch(replies);
    const scriptedFetch = globalThis.fetch;
    globalThis.fetch = Object.assign(
      (input: Parameters<typeof fetch>[0], init?: Parameters<typeof fetch>[1]) => {
        const href =
          typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
        const url = new URL(href, "https://auction.test");
        if (
          url.origin === "https://auction.test" &&
          url.pathname === "/api/v1/browser-errors" &&
          init?.method === "POST"
        ) {
          beacons.push(url.searchParams.get("class"));
          return Promise.resolve(new Response(null, { status: 204 }));
        }
        return scriptedFetch(input, init);
      },
      { preconnect: scriptedFetch.preconnect },
    );
    const { host } = harness.registerHost();
    const result = await run(host, controller.signal);
    expect(
      requests.map<ExpectedRequest>(({ method, url, body }, index) => {
        const { origin, pathname, search } = new URL(url, "https://auction.test");
        expect(origin).toBe("https://auction.test");
        const path = pathname + search;
        return expected[index]?.[2] === undefined ? [method, path] : [method, path, body];
      }),
    ).toEqual([...expected]);
    if (expectedDecodeBeacons !== undefined) {
      expect(beacons).toEqual(Array.from({ length: expectedDecodeBeacons }, () => "dto_decode"));
    }
    return result;
  } finally {
    try {
      controller.abort();
    } finally {
      try {
        harness.cleanup();
      } finally {
        restoreWindow?.();
      }
    }
  }
}

/** 公开命令写入前的会话复核回执。 */
export const SESSION_REPLY: Reply = { body: JSON.stringify(TEST_SESSION_BODY) };
/** 写入前的会话复核请求。 */
export const SESSION_REQUEST = ["GET", "/api/v1/admin/session?include=server_time"] as const;

test("契约负例夹具仅移除指定嵌套字段，缺少目标字段时失败", () => {
  const body = '{"code":"ok","data":{"id":"lot-1","version":2}}';
  expect(parseJson(withoutField(body, "/data/version"))).toEqual({
    ok: true,
    value: { code: "ok", data: { id: "lot-1" } },
  });
  expect(() => withoutField(body, "/data/absent")).toThrow();
});
