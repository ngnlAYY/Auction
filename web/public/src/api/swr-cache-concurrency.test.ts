// SWR 缓存的并发读取、取消和修订号回归断言。

import { expect, test } from "bun:test";
import { err, ok, type Result } from "@auction/web-shared/result";
import {
  type FetchOutcome,
  getOrFetch,
  invalidateAfterCommand,
  invalidateAll,
  LIVE_TTL_MILLIS,
  readOrFetch,
  resetStoreForTest,
} from "./swr-cache";

type Value = string;
type Failure = string;
type Outcome = FetchOutcome<Value, Failure>;

interface Cell<T> {
  value: T;
}

const TYPE_ID = "swr-cache-test";
const ABORT_ERROR = "aborted";

const passiveSignal = (): AbortSignal => new AbortController().signal;

function changed(value: Value, etag: string | null = null): Outcome {
  return { kind: "changed", value, etag };
}

function transport(error: Failure): Outcome {
  return { kind: "transport", error };
}

function busy(error: Failure): Outcome {
  return { kind: "busy", error };
}

async function call(
  url: string,
  now: Cell<number>,
  ttlMillis: number,
  calls: Cell<number>,
  outcome: (etag: string | null) => Outcome,
): Promise<Result<Value, Failure>> {
  return getOrFetch(
    url,
    ttlMillis,
    TYPE_ID,
    async (etag) => {
      calls.value += 1;
      return outcome(etag);
    },
    ABORT_ERROR,
    passiveSignal(),
    () => now.value,
  );
}

test("invalidation_during_request_keeps_the_inflight_response_stale：请求在途失效后响应只保留为过期条目", async () => {
  resetStoreForTest();
  const url = "/api/v1/auctions/current?world=main";
  const now = { value: 30_000 };
  const calls = { value: 0 };
  expect(
    await call(url, now, LIVE_TTL_MILLIS, calls, () => {
      // 模拟请求在途时实时事件完成失效：本次响应照常交给调用方，期间发生失效时只作为过期条目
      // 保留，供条件请求与暂时性失败回退，不作为新鲜命中。
      invalidateAll();
      return changed("v1");
    }),
  ).toEqual(ok("v1"));
  expect(await call(url, now, LIVE_TTL_MILLIS, calls, () => changed("v2"))).toEqual(ok("v2"));
  expect(calls.value).toBe(2);
});

test("response_after_inflight_invalidation_backs_revalidation_and_fallback：在途失效后的响应支撑再验证与回退", async () => {
  resetStoreForTest();
  const url = "/api/v1/lots/00000000-0000-0000-0000-00000000000e";
  const now = { value: 35_000 };
  const calls = { value: 0 };
  expect(await call(url, now, LIVE_TTL_MILLIS, calls, () => changed("v1", "etag-1"))).toEqual(
    ok("v1"),
  );
  invalidateAll();
  // 重读在途时又来一次失效：v2 已交给页面展示，缓存随之前进到 v2，只是仍按过期处理。
  expect(
    await call(url, now, LIVE_TTL_MILLIS, calls, (etag) => {
      expect(etag).toBe("etag-1");
      invalidateAll();
      return changed("v2", "etag-2");
    }),
  ).toEqual(ok("v2"));
  // 下一次读取带 v2 的 ETag 再验证；读闸繁忙时回退 v2，不倒退到页面已替换掉的 v1。
  expect(
    await call(url, now, LIVE_TTL_MILLIS, calls, (etag) => {
      expect(etag).toBe("etag-2");
      return busy("read_busy");
    }),
  ).toEqual(ok("v2"));
  expect(calls.value).toBe(3);
});

test("concurrent_reads_share_one_inflight_request：同一 URL 的并发读取共用一个在途请求", async () => {
  resetStoreForTest();
  const url = "/api/v1/auctions/00000000-0000-0000-0000-00000000000a/lots?page=1&page_size=20";
  const calls = { value: 0 };
  let release = (_outcome: Outcome): void => {};
  const gate = new Promise<Outcome>((resolve) => {
    release = resolve;
  });
  const first = getOrFetch(
    url,
    LIVE_TTL_MILLIS,
    TYPE_ID,
    () => {
      calls.value += 1;
      return gate;
    },
    ABORT_ERROR,
    passiveSignal(),
    () => 1_000,
  );
  const second = getOrFetch(
    url,
    LIVE_TTL_MILLIS,
    TYPE_ID,
    async () => {
      calls.value += 1;
      return changed("second");
    },
    ABORT_ERROR,
    passiveSignal(),
    () => 1_000,
  );
  // 第二个读取只等待，不发出第二个相同请求。
  expect(calls.value).toBe(1);
  release(changed("v1"));
  expect(await first).toEqual(ok("v1"));
  expect(await second).toEqual(ok("v1"));
  expect(calls.value).toBe(1);
});

