// 查询状态实例之间的地址栏同步：任一实例经本模块写回后，其他挂载实例都必须重读。

import { expect, test } from "bun:test";
import type { QueryState } from "@auction/web-shared/net/query";
import { createRouter, RouterContext } from "@auction/web-shared/router";
import { createEffect, createRoot, flush } from "solid-js";

import type { PublicBoardQuery } from "./board";
import { publicBoardQueryDefault } from "./board";
import { usePublicQueryState } from "./state";

interface FakeHistory {
  readonly state: unknown;
  scrollRestoration: "auto" | "manual";
  pushState(state: unknown, title: string, url?: string | null): void;
  replaceState(state: unknown, title: string, url?: string | null): void;
}

interface FakeWindow {
  readonly location: { pathname: string; search: string; hash: string };
  readonly history: FakeHistory;
  failHistoryWrites(fail: boolean): void;
  readonly scrollX: number;
  readonly scrollY: number;
  scrollTo(): void;
  requestAnimationFrame(callback: FrameRequestCallback): number;
  cancelAnimationFrame(handle: number): void;
  addEventListener(type: string, listener: () => void): void;
  removeEventListener(type: string, listener: () => void): void;
}

function createFakeWindow(initialUrl: string): FakeWindow {
  const location = { pathname: "/", search: "", hash: "" };
  let state: unknown = null;
  let failWrites = false;
  const applyUrl = (url: string | null | undefined): void => {
    if (url === undefined || url === null) {
      return;
    }
    const parsed = new URL(url, "https://auction.test");
    location.pathname = parsed.pathname;
    location.search = parsed.search;
    location.hash = parsed.hash;
  };
  applyUrl(initialUrl);
  return {
    location,
    history: {
      get state() {
        return state;
      },
      scrollRestoration: "auto",
      pushState(nextState, _title, url) {
        if (failWrites) {
          throw new Error("history.pushState 被测试拒绝");
        }
        state = nextState;
        applyUrl(url);
      },
      replaceState(nextState, _title, url) {
        if (failWrites) {
          throw new Error("history.replaceState 被测试拒绝");
        }
        state = nextState;
        applyUrl(url);
      },
    },
    failHistoryWrites(fail) {
      failWrites = fail;
    },
    scrollX: 0,
    scrollY: 0,
    scrollTo() {},
    requestAnimationFrame() {
      return 0;
    },
    cancelAnimationFrame() {},
    addEventListener() {},
    removeEventListener() {},
  };
}

interface MountedQueryStates {
  readonly first: QueryState<PublicBoardQuery>;
  readonly second: QueryState<PublicBoardQuery>;
  failHistoryWrites(fail: boolean): void;
  secondUpdates(): number;
  close(): void;
}

function mountQueryStates(): MountedQueryStates {
  const fake = createFakeWindow("/");
  const previousWindow = Object.getOwnPropertyDescriptor(globalThis, "window");
  Object.defineProperty(globalThis, "window", {
    configurable: true,
    writable: true,
    value: fake,
  });
  let first!: QueryState<PublicBoardQuery>;
  let second!: QueryState<PublicBoardQuery>;
  let updates = 0;
  const previousRouterDefault = RouterContext.defaultValue;
  let disposeRoot = (): void => {};
  createRoot((dispose) => {
    disposeRoot = dispose;
    const router = createRouter({ routes: [{ name: "Home", pattern: "/" }] });
    // 测试没有渲染树，直接以上下文默认值提供路由；关闭时恢复，避免影响其他用例。
    RouterContext.defaultValue = router;
    const Harness = (): null => {
      first = usePublicQueryState();
      second = usePublicQueryState();
      createEffect(
        () => second.state(),
        () => {
          updates += 1;
        },
      );
      return null;
    };
    Harness();
    // 初次 effect 要在 root 内刷新；测试没有渲染器替我们安排提交阶段。
    flush();
  });
  return {
    first,
    second,
    failHistoryWrites: (fail) => fake.failHistoryWrites(fail),
    secondUpdates: () => updates,
    close() {
      disposeRoot();
      flush();
      RouterContext.defaultValue = previousRouterDefault;
      if (previousWindow === undefined) {
        Reflect.deleteProperty(globalThis, "window");
      } else {
        Object.defineProperty(globalThis, "window", previousWindow);
      }
    },
  };
}

test("writing_from_one_query_state_refreshes_the_others：一个实例写回后其他实例读到新值", () => {
  const mounted = mountQueryStates();
  try {
    const next = {
      ...publicBoardQueryDefault(),
      world: "isekai",
      auctionId: "67e55044-10b1-426f-9247-bb680e5fe0c8",
    };
    mounted.first.set(next, "replace");
    flush();
    expect(mounted.second.state().world).toBe("isekai");
    expect(mounted.second.state().auctionId).toBe(next.auctionId);
  } finally {
    mounted.close();
  }
});

test("rewriting_the_same_query_does_not_refresh_other_query_states：同值写入不触发其他实例更新", () => {
  const mounted = mountQueryStates();
  try {
    mounted.first.set({ ...publicBoardQueryDefault(), world: "main" }, "replace");
    flush();
    const current = mounted.second.state.peek();
    const updates = mounted.secondUpdates();
    mounted.first.set({ ...current }, "replace");
    flush();
    // 其他实例虽因地址版本变化重读，但结构相等时不写入状态，也不触发下游 effect。
    expect(mounted.second.state.peek()).toBe(current);
    expect(mounted.secondUpdates()).toBe(updates);
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
      mounted.failHistoryWrites(true);
      write(mounted.first, next);
      flush();
      expect(mounted.first.state.peek()).toBe(firstBefore);
      expect(mounted.second.state.peek()).toBe(secondBefore);
      expect(mounted.secondUpdates()).toBe(updates);

      mounted.failHistoryWrites(false);
      write(mounted.first, next);
      flush();
      expect(mounted.first.state().world, name).toBe("isekai");
      expect(mounted.second.state().world, name).toBe("isekai");
      expect(mounted.secondUpdates(), name).toBe(updates + 1);
    } finally {
      mounted.close();
    }
  }
});
