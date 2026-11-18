// Public 查询测试共用的浏览器历史与查询挂载夹具。

import { expect } from "bun:test";
import { createRouter, type Router, RouterContext } from "@auction/web-shared/router";
import { installFakeWindow } from "@auction/web-shared/router/test-window";
import { createEffect, createRoot, flush } from "solid-js";

import { type PublicQueryState, usePublicQueryState } from "./state";

interface MountedQueryStates {
  readonly first: PublicQueryState;
  readonly second: PublicQueryState;
  readonly router: Router;
  readonly location: URL;
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
  const { fake, restore } = installFakeWindow(initialUrl);
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
    failHistoryWrites: fake.rejectWrites,
    secondUpdates: () => updates,
    snapshotUpdates: () => snapshotUpdates,
    pushCount: () => fake.writes.filter(({ mode }) => mode === "push").length,
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
      restore();
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