test("waiter_requests_again_when_the_inflight_response_was_invalidated：在途响应失效后等待方自行重读", async () => {
  resetStoreForTest();
  const url = "/api/v1/auctions/current?world=isekai";
  const calls = { value: 0 };
  let release = (_outcome: Outcome): void => {};
  const gate = new Promise<Outcome>((resolve) => {
    release = resolve;
  });
  const first = getOrFetch(
    url,
    LIVE_TTL_MILLIS,
    TYPE_ID,
    () => {
      calls.value += 1;
      return gate;
    },
    ABORT_ERROR,
    passiveSignal(),
    () => 1_000,
  );
  const second = getOrFetch(
    url,
    LIVE_TTL_MILLIS,
    TYPE_ID,
    async (etag) => {
      // 等待方不把首个响应当作新鲜命中，而是带它的 ETag 自行再验证。
      expect(etag).toBe("etag-stale");
      calls.value += 1;
      return changed("fresh");
    },
    ABORT_ERROR,
    passiveSignal(),
    () => 1_000,
  );
  // 在途期间失效：首个响应照常交给其调用方，只作为过期条目保留，等待方必须自己读取新快照。
  invalidateAll();
  release(changed("stale", "etag-stale"));
  expect(await first).toEqual(ok("stale"));
  expect(await second).toEqual(ok("fresh"));
  expect(calls.value).toBe(2);
});

test("dropped_inflight_request_releases_waiters：发起方取消后等待方不会被永久挂起", async () => {
  resetStoreForTest();
  const url = "/api/v1/lots/00000000-0000-0000-0000-00000000000b";
  const calls = { value: 0 };
  const controller = new AbortController();
  let release = (_outcome: Outcome): void => {};
  const gate = new Promise<Outcome>((resolve) => {
    release = resolve;
  });
  const first = getOrFetch(
    url,
    LIVE_TTL_MILLIS,
    TYPE_ID,
    () => {
      calls.value += 1;
      return gate;
    },
    ABORT_ERROR,
    controller.signal,
    () => 1_000,
  );
  const second = getOrFetch(
    url,
    LIVE_TTL_MILLIS,
    TYPE_ID,
    async () => {
      calls.value += 1;
      return changed("own");
    },
    ABORT_ERROR,
    passiveSignal(),
    () => 1_000,
  );
  // 发起方随组件卸载被取消：登记撤销，等待方不能永远挂起。
  controller.abort();
  expect(await second).toEqual(ok("own"));
  expect(calls.value).toBe(2);
  release(changed("unused"));
  expect(await first).toEqual(err(ABORT_ERROR));
  // 已取消请求的迟到响应不得回写缓存：仍读到等待方取得的 own，也不会再发请求。
  expect(
    await call(url, { value: 1_000 }, LIVE_TTL_MILLIS, calls, () => changed("late-read")),
  ).toEqual(ok("own"));
  expect(calls.value).toBe(2);
});

test("command_reply_during_an_inflight_reread_forbids_the_pre_command_fallback：重读在途收到命令答复时按最新下限判定", async () => {
  const cases = [
    [true, true],
    [false, true],
    [true, false],
  ] as const;
  for (const [isBusy, commandReply] of cases) {
    resetStoreForTest();
    const url = "/api/v1/lots/00000000-0000-0000-0000-000000000012";
    const now = { value: 110_000 };
    const calls = { value: 0 };
    expect(
      await call(url, now, LIVE_TTL_MILLIS, calls, () => changed("v-before", "etag-1")),
    ).toEqual(ok("v-before"));
    // 他人竞价等普通失效：重读开始时这份快照仍允许作为回退值。
    invalidateAll();
    let release = (_outcome: Outcome): void => {};
    const gate = new Promise<Outcome>((resolve) => {
      release = resolve;
    });
    const reread = readOrFetch(
      url,
      LIVE_TTL_MILLIS,
      TYPE_ID,
      (etag) => {
        expect(etag).toBe("etag-1");
        return gate;
      },
      ABORT_ERROR,
      passiveSignal(),
      () => now.value,
    );
    if (commandReply) {
      // 重读在途期间用户命令得到答复。
      invalidateAfterCommand();
    } else {
      invalidateAll();
    }
    release(isBusy ? busy("read_busy") : transport("network_error"));
    const result = await reread;
    if (commandReply) {
      expect(result).toEqual(err(isBusy ? "read_busy" : "network_error"));
    } else {
      expect(result).toEqual(ok({ value: "v-before", staleFallback: 110_000 }));
    }
  }
});

