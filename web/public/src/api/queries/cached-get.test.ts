// 公开只读 GET 非 2xx 分类的回归断言：只有繁忙与网关状态可以回退旧快照。

import { expect, jest, spyOn, test } from "bun:test";
import { DEFAULT_REQUEST_TIMEOUT_MILLIS } from "@auction/web-shared/net/transport";
import { err, ok } from "@auction/web-shared/result";

import {
  invalidateAfterCommand,
  invalidateAll,
  LIVE_TTL_MILLIS,
  resetStoreForTest,
} from "../swr-cache";

const { cachedPublicRead, failureOutcome } = await import("./cached-get");

test("only_busy_and_gateway_statuses_are_transient_rejections：只有繁忙与网关状态是暂时性拒绝", () => {
  for (const status of [429, 502, 503, 504, 520, 521, 522, 523, 524]) {
    expect(failureOutcome<null, string>(status, "busy"), String(status)).toEqual({
      kind: "busy",
      error: "busy",
    });
  }
  // 其余非 2xx（含无 ETag 却收到的 304、500、CDN 的 525/526 证书错误与业务拒绝）仍失败关闭。
  for (const status of [304, 400, 401, 403, 409, 422, 500, 501, 525, 526]) {
    expect(failureOutcome<null, string>(status, "rejected"), String(status)).toEqual({
      kind: "rejected",
      error: "rejected",
    });
  }
  for (const status of [404, 410]) {
    expect(failureOutcome<null, string>(status, "not_found")).toEqual({
      kind: "missing",
      error: "not_found",
    });
  }
});

test.each([
  [404, "断流"],
  [410, "断流"],
  [403, "断流"],
  [500, "断流"],
  [503, "断流"],
  [404, "取消"],
  [404, "超时"],
] as const)(
  "incomplete_http_response_uses_the_known_status：%i 响应正文%s",
  async (status, failure) => {
    const url = "/api/v1/lots/00000000-0000-4000-8000-000000000003";
    const etags: (string | null)[] = [];
    const secondRead = new AbortController();
    const fetchSpy = spyOn(globalThis, "fetch");
    resetStoreForTest();
    if (failure === "超时") jest.useFakeTimers();
    try {
      fetchSpy.mockImplementation(
        Object.assign(
          async (_input: URL | RequestInfo, init?: RequestInit) => {
            etags.push(new Headers(init?.headers).get("If-None-Match"));
            if (etags.length === 1) {
              return new Response("原快照", { headers: { ETag: '"old"' } });
            }
            if (etags.length === 2) {
              const response = new Response(null, { status });
              response.arrayBuffer = () =>
                new Promise<ArrayBuffer>((_resolve, reject) => {
                  const interrupt = () => reject(new Error("正文中断"));
                  if (failure === "断流") {
                    interrupt();
                    return;
                  }
                  init?.signal?.addEventListener("abort", interrupt, { once: true });
                  // 进入正文读取时，传输层已通知调用方收到响应头。
                  if (failure === "取消") secondRead.abort();
                  else jest.advanceTimersByTime(DEFAULT_REQUEST_TIMEOUT_MILLIS);
                });
              return response;
            }
            throw new Error("断网");
          },
          { preconnect: () => {} },
        ),
      );
      const read = (signal = new AbortController().signal) =>
        cachedPublicRead(
          url,
          LIVE_TTL_MILLIS,
          "status-test",
          (response) => ok(response.text()),
          signal,
        );
      const first = await read();
      expect(first).toEqual(ok({ value: "原快照", staleFallback: null }));
      invalidateAll();

      const second = await read(secondRead.signal);
      if (status === 503) {
        expect(second).toMatchObject(ok({ value: "原快照", staleFallback: expect.any(Number) }));
      } else {
        expect(second).toEqual(err(failure === "超时" ? "request_timeout" : "network_error"));
      }

      const third = await read();
      if (status === 404 || status === 410) {
        expect(third).toEqual(err("network_error"));
        expect(etags).toEqual([null, '"old"', null]);
      } else {
        expect(third).toMatchObject(ok({ value: "原快照", staleFallback: expect.any(Number) }));
        expect(etags).toEqual([null, '"old"', '"old"']);
      }
    } finally {
      fetchSpy.mockRestore();
      resetStoreForTest();
      if (failure === "超时") jest.useRealTimers();
    }
  },
);

test.each([404, 410])(
  "late_missing_response_evicts_the_post_command_snapshot：较早请求迟到的 %i 驱逐新快照",
  async (status) => {
    const delayed = Promise.withResolvers<Response>();
    const missing = Response.json({ code: "not_found", message: "", data: null }, { status });
    const etags: (string | null)[] = [];
    const fetchSpy = spyOn(globalThis, "fetch");
    resetStoreForTest();
    try {
      fetchSpy.mockImplementation(
        Object.assign(
          async (_input: URL | RequestInfo, init?: RequestInit) => {
            etags.push(new Headers(init?.headers).get("If-None-Match"));
            if (etags.length === 1) return delayed.promise;
            if (etags.length === 2) return new Response("新快照", { headers: { ETag: '"new"' } });
            throw new Error("断网");
          },
          { preconnect: () => {} },
        ),
      );
      const before = readCachedLot();
      invalidateAfterCommand();
      const after = readCachedLot();
      try {
        expect(etags).toEqual([null, null]);
        expect(await after).toEqual(ok({ value: "新快照", staleFallback: null }));
        delayed.resolve(missing);
        expect(await before).toEqual(err("not_found"));
        expect(await readCachedLot()).toEqual(err("network_error"));
        expect(etags).toEqual([null, null, null]);
      } finally {
        delayed.resolve(missing);
        await Promise.allSettled([before, after]);
      }
    } finally {
      fetchSpy.mockRestore();
      resetStoreForTest();
    }
  },
);

