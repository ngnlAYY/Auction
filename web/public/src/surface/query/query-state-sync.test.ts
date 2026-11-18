// 查询状态实例之间的地址栏同步：任一实例经本模块写回后，其他挂载实例都必须重读。

import { expect, test } from "bun:test";
import type { QueryState, QueryWriteMode } from "@auction/web-shared/net/query";
import { writeLocationPreservingScroll } from "@auction/web-shared/router";
import { flush } from "solid-js";

import { auctionParameter } from "./auction";
import { type PublicBoardQuery, publicBoardQueryDefault } from "./board";
import { initialPublicLocation, readPublicQuery } from "./location";
import { expectCurrentAddress, mountQueryStates } from "./query-state-fixture.test";
import {
  commitAuctionSelection,
  setPublicQuery,
  updatePublicQueryState,
  writePublicQuery,
} from "./state";

test("writing_from_one_query_state_refreshes_the_others：一个实例写回后其他实例读到新值", () => {
  const mounted = mountQueryStates();
  try {
    const next = {
      ...publicBoardQueryDefault(),
      world: "isekai",
      auction: auctionParameter(AUCTION),
    };
    mounted.first.set(next, "replace");
    expectCurrentAddress(mounted);
    flush();
    expect(mounted.second.state().world).toBe("isekai");
    expect(mounted.second.state().auction).toEqual(next.auction);
  } finally {
    mounted.close();
  }
});

test("rewriting_the_same_query_does_not_refresh_other_query_states：同值写入不触发其他实例更新", () => {
  const mounted = mountQueryStates();
  try {
    mounted.first.set({ ...publicBoardQueryDefault(), world: "main" }, "replace");
    expectCurrentAddress(mounted);
    flush();
    const current = mounted.second.state.peek();
    const updates = mounted.secondUpdates();
    mounted.first.set({ ...current }, "replace");
    expectCurrentAddress(mounted);
    flush();
    // 地址不变时跳过写入，其他实例也不需要重读。
    expect(mounted.second.state.peek()).toBe(current);
    expect(mounted.secondUpdates()).toBe(updates);
  } finally {
    mounted.close();
  }
});

const AUCTION = "67e55044-10b1-426f-9247-bb680e5fe0c8";

test.each([
  {
    pathname: "/bidlog",
    mode: "push",
    update(query: Omit<PublicBoardQuery, "auction">) {
      query.page = 1;
    },
  },
  {
    pathname: "/fulfillment",
    mode: "push",
    update(query: Omit<PublicBoardQuery, "auction">) {
      query.bidLogLot = null;
      query.page = 1;
    },
  },
  {
    pathname: "/compensation-pool",
    mode: "push",
    update(query: Omit<PublicBoardQuery, "auction">) {
      query.movementPage = 1;
    },
  },
] satisfies {
  pathname: string;
  mode: QueryWriteMode;
  update(query: Omit<PublicBoardQuery, "auction">): void;
}[])("$pathname 写入合法场次后，地址和查询状态都读回新选择", ({ pathname, mode, update }) => {
  const mounted = mountQueryStates(`${pathname}?auction=truncated&page=4&lot=One12`);
  try {
    expect(mounted.first.state.peek().auction).toEqual({ kind: "invalid", values: ["truncated"] });
    expect(commitAuctionSelection(mounted.first.state, AUCTION, mode, update)).toBe(true);
    expectCurrentAddress(mounted);
    expect(mounted.first.state.peek()).toMatchObject({
      auction: { kind: "selected", id: AUCTION },
    });
    flush();
    expect(mounted.location.searchParams.getAll("auction")).toEqual([AUCTION]);
    expect(readPublicQuery()).toMatchObject({ auction: { kind: "selected", id: AUCTION } });
    expect(mounted.second.state.peek()).toMatchObject({
      auction: { kind: "selected", id: AUCTION },
    });
  } finally {
    mounted.close();
  }
});

test("保留未触及的非法场次，补偿池显式清除时恢复未指定状态", () => {
  const mounted = mountQueryStates("/compensation-pool?auction=truncated&movement_page=4&page=3");
  try {
    expect(
      updatePublicQueryState(
        mounted.first.state,
        (query) => {
          query.movementPage = 2;
        },
        "replace",
      ),
    ).toBe(true);
    expectCurrentAddress(mounted);
    expect(readPublicQuery().auction).toEqual({ kind: "invalid", values: ["truncated"] });

    expect(
      commitAuctionSelection(mounted.first.state, null, "push", (query) => {
        query.page = 2;
        query.movementPage = 1;
      }),
    ).toBe(true);
    expectCurrentAddress(mounted);
    expect(mounted.location.searchParams.has("auction")).toBe(false);
    expect(readPublicQuery()).toMatchObject({ auction: { kind: "unspecified" } });
    mounted.back();
    expect(mounted.first.state.peek()).toMatchObject({
      auction: { kind: "invalid", values: ["truncated"] },
      movementPage: 2,
    });
  } finally {
    mounted.close();
  }
});

test("自动选场保留非法链接，显式提交也拒绝非 UUID 目标", () => {
  const mounted = mountQueryStates("/bidlog?auction=bad&a%75ction=other&page=4&keep=%2b#history");
  try {
    const state = mounted.first.state;
    const previous = state.peek();
    const address = mounted.location.href;
    expect(commitAuctionSelection(state, AUCTION, "replace", ["page"])).toBe(false);
    expect(commitAuctionSelection(state, "invalid", "push", ["page"])).toBe(false);
    expect(state.peek()).toBe(previous);
    expect(mounted.location.href).toBe(address);

    expect(commitAuctionSelection(state, AUCTION, "push", ["page"])).toBe(true);
    expect(mounted.location.search).toBe(`?keep=%2b&auction=${AUCTION}`);
    expect(mounted.location.hash).toBe("#history");
    expect(state.peek().auction).toEqual({ kind: "selected", id: AUCTION });
    mounted.back();
    expect(mounted.location.href).toBe(address);
    expect(state.peek().auction).toEqual({ kind: "invalid", values: ["bad", "other"] });
  } finally {
    mounted.close();
  }
});

