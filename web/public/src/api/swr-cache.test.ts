// SWR 缓存的行为断言：TTL、条件请求、暂时性失败回退、在途请求串行、失效与条目上限逐条对照
// Rust 源实现的同名测试。

import { expect, test } from "bun:test";
import { err, ok, type Result } from "@auction/web-shared/result";
import {
  CATALOG_TTL_MILLIS,
  type CacheRead,
  entryCountForTest,
  type FetchOutcome,
  getOrFetch,
  hasEntryForTest,
  invalidateAfterCommand,
  invalidateAll,
  isCacheableUrl,
  LIVE_TTL_MILLIS,
  MAX_ENTRIES,
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

function notModified(): Outcome {
  return { kind: "not_modified" };
}

function transport(error: Failure): Outcome {
  return { kind: "transport", error };
}

function busy(error: Failure): Outcome {
  return { kind: "busy", error };
}

function rejected(error: Failure): Outcome {
  return { kind: "rejected", error };
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

async function read(
  url: string,
  ttlMillis: number,
  now: () => number,
  outcome: Outcome,
  signal: AbortSignal = passiveSignal(),
): Promise<Result<CacheRead<Value>, Failure>> {
  return readOrFetch(url, ttlMillis, TYPE_ID, async () => outcome, ABORT_ERROR, signal, now);
}

test("fresh_entry_is_served_without_second_request：TTL 内直接命中新鲜条目", async () => {
  resetStoreForTest();
  const url = "/api/v1/auctions/current?world=main";
  const now = { value: 1_000 };
  const calls = { value: 0 };
  expect(
    await call(url, now, LIVE_TTL_MILLIS, calls, (etag) => {
      expect(etag).toBeNull();
      return changed("v1", "etag-1");
    }),
  ).toEqual(ok("v1"));
  now.value = 1_000 + LIVE_TTL_MILLIS - 1;
  expect(await call(url, now, LIVE_TTL_MILLIS, calls, () => changed("v2"))).toEqual(ok("v1"));
  expect(calls.value).toBe(1);
});

test("expired_entry_revalidates_with_etag_and_304_reuses_value：过期后带 ETag 再验证，304 复用正文", async () => {
  resetStoreForTest();
  const url = "/api/v1/lots/00000000-0000-0000-0000-000000000001";
  const now = { value: 5_000 };
  const calls = { value: 0 };
  expect(await call(url, now, LIVE_TTL_MILLIS, calls, () => changed("v1", "etag-1"))).toEqual(
    ok("v1"),
  );
  now.value = 5_000 + LIVE_TTL_MILLIS;
  expect(
    await call(url, now, LIVE_TTL_MILLIS, calls, (etag) => {
      expect(etag).toBe("etag-1");
      return notModified();
    }),
  ).toEqual(ok("v1"));
  // 304 已刷新写入时间：TTL 内再次读取不再发请求。
  now.value = 5_000 + LIVE_TTL_MILLIS + LIVE_TTL_MILLIS - 1;
  expect(await call(url, now, LIVE_TTL_MILLIS, calls, () => changed("v3"))).toEqual(ok("v1"));
  expect(calls.value).toBe(2);
});

test("transport_error_falls_back_to_stale_value_without_caching_failure：传输失败回退旧值且不缓存失败", async () => {
  resetStoreForTest();
  const url = "/api/v1/auctions/00000000-0000-0000-0000-000000000002/lots?page=1&page_size=20";
  const now = { value: 10_000 };
  const calls = { value: 0 };
  expect(await call(url, now, LIVE_TTL_MILLIS, calls, () => changed("v1"))).toEqual(ok("v1"));
  now.value = 10_000 + LIVE_TTL_MILLIS;
  expect(await call(url, now, LIVE_TTL_MILLIS, calls, () => transport("network_error"))).toEqual(
    ok("v1"),
  );
  expect(calls.value).toBe(2);
  // 失败没有刷新写入时间：下一次仍按过期处理并重新请求权威值。
  expect(await call(url, now, LIVE_TTL_MILLIS, calls, () => changed("v2"))).toEqual(ok("v2"));
  expect(calls.value).toBe(3);
  expect(await call(url, now, LIVE_TTL_MILLIS, calls, () => changed("v3"))).toEqual(ok("v2"));
  expect(calls.value).toBe(3);
});

test("rejected_response_neither_replaces_nor_falls_back：拒绝响应既不替换也不回退", async () => {
  resetStoreForTest();
  const url = "/api/v1/price-history?page=1&page_size=50&sort=time_desc";
  const now = { value: 20_000 };
  const calls = { value: 0 };
  expect(await call(url, now, CATALOG_TTL_MILLIS, calls, () => changed("v1"))).toEqual(ok("v1"));
  now.value = 20_000 + CATALOG_TTL_MILLIS;
  expect(
    await call(url, now, CATALOG_TTL_MILLIS, calls, () => rejected("invalid_response")),
  ).toEqual(err("invalid_response"));
  // 4xx/5xx 或正文校验失败不得回退旧值；旧条目仍在，但下一次仍会重新请求。
  expect(await call(url, now, CATALOG_TTL_MILLIS, calls, () => changed("v2"))).toEqual(ok("v2"));
  expect(calls.value).toBe(3);
});

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

test("invalidated_entry_revalidates_with_its_etag_and_304_restores_freshness：失效条目带旧 ETag 再验证并由 304 恢复新鲜", async () => {
  resetStoreForTest();
  const url = "/api/v1/auctions/00000000-0000-0000-0000-000000000003/bid-log?page=1&page_size=50";
  const now = { value: 40_000 };
  const calls = { value: 0 };
  expect(await call(url, now, LIVE_TTL_MILLIS, calls, () => changed("v1", "etag-1"))).toEqual(
    ok("v1"),
  );
  // 仍在 TTL 内，但失效后不得当作新鲜命中：必须带旧 ETag 向服务端再验证。
  invalidateAll();
  expect(
    await call(url, now, LIVE_TTL_MILLIS, calls, (etag) => {
      expect(etag).toBe("etag-1");
      return notModified();
    }),
  ).toEqual(ok("v1"));
  // 304 已在当前修订号下确认正文：TTL 内再次读取不再发请求。
  expect(await call(url, now, LIVE_TTL_MILLIS, calls, () => changed("v3"))).toEqual(ok("v1"));
  expect(calls.value).toBe(2);
  // 再次失效后正文变化时照常写入新值。
  invalidateAll();
  expect(await call(url, now, LIVE_TTL_MILLIS, calls, () => changed("v4", "etag-4"))).toEqual(
    ok("v4"),
  );
  expect(calls.value).toBe(3);
});

test("busy_rejection_serves_the_invalidated_snapshot_and_keeps_it_stale：暂时拒绝回退已失效快照且不刷新", async () => {
  resetStoreForTest();
  const url = "/api/v1/auctions/00000000-0000-0000-0000-00000000000c/lots?page=1&page_size=20";
  const now = { value: 50_000 };
  const calls = { value: 0 };
  expect(await call(url, now, LIVE_TTL_MILLIS, calls, () => changed("v1", "etag-1"))).toEqual(
    ok("v1"),
  );
  invalidateAll();
  // 429/503 类暂时拒绝：已有快照继续展示，不把错误面板换上整张目录。
  expect(await call(url, now, LIVE_TTL_MILLIS, calls, () => busy("read_busy"))).toEqual(ok("v1"));
  // 回退不刷新条目：下一次读取仍带旧 ETag 再验证，而不是命中旧值。
  expect(
    await call(url, now, LIVE_TTL_MILLIS, calls, (etag) => {
      expect(etag).toBe("etag-1");
      return changed("v2", "etag-2");
    }),
  ).toEqual(ok("v2"));
  expect(calls.value).toBe(3);
  // 传输失败对已失效条目同样回退旧值。
  invalidateAll();
  expect(await call(url, now, LIVE_TTL_MILLIS, calls, () => transport("network_error"))).toEqual(
    ok("v2"),
  );
});

test("busy_rejection_without_snapshot_still_fails：没有快照时暂时拒绝仍返回错误", async () => {
  resetStoreForTest();
  const url = "/api/v1/lots/00000000-0000-0000-0000-00000000000d";
  const now = { value: 60_000 };
  const calls = { value: 0 };
  // 首屏没有可回退的快照：照常返回错误，由页面显示错误与重试。
  expect(await call(url, now, LIVE_TTL_MILLIS, calls, () => busy("read_busy"))).toEqual(
    err("read_busy"),
  );
  expect(entryCountForTest()).toBe(0);
  // 暂时拒绝不留条目：下一次读取不带 ETag。
  expect(
    await call(url, now, LIVE_TTL_MILLIS, calls, (etag) => {
      expect(etag).toBeNull();
      return changed("v1");
    }),
  ).toEqual(ok("v1"));
});

test("other_rejections_do_not_fall_back_to_the_invalidated_snapshot：其他拒绝不回退已失效快照", async () => {
  resetStoreForTest();
  const url = "/api/v1/auctions/current?world=main";
  const now = { value: 70_000 };
  const calls = { value: 0 };
  expect(await call(url, now, LIVE_TTL_MILLIS, calls, () => changed("v1"))).toEqual(ok("v1"));
  invalidateAll();
  expect(await call(url, now, LIVE_TTL_MILLIS, calls, () => rejected("not_found"))).toEqual(
    err("not_found"),
  );
});

test("only_transient_failure_fallbacks_are_marked_stale：只有暂时性失败回退才标记旧快照", async () => {
  resetStoreForTest();
  const url = "/api/v1/auctions/00000000-0000-0000-0000-00000000000f/lots?page=1&page_size=20";
  const now = { value: 90_000 };
  const readAtNow = (outcome: Outcome) => read(url, LIVE_TTL_MILLIS, () => now.value, outcome);
  // 新响应与 TTL 内的新鲜命中（不会调用 fetch）都是权威值。
  expect(await readAtNow(changed("v1", "etag-1"))).toEqual(
    ok({ value: "v1", staleFallback: null }),
  );
  now.value = 95_000;
  expect(await readAtNow(rejected("unused"))).toEqual(ok({ value: "v1", staleFallback: null }));
  invalidateAll();
  now.value = 120_000;
  // 暂时拒绝与传输失败回退的旧快照带标记与最近一次确认时刻（写入时间，不是本次读取时间），
  // 调用方据此决定是否稍后再读或提示数据时点。
  expect(await readAtNow(busy("read_busy"))).toEqual(ok({ value: "v1", staleFallback: 90_000 }));
  expect(await readAtNow(transport("network_error"))).toEqual(
    ok({ value: "v1", staleFallback: 90_000 }),
  );
  // 304 确认的正文同样是权威值，并把确认时刻前移。
  expect(await readAtNow(notModified())).toEqual(ok({ value: "v1", staleFallback: null }));
  invalidateAll();
  now.value = 150_000;
  expect(await readAtNow(busy("read_busy"))).toEqual(ok({ value: "v1", staleFallback: 120_000 }));
});

test("command_invalidation_forbids_falling_back_to_pre_command_snapshots：命令失效禁止回退到命令前快照", async () => {
  resetStoreForTest();
  const url = "/api/v1/lots/00000000-0000-0000-0000-00000000000c";
  const now = { value: 90_000 };
  const calls = { value: 0 };
  expect(await call(url, now, LIVE_TTL_MILLIS, calls, () => changed("v1", "etag-1"))).toEqual(
    ok("v1"),
  );
  invalidateAfterCommand();
  expect(
    await call(url, now, LIVE_TTL_MILLIS, calls, (etag) => {
      expect(etag).toBe("etag-1");
      return busy("read_busy");
    }),
  ).toEqual(err("read_busy"));
  expect(await call(url, now, LIVE_TTL_MILLIS, calls, () => transport("network"))).toEqual(
    err("network"),
  );
  // 命令之后的 304 是权威确认：条目回到当前修订号，此后的暂时性失败可以回退。
  expect(await call(url, now, LIVE_TTL_MILLIS, calls, () => notModified())).toEqual(ok("v1"));
  invalidateAll();
  expect(await call(url, now, LIVE_TTL_MILLIS, calls, () => busy("read_busy"))).toEqual(ok("v1"));
  expect(calls.value).toBe(5);
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

test("entry_cap_evicts_the_oldest_write：条目上限淘汰最早写入的条目", async () => {
  resetStoreForTest();
  const calls = { value: 0 };
  const url = (index: number) =>
    `/api/v1/lots/00000000-0000-0000-0000-${String(index).padStart(12, "0")}`;
  for (let index = 0; index <= MAX_ENTRIES; index += 1) {
    const now = { value: 1_000 + index };
    expect(await call(url(index), now, LIVE_TTL_MILLIS, calls, () => changed("v"))).toEqual(
      ok("v"),
    );
    // 失效不删除条目，上限是会话内条目数的唯一约束。
    invalidateAll();
  }
  expect(entryCountForTest()).toBe(MAX_ENTRIES);
  expect(hasEntryForTest(url(0)), "最早写入的条目应被淘汰").toBe(false);
  expect(hasEntryForTest(url(1))).toBe(true);
  expect(hasEntryForTest(url(MAX_ENTRIES))).toBe(true);
});

test("misaligned_current_and_lots_reach_the_server_only_after_invalidation：不一致快照只在失效后回到服务端对齐", async () => {
  const current = "/api/v1/auctions/current?world=main&include=proxy_default_mode";
  const lots = "/api/v1/auctions/00000000-0000-0000-0000-000000000010/lots?page=1&page_size=20";
  // 旧场次配新目录（开场前后各持一边），以及相反的旧目录配新场次。
  const pairs = [
    [current, "etag-current", lots],
    [lots, "etag-lots", current],
  ] as const;
  for (const [laggingUrl, laggingEtag, freshUrl] of pairs) {
    resetStoreForTest();
    const now = { value: 100_000 };
    const calls = { value: 0 };
    const laggingValue = "preview";
    const freshValue = "live";
    expect(
      await call(laggingUrl, now, LIVE_TTL_MILLIS, calls, () => changed(laggingValue, laggingEtag)),
    ).toEqual(ok(laggingValue));
    expect(
      await call(freshUrl, now, LIVE_TTL_MILLIS, calls, () => changed(freshValue, "etag-fresh")),
    ).toEqual(ok(freshValue));
    // 只推进代号的重读（旧的对齐重读与恢复可见补读）：两侧都命中新鲜条目，不发请求。
    now.value = 100_000 + 1_000;
    expect(await call(laggingUrl, now, LIVE_TTL_MILLIS, calls, () => changed("live"))).toEqual(
      ok(laggingValue),
    );
    expect(await call(freshUrl, now, LIVE_TTL_MILLIS, calls, () => changed("other"))).toEqual(
      ok(freshValue),
    );
    expect(calls.value).toBe(2);
    // 先失效再重读：滞后的一侧带 ETag 取得新快照，另一侧 304 确认，两侧重新对齐。
    invalidateAll();
    expect(
      await call(laggingUrl, now, LIVE_TTL_MILLIS, calls, (etag) => {
        expect(etag).toBe(laggingEtag);
        return changed("live", "etag-new");
      }),
    ).toEqual(ok("live"));
    expect(
      await call(freshUrl, now, LIVE_TTL_MILLIS, calls, (etag) => {
        expect(etag).toBe("etag-fresh");
        return notModified();
      }),
    ).toEqual(ok(freshValue));
    expect(calls.value).toBe(4);
  }
});

test("historical_deep_link_rereads_revalidate_and_busy_falls_back_marked_stale：历史深链失效后再验证，繁忙回退标记旧快照", async () => {
  resetStoreForTest();
  const directory = "/api/v1/auctions?page=1&page_size=200&status=settled";
  const lots = "/api/v1/auctions/00000000-0000-0000-0000-000000000011/lots?page=1&page_size=20";
  const readMarked = async (url: string, ttlMillis: number, outcome: Outcome) => {
    const result = await read(url, ttlMillis, () => 200_000, outcome);
    return result.ok
      ? ok({ value: result.value.value, fallback: result.value.staleFallback !== null })
      : result;
  };
  expect(await readMarked(directory, CATALOG_TTL_MILLIS, changed("final", "d1"))).toEqual(
    ok({ value: "final", fallback: false }),
  );
  expect(await readMarked(lots, LIVE_TTL_MILLIS, changed("final", "l1"))).toEqual(
    ok({ value: "final", fallback: false }),
  );
  // TTL 内未失效：新鲜命中，不会调用 fetch（这里给出的拒绝不会被使用）。
  expect(await readMarked(directory, CATALOG_TTL_MILLIS, rejected("unused"))).toEqual(
    ok({ value: "final", fallback: false }),
  );
  invalidateAll();
  // 失效后真实再验证：结算后的目录返回新快照。
  expect(await readMarked(directory, CATALOG_TTL_MILLIS, changed("settled", "d2"))).toEqual(
    ok({ value: "settled", fallback: false }),
  );
  // lot 外壳重读遇到 503：回退并标明旧快照，不当作已对齐的权威值。
  expect(await readMarked(lots, LIVE_TTL_MILLIS, busy("read_busy"))).toEqual(
    ok({ value: "final", fallback: true }),
  );
});

test("only_public_read_paths_are_cacheable：只有公开只读路径可以进入缓存", () => {
  for (const url of [
    "/api/v1/auctions/current?world=main",
    "/api/v1/auctions?page=1&page_size=50&status=settled",
    "/api/v2/auctions?page=1&page_size=50&status=settled",
    "/api/v1/auctions/00000000-0000-0000-0000-000000000005/lots?page=1",
    "/api/v1/auctions/00000000-0000-0000-0000-000000000005/bid-log?page=1",
    "/api/v1/auctions/00000000-0000-0000-0000-000000000005/fulfillment?page=1",
    "/api/v1/lots/00000000-0000-0000-0000-000000000006",
    "/api/v1/price-history?page=1",
  ]) {
    expect(isCacheableUrl(url), `${url} 应允许缓存`).toBe(true);
  }
  for (const url of [
    "/api/v1/session",
    "/api/v1/worlds",
    "/api/v1/me/lot-watches?auction_id=00000000-0000-0000-0000-000000000007",
    "/api/v1/me/lot-watch-preference",
    "/api/v1/lots/00000000-0000-0000-0000-000000000008/watch",
    "/api/v1/lots/00000000-0000-0000-0000-000000000008/bids",
    "/api/v1/lots/00000000-0000-0000-0000-000000000008/proxy-authorizations",
    "/api/v1/auctions/00000000-0000-0000-0000-000000000009/lots/extra",
    "/api/v1/auctions/00000000-0000-0000-0000-000000000009/fulfillment/extra",
  ]) {
    expect(isCacheableUrl(url), `${url} 不得进入缓存`).toBe(false);
  }
});

test("non_cacheable_url_never_stores_a_value：非白名单 URL 永不写入缓存", async () => {
  resetStoreForTest();
  const url = "/api/v1/session";
  const now = { value: 60_000 };
  const calls = { value: 0 };
  expect(await call(url, now, LIVE_TTL_MILLIS, calls, () => changed("anonymous"))).toEqual(
    ok("anonymous"),
  );
  // 即使仍在 TTL 内也必须直连；私有读取不因重复调用而命中缓存。
  expect(await call(url, now, LIVE_TTL_MILLIS, calls, () => changed("authenticated"))).toEqual(
    ok("authenticated"),
  );
  expect(calls.value).toBe(2);
});