test("eviction_at_headers_blocks_every_earlier_inflight_write：响应头驱逐后只有新请求可以重建快照", async () => {
  const missingHeaders = Promise.withResolvers<Response>();
  const missingBody = Promise.withResolvers<ArrayBuffer>();
  const youngerResponse = Promise.withResolvers<Response>();
  const readingBody = Promise.withResolvers<void>();
  const missing = new Response(null, { status: 404 });
  missing.arrayBuffer = () => {
    readingBody.resolve();
    return missingBody.promise;
  };
  const errorBody = await Response.json({
    code: "not_found",
    message: "",
    data: null,
  }).arrayBuffer();
  const fetchSpy = spyOn(globalThis, "fetch");
  resetStoreForTest();
  try {
    let requests = 0;
    fetchSpy.mockImplementation(
      Object.assign(
        async () => {
          requests += 1;
          if (requests === 1) return missingHeaders.promise;
          if (requests === 2) return youngerResponse.promise;
          if (requests === 5) return new Response("驱逐后重建");
          throw new Error("断网");
        },
        { preconnect: () => {} },
      ),
    );
    const missingRead = readCachedLot();
    invalidateAfterCommand();
    const younger = readCachedLot();
    let offline: ReturnType<typeof readCachedLot> | null = null;
    try {
      missingHeaders.resolve(missing);
      await readingBody.promise;
      offline = readCachedLot();
      expect(requests).toBe(3);
      youngerResponse.resolve(new Response("驱逐前已在途"));
      expect(await younger).toEqual(ok({ value: "驱逐前已在途", staleFallback: null }));
      expect(await offline).toEqual(err("network_error"));
      expect(await readCachedLot()).toEqual(err("network_error"));
      const rebuilt = ok({ value: "驱逐后重建", staleFallback: null });
      expect(await readCachedLot()).toEqual(rebuilt);
      // 同一个 404 的正文收尾不能再次驱逐响应头之后重建的快照。
      missingBody.resolve(errorBody);
      expect(await missingRead).toEqual(err("not_found"));
      expect(await readCachedLot()).toEqual(rebuilt);
      expect(requests).toBe(5);
    } finally {
      youngerResponse.resolve(new Response("驱逐前已在途"));
      missingBody.resolve(errorBody);
      await Promise.allSettled([missingRead, younger, offline]);
    }
  } finally {
    fetchSpy.mockRestore();
    resetStoreForTest();
  }
});

test("observed_missing_blocks_an_inflight_fallback_after_cancellation：已取消请求的 410 仍禁止在途读回退", async () => {
  const delayed = Promise.withResolvers<Response>();
  const rereadResponse = Promise.withResolvers<Response>();
  const missing = Response.json({ code: "not_found", message: "", data: null }, { status: 410 });
  const busy = Response.json({ code: "read_busy", message: "", data: null }, { status: 503 });
  const fetchSpy = spyOn(globalThis, "fetch");
  resetStoreForTest();
  try {
    let requests = 0;
    fetchSpy.mockImplementation(
      Object.assign(
        async () => {
          requests += 1;
          if (requests === 1) return new Response("原快照", { headers: { ETag: '"old"' } });
          if (requests === 2) return delayed.promise;
          if (requests === 3) return rereadResponse.promise;
          throw new Error("断网");
        },
        { preconnect: () => {} },
      ),
    );
    expect(await readCachedLot()).toEqual(ok({ value: "原快照", staleFallback: null }));
    invalidateAll();
    const controller = new AbortController();
    const cancelled = readCachedLot(controller.signal);
    controller.abort();
    const reread = readCachedLot();
    try {
      // 此时仍能观察到已到达的状态；取消的是调用方，当前缓存仍须接收 410。
      delayed.resolve(missing);
      expect(await cancelled).toEqual(err("network_error"));
      rereadResponse.resolve(busy);
      expect(await reread).toEqual(err("read_busy"));
      expect(await readCachedLot()).toEqual(err("network_error"));
    } finally {
      delayed.resolve(missing);
      rereadResponse.resolve(busy);
      await Promise.allSettled([cancelled, reread]);
    }
  } finally {
    fetchSpy.mockRestore();
    resetStoreForTest();
  }
});

function readCachedLot(signal = new AbortController().signal) {
  return cachedPublicRead(
    "/api/v1/lots/00000000-0000-4000-8000-000000000003",
    LIVE_TTL_MILLIS,
    "status-test",
    (response) => ok(response.text()),
    signal,
  );
}
