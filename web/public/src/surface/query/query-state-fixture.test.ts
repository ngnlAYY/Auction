// Public 查询测试共用的浏览器历史与查询挂载夹具。

import { expect } from "bun:test";
import { createRouter, type Router, RouterContext } from "@auction/web-shared/router";
import { createEffect, createRoot, flush } from "solid-js";

import { type PublicQueryState, usePublicQueryState } from "./state";

interface FakeHistory {
  readonly state: unknown;
  scrollRestoration: "auto" | "manual";
  pushState(state: unknown, title: string, url?: string | null): void;
  replaceState(state: unknown, title: string, url?: string | null): void;
  back(): void;
  forward(): void;
}

interface FakeWindow extends EventTarget {
  readonly location: URL;
  readonly history: FakeHistory;
  failHistoryWrites(fail: boolean): void;
  pushCount(): number;
  readonly scrollX: number;
  readonly scrollY: number;
  scrollTo(): void;
  requestAnimationFrame(callback: FrameRequestCallback): number;
  cancelAnimationFrame(handle: number): void;
}

function createFakeWindow(initialUrl: string): FakeWindow {
  const events = new EventTarget();
  const location = new URL(initialUrl, "https://auction.test");
  const past: { url: string; state: unknown }[] = [];
  const future: { url: string; state: unknown }[] = [];
  let entry: { url: string; state: unknown } = { url: location.href, state: null };
  let failWrites = false;
  let pushes = 0;
  const writeEntry = (state: unknown, url: string | null | undefined): void => {
    location.href = new URL(url ?? location.href, location).href;
    entry = { url: location.href, state };
  };
  const fixture: Omit<FakeWindow, keyof EventTarget> = {
    location,
    history: {
      get state() {
        return entry.state;
      },
      scrollRestoration: "auto",
      pushState(nextState, _title, url) {
        if (failWrites) {
          throw new Error("history.pushState 被测试拒绝");
        }
        pushes += 1;
        past.push(entry);
        future.length = 0;
        writeEntry(nextState, url);
      },
      replaceState(nextState, _title, url) {
        if (failWrites) {
          throw new Error("history.replaceState 被测试拒绝");
        }
        writeEntry(nextState, url);
      },
      back() {
        const previous = past.pop();
        if (previous !== undefined) {
          future.push(entry);
          writeEntry(previous.state, previous.url);
          events.dispatchEvent(new Event("popstate"));
        }
      },
      forward() {
        const next = future.pop();
        if (next !== undefined) {
          past.push(entry);
          writeEntry(next.state, next.url);
          events.dispatchEvent(new Event("popstate"));
        }
      },
    },
    failHistoryWrites(fail) {
      failWrites = fail;
    },
    pushCount: () => pushes,
    scrollX: 0,
    scrollY: 0,
    scrollTo() {},
    requestAnimationFrame() {
      return 0;
    },
    cancelAnimationFrame() {},
  };
  return Object.assign(events, fixture);
}

interface MountedQueryStates {
  readonly first: PublicQueryState;
  readonly second: PublicQueryState;
  readonly router: Router;
  readonly location: FakeWindow["location"];
  failHistoryWrites(fail: boolean): void;
  secondUpdates(): number;
  snapshotUpdates(): number;
  pushCount(): number;
  back(): void;
  forward(): void;
  visibleRows(): readonly number[];
  close(): void;
}

/** 挂载使用同一地址的两个 Public 查询实例及列表替身。 */
export function mountQueryStates(initialUrl = "/"): MountedQueryStates {
  const fake = createFakeWindow(initialUrl);
  const previousWindow = Object.getOwnPropertyDescriptor(globalThis, "window");
  Object.defineProperty(globalThis, "window", {
    configurable: true,
    writable: true,
    value: fake,
  });
  let first!: PublicQueryState;
  let second!: PublicQueryState;
  let router!: Router;
  let updates = 0;
  let snapshotUpdates = 0;
  const rows = Array.from({ length: 100 }, (_, index) => index + 1);
  let visibleRows: readonly number[] = [];
  const previousRouterDefault = RouterContext.defaultValue;
  let disposeRoot = (): void => {};
  createRoot((dispose) => {
    disposeRoot = dispose;
    router = createRouter({
      routes: [
        { name: "Home", pattern: "/" },
        { name: "BidLog", pattern: "/bidlog" },
        { name: "Fulfillment", pattern: "/fulfillment" },
        { name: "CompensationPool", pattern: "/compensation-pool" },
        { name: "History", pattern: "/history" },
      ],
    });
    createEffect(
      () => [router.currentLocationString()],
      () => {
        snapshotUpdates += 1;
      },
    );
    // 测试没有渲染树，直接以上下文默认值提供路由；关闭时恢复，避免影响其他用例。
    RouterContext.defaultValue = router;
    const Harness = (): null => {
      first = usePublicQueryState();
      second = usePublicQueryState();
      createEffect(
        () => second.state(),
        ({ page, pageSize }) => {
          updates += 1;
          // 模拟订阅已应用查询的列表视图。
          visibleRows = rows.slice((page - 1) * pageSize, page * pageSize);
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
    router,
    location: fake.location,
    failHistoryWrites: (fail) => fake.failHistoryWrites(fail),
    secondUpdates: () => updates,
    snapshotUpdates: () => snapshotUpdates,
    pushCount: fake.pushCount,
    back() {
      fake.history.back();
      flush();
    },
    forward() {
      fake.history.forward();
      flush();
    },
    visibleRows: () => visibleRows,
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

/** 断言快照同步到当前地址，并返回该地址供历史导航断言使用。 */
export function expectCurrentAddress(mounted: MountedQueryStates): string {
  const { pathname, search, hash } = mounted.location;
  const address = `${pathname}${search}${hash}`;
  expect(mounted.router.currentLocationString()).toBe(address);
  return address;
}