test("inflight_response_from_before_a_command_is_not_a_fallback：命令前在途、命令后写回的响应不作回退", async () => {
  resetStoreForTest();
  const url = "/api/v1/auctions/00000000-0000-0000-0000-00000000000d/lots?page=1&page_size=20";
  const now = { value: 95_000 };
  const calls = { value: 0 };
  expect(
    await call(url, now, LIVE_TTL_MILLIS, calls, () => {
      invalidateAfterCommand();
      return changed("v-before-command", "etag-before");
    }),
  ).toEqual(ok("v-before-command"));
  expect(
    await call(url, now, LIVE_TTL_MILLIS, calls, (etag) => {
      expect(etag).toBe("etag-before");
      return busy("read_busy");
    }),
  ).toEqual(err("read_busy"));
  expect(calls.value).toBe(2);
});

test.each([
  [200, changed("旧请求", "etag-before")],
  [304, { kind: "not_modified" }],
] as const)(
  "command_invalidation_bypasses_a_delayed_pre_command_read：命令后的重读跳过迟到的 %i 请求",
  async (status, oldOutcome) => {
    resetStoreForTest();
    const url = "/api/v1/auctions/current?world=main";
    const now = { value: 120_000 };
    const calls = { value: 0 };
    expect(await call(url, now, LIVE_TTL_MILLIS, calls, () => changed("原快照", "etag-1"))).toEqual(
      ok("原快照"),
    );
    invalidateAll();
    const delayed = Promise.withResolvers<Outcome>();
    const oldRead = getOrFetch(
      url,
      LIVE_TTL_MILLIS,
      TYPE_ID,
      (etag) => {
        expect(etag).toBe("etag-1");
        calls.value += 1;
        return delayed.promise;
      },
      ABORT_ERROR,
      passiveSignal(),
      () => now.value,
    );

    invalidateAfterCommand();
    const newRead = call(url, now, LIVE_TTL_MILLIS, calls, (etag) => {
      expect(etag).toBe("etag-1");
      return changed("命令后", "etag-after");
    });
    try {
      // 新请求必须在旧响应到达前发出，同一 URL 也不能等待命令前的请求。
      expect(calls.value).toBe(3);
      expect(await newRead).toEqual(ok("命令后"));
      delayed.resolve(oldOutcome);
      expect(await oldRead).toEqual(ok(status === 200 ? "旧请求" : "原快照"));
      expect(await call(url, now, LIVE_TTL_MILLIS, calls, () => changed("多余请求"))).toEqual(
        ok("命令后"),
      );
      expect(calls.value).toBe(3);
    } finally {
      delayed.resolve(oldOutcome);
      await Promise.allSettled([oldRead, newRead]);
    }
  },
);

test("late_304_cannot_mark_a_pre_command_snapshot_fresh：迟到的 304 不能将命令前的快照标回新鲜", async () => {
  resetStoreForTest();
  const url = "/api/v1/auctions/current?world=main";
  const now = { value: 130_000 };
  const calls = { value: 0 };
  expect(await call(url, now, LIVE_TTL_MILLIS, calls, () => changed("原快照", "etag-1"))).toEqual(
    ok("原快照"),
  );
  invalidateAll();
  const delayed = Promise.withResolvers<Outcome>();
  const oldRead = getOrFetch(
    url,
    LIVE_TTL_MILLIS,
    TYPE_ID,
    (etag) => {
      expect(etag).toBe("etag-1");
      calls.value += 1;
      return delayed.promise;
    },
    ABORT_ERROR,
    passiveSignal(),
    () => now.value,
  );
  invalidateAfterCommand();
  delayed.resolve({ kind: "not_modified" });
  expect(await oldRead).toEqual(ok("原快照"));
  expect(await call(url, now, LIVE_TTL_MILLIS, calls, () => transport("network_error"))).toEqual(
    err("network_error"),
  );
  expect(calls.value).toBe(3);
});
