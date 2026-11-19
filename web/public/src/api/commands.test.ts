// Public 写请求的回执丢失与发送前失败对权威重读的影响。

import { expect, jest, spyOn, test } from "bun:test";
import { DEFAULT_REQUEST_TIMEOUT_MILLIS } from "@auction/web-shared/net/transport";
import { err, ok, type Result } from "@auction/web-shared/result";

import {
  COMMAND_NOT_SENT,
  commandErrorOutcomeUncertain,
  createProxyAuthorization,
  lowerProxyAuthorization,
  placeBid,
} from "./commands";
import { cachedPublicGet } from "./queries/cached-get";
import { logoutPublic } from "./session";
import { LIVE_TTL_MILLIS, resetStoreForTest } from "./swr-cache";
import { setLotWatchPreference } from "./watch-preference";
import { setLotWatch } from "./watches";

const USER_ID = "00000000-0000-4000-8000-000000000001";
const AUCTION_ID = "00000000-0000-4000-8000-000000000002";
const LOT_ID = "00000000-0000-4000-8000-000000000003";
const COMMAND_ID = "00000000-0000-4000-8000-000000000004";
const AUTHORIZATION_ID = "00000000-0000-4000-8000-000000000005";

const bid = (signal: AbortSignal) => placeBid(USER_ID, LOT_ID, 2_000, COMMAND_ID, signal);
const createProxy = (signal: AbortSignal) =>
  createProxyAuthorization(USER_ID, LOT_ID, 2_000, "fast", COMMAND_ID, signal);
const lowerProxy = (signal: AbortSignal) =>
  lowerProxyAuthorization(USER_ID, LOT_ID, AUTHORIZATION_ID, 2_000, COMMAND_ID, signal);
const watch = (signal: AbortSignal) =>
  setLotWatch(
    USER_ID,
    AUCTION_ID,
    LOT_ID,
    { command_id: COMMAND_ID, watched: true, channel: "mm", expected_version: 0 },
    signal,
  );
const watchPreference = (signal: AbortSignal) =>
  setLotWatchPreference(
    USER_ID,
    { command_id: COMMAND_ID, channel: "mm", expected_version: 0 },
    signal,
  );

async function withCachedSnapshot(
  commandResponse: (init: RequestInit | undefined) => Response | Promise<Response>,
  run: (read: () => Promise<Result<string, string>>, requests: readonly string[]) => Promise<void>,
): Promise<void> {
  const url = `/api/v1/auctions/${AUCTION_ID}/lots?page=1&page_size=20&fields=summary`;
  const requests: string[] = [];
  const fetchSpy = spyOn(globalThis, "fetch");
  resetStoreForTest();
  try {
    fetchSpy.mockImplementation(
      Object.assign(
        async (_input: URL | RequestInfo, init?: RequestInit) => {
          const method = init?.method ?? "GET";
          requests.push(method);
          if (method !== "GET") return commandResponse(init);
          if (requests.length === 1) return new Response("命令前", { headers: { ETag: '"old"' } });
          throw new Error("断网");
        },
        { preconnect: () => {} },
      ),
    );
    const read = () =>
      cachedPublicGet(
        url,
        LIVE_TTL_MILLIS,
        "command-test",
        (response) => ok(response.text()),
        new AbortController().signal,
      );
    expect(await read()).toEqual(ok("命令前"));
    await run(read, requests);
  } finally {
    fetchSpy.mockRestore();
    resetStoreForTest();
  }
}

test.each([
  ["出价", bid],
  ["创建代理", createProxy],
  ["降低代理", lowerProxy],
] as const)(
  "an_unreadable_success_blocks_the_pre_command_snapshot：%s 的 200 正文中断",
  async (_name, send) => {
    await withCachedSnapshot(
      () =>
        new Response(
          new ReadableStream({
            start(controller) {
              controller.error(new Error("正文中断"));
            },
          }),
        ),
      async (read, requests) => {
        const result = await send(new AbortController().signal);
        expect(result).toEqual(err({ code: "network_error", status: 0 }));
        expect(await read()).toEqual(err("network_error"));
        expect(requests).toEqual(["GET", "POST", "GET"]);
      },
    );
  },
);

test.each([503, 200])(
  "an_untrusted_bid_receipt_blocks_the_pre_command_snapshot：出价的 %i 回执不能确认结果",
  async (status) => {
    await withCachedSnapshot(
      () => new Response('{"code":"internal_error","message":"","data":null}', { status }),
      async (read, requests) => {
        const result = await bid(new AbortController().signal);
        expect(result.ok).toBe(false);
        expect(await read()).toEqual(err("network_error"));
        expect(requests).toEqual(["GET", "POST", "GET"]);
      },
    );
  },
);