test("从非法查询构造完整状态时，新的合法 ID 不会继承旧非法值", () => {
  const mounted = mountQueryStates("/bidlog?auction=truncated");
  try {
    mounted.first.replace({ ...mounted.first.state.peek(), auction: auctionParameter(AUCTION) });
    expectCurrentAddress(mounted);
    expect(mounted.first.state.peek()).toMatchObject({
      auction: { kind: "selected", id: AUCTION },
    });
    expect(readPublicQuery()).toMatchObject({ auction: { kind: "selected", id: AUCTION } });
  } finally {
    mounted.close();
  }
});

test.each(["set", "replace", "push", "writePublicQuery", "setPublicQuery"] as const)(
  "完整查询 %s 显式清除场次后，地址栏读回未指定场次",
  (entry) => {
    const mounted = mountQueryStates("/?auction=truncated&ref=x");
    try {
      const cleared = { ...mounted.first.state.peek(), auction: auctionParameter(null) };
      switch (entry) {
        case "set":
          mounted.first.set(cleared, "replace");
          break;
        case "replace":
          mounted.first.replace(cleared);
          break;
        case "push":
          mounted.first.push(cleared);
          break;
        case "writePublicQuery":
          expect(writePublicQuery(cleared, "replace")).toBe(true);
          break;
        case "setPublicQuery":
          expect(setPublicQuery(cleared, true)).toBe(true);
          break;
      }
      expectCurrentAddress(mounted);
      flush();
      expect(mounted.location.searchParams.has("auction")).toBe(false);
      expect(readPublicQuery()).toMatchObject({ auction: { kind: "unspecified" } });
      expect(mounted.first.state.peek()).toMatchObject({ auction: { kind: "unspecified" } });
    } finally {
      mounted.close();
    }
  },
);

test.each(["truncated", AUCTION])("完整查询省略场次时沿用地址中的 %s", (auction) => {
  const mounted = mountQueryStates(`/?auction=${auction}&ref=x`);
  try {
    const original = readPublicQuery();
    const { auction: _auction, ...fields } = original;
    mounted.first.replace({ ...fields, page: 2 });
    expectCurrentAddress(mounted);
    expect(mounted.location.searchParams.getAll("auction")).toEqual([auction]);
    expect(readPublicQuery()).toMatchObject({
      auction: original.auction,
      page: 2,
    });
  } finally {
    mounted.close();
  }
});

test("写入另一项筛选时保留以问号开头的未知参数", () => {
  const mounted = mountQueryStates("/??page=2&ref=x");
  try {
    expect(mounted.first.state.peek().page).toBe(1);
    mounted.first.push({ ...mounted.first.state.peek(), historyName: "Phase Robe" });
    expectCurrentAddress(mounted);
    const params = new URLSearchParams(mounted.location.search);
    expect(params.get("?page")).toBe("2");
    expect(params.get("page")).toBeNull();
    expect(params.get("ref")).toBe("x");
  } finally {
    mounted.close();
  }
});

test("启动查询只清除后加的空问号，片段与原始问号仍保留", () => {
  const mounted = mountQueryStates("/#lots");
  try {
    const initial = initialPublicLocation();
    expect(writeLocationPreservingScroll("/?#lots", "replace")).toBe(true);
    expectCurrentAddress(mounted);
    const hadMarker = initialPublicLocation();
    hadMarker.expire();
    expect(mounted.location.href).toBe("https://auction.test/?#lots");
    initial.expire();
    expectCurrentAddress(mounted);
    expect(mounted.location.href).toBe("https://auction.test/#lots");
    expect(mounted.router.currentLocationString()).toBe("/#lots");
  } finally {
    mounted.close();
  }
});

test("failed_history_writes_keep_state_and_can_be_retried：写入失败不提交状态或通知其他实例", () => {
  const writers: readonly [
    name: string,
    write: (state: QueryState<PublicBoardQuery>, next: PublicBoardQuery) => void,
  ][] = [
    ["set", (state, next) => state.set(next, "replace")],
    ["push", (state, next) => state.push(next)],
    ["replace", (state, next) => state.replace(next)],
  ];
  const next = { ...publicBoardQueryDefault(), world: "isekai" };
  for (const [name, write] of writers) {
    const mounted = mountQueryStates();
    try {
      const firstBefore = mounted.first.state.peek();
      const secondBefore = mounted.second.state.peek();
      const updates = mounted.secondUpdates();
      const locationBefore = mounted.location.href;
      const snapshotUpdates = mounted.snapshotUpdates();
      mounted.failHistoryWrites(true);
      write(mounted.first, next);
      expectCurrentAddress(mounted);
      expect(mounted.location.href).toBe(locationBefore);
      flush();
      expect(mounted.first.state.peek()).toBe(firstBefore);
      expect(mounted.second.state.peek()).toBe(secondBefore);
      expect(mounted.secondUpdates()).toBe(updates);
      expect(mounted.snapshotUpdates()).toBe(snapshotUpdates);

      mounted.failHistoryWrites(false);
      write(mounted.first, next);
      expectCurrentAddress(mounted);
      flush();
      expect(mounted.first.state().world, name).toBe("isekai");
      expect(mounted.second.state().world, name).toBe("isekai");
      expect(mounted.secondUpdates(), name).toBe(updates + 1);
    } finally {
      mounted.close();
    }
  }
});