test.each([
  ['{"code":"version_conflict","message":"","data":null}', "version_conflict", false],
  [
    '{"code":"unexpected","c\\u006fde":"version_conflict","message":"","data":null}',
    "invalid_response",
    true,
  ],
  ['{"code":"version_conflict","message":"","data":{"a":1e999,"a":0}}', "invalid_response", true],
  [
    '{"code":"version_conflict","message":"","data":{"a":"\\ud800","a":0}}',
    "invalid_response",
    true,
  ],
  ['{"code":"version_conflict","message":"","data":{"a":1,"a":2}}', "version_conflict", false],
  [
    `{"code":"version_conflict","message":"","data":{"a":${"[".repeat(126)}0${"]".repeat(126)}}}`,
    "invalid_response",
    true,
  ],
  [
    `{"code":"version_conflict","message":"","data":{"a":${"[".repeat(125)}0${"]".repeat(125)}}}`,
    "version_conflict",
    false,
  ],
] as const)("竞价只接受唯一错误码的明确拒绝：%s", async (body, code, uncertain) => {
  await withCachedSnapshot(
    () => new Response(body, { status: 409 }),
    async (_read, requests) => {
      const result = await bid(new AbortController().signal);
      expect(result).toEqual(err({ code, status: 409 }));
      if (!result.ok) expect(commandErrorOutcomeUncertain(result.error)).toBe(uncertain);
      expect(requests).toEqual(["GET", "POST"]);
    },
  );
});

test.each(["1", "1.0", "1e0"])("装备标记回执只接受整数版本词法（%s）", async (token) => {
  const reply = {
    code: "ok",
    message: "",
    extension: { version: 2 },
    data: {
      user_id: USER_ID,
      auction_id: AUCTION_ID,
      watch: { lot_id: LOT_ID, watched: true, channel: "mm", version: 1 },
    },
  };
  const body = JSON.stringify(reply)
    .replace('"version":1', `"version":${token}`)
    .replace('"version":2', '"version":2.0');
  await withCachedSnapshot(
    () => new Response(body),
    async (_read, requests) => {
      expect(await watch(new AbortController().signal)).toEqual(
        token === "1" ? ok(reply.data) : err({ code: "invalid_response", status: 200 }),
      );
      expect(requests).toEqual(["GET", "PUT"]);
    },
  );
});

test.each([200, 503])(
  "watch_reply_leaves_the_public_catalog_fresh：标记回执 %i 保留公开目录的缓存命中",
  async (status) => {
    const watched = {
      user_id: USER_ID,
      auction_id: AUCTION_ID,
      watch: { lot_id: LOT_ID, watched: true, channel: "mm", version: 1 },
    };
    const reply =
      status === 200
        ? { code: "ok", message: "", data: watched }
        : { code: "service_unavailable", message: "", data: null };
    await withCachedSnapshot(
      () => Response.json(reply, { status }),
      async (read, requests) => {
        const result = await watch(new AbortController().signal);
        expect(result).toEqual(status === 200 ? ok(watched) : err({ code: reply.code, status }));
        expect(await read()).toEqual(ok("命令前"));
        expect(requests).toEqual(["GET", "PUT"]);
      },
    );
  },
);

test("dispatched_command_timeout_blocks_fallback：写请求超时后禁止回退", async () => {
  jest.useFakeTimers();
  try {
    await withCachedSnapshot(
      (init) =>
        new Promise<Response>((_resolve, reject) => {
          init?.signal?.addEventListener("abort", () => reject(new Error("写请求已中止")), {
            once: true,
          });
          jest.advanceTimersByTime(DEFAULT_REQUEST_TIMEOUT_MILLIS);
        }),
      async (read, requests) => {
        expect(await bid(new AbortController().signal)).toEqual(
          err({ code: "request_timeout", status: 0 }),
        );
        expect(await read()).toEqual(err("network_error"));
        expect(requests).toEqual(["GET", "POST", "GET"]);
      },
    );
  } finally {
    jest.useRealTimers();
  }
});

test.each([false, true])(
  "dispatched_command_without_headers_blocks_fallback：未收到响应头，主动中止 %s",
  async (abort) => {
    const controller = new AbortController();
    await withCachedSnapshot(
      () => {
        if (abort) controller.abort();
        throw new Error("传输中断");
      },
      async (read) => {
        expect(await bid(controller.signal)).toEqual(err({ code: "network_error", status: 0 }));
        expect(await read()).toEqual(err("network_error"));
      },
    );
  },
);

test("explicit_rejections_keep_each_commands_invalidation_policy：明确拒绝沿用各命令的缓存策略", async () => {
  const rejected = { code: "version_conflict", message: "", data: null };
  await withCachedSnapshot(
    () => Response.json(rejected, { status: 409 }),
    async (read, requests) => {
      expect(await watchPreference(new AbortController().signal)).toEqual(
        err({ code: "version_conflict", status: 409 }),
      );
      expect(await read()).toEqual(ok("命令前"));
      expect(await bid(new AbortController().signal)).toEqual(
        err({ code: "version_conflict", status: 409 }),
      );
      expect(await read()).toEqual(err("network_error"));
      expect(requests).toEqual(["GET", "PUT", "POST", "GET"]);
    },
  );
});

test("pre_aborted_commands_and_lock_failure_leave_the_cached_snapshot_fresh：发送前中止或取锁失败保留新鲜缓存", async () => {
  await withCachedSnapshot(
    () => {
      throw new Error("未发送");
    },
    async (read, requests) => {
      const controller = new AbortController();
      controller.abort();
      expect(await bid(controller.signal)).toEqual(err(COMMAND_NOT_SENT));
      expect(await logoutPublic(USER_ID, new AbortController().signal)).toEqual(
        err("session_lock_unavailable"),
      );
      expect(await read()).toEqual(ok("命令前"));
      expect(requests).toEqual(["GET"]);
    },
  );
});
