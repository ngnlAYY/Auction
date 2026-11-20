// 认证外壳的路由刷新：真实地址变化和重复输入均由同一挂载中的响应式更新触发。

import { expect, test } from "bun:test";
import { createCell } from "@auction/web-shared/reactive/cell";
import {
  createRouter,
  type RouteMatch,
  type Router,
  RouterContext,
} from "@auction/web-shared/router";
import { installFakeWindow } from "@auction/web-shared/router/test-window";
import { createRoot, flush } from "solid-js";
import { ADMIN_ROUTES } from "../router/route";
import { createComponent, mountNode } from "../solid-dom-fixture";
import { installAuthDocument, settleAuth } from "./auth-environment-fixture";
import { installAuthLocale } from "./auth-locale-fixture.test";
import { installFetch, transport } from "./effects-session-fixture";
import { sessionFixture } from "./session/session-fixture";
import { AdminAuthenticatedShell } from "./shell";

type RefreshCounts = readonly [readiness: number, pending: number];

interface MountedShell {
  readonly router: Router;
  counts(): RefreshCounts;
  publishRoute(route: RouteMatch): void;
}

async function withShell(check: (shell: MountedShell) => Promise<void>): Promise<void> {
  const locale = await installAuthLocale();
  let restoreDocument = (): void => {};
  let restoreWindow = (): void => {};
  let restoreFetch = (): void => {};
  let disposeRoot = (): void => {};
  try {
    const stubs = transport();
    restoreFetch = installFetch(stubs);
    restoreWindow = installFakeWindow("/").restore;
    const installed = installAuthDocument();
    restoreDocument = installed.restore;
    const injected = createCell<RouteMatch | null>(null);
    const readinessRefresh = createCell(0);
    const router = createRoot((dispose) => {
      disposeRoot = dispose;
      const live = createRouter({ routes: ADMIN_ROUTES });
      const observed: Router = { ...live, current: () => injected() ?? live.current() };
      mountNode(
        installed.document,
        createComponent(RouterContext, {
          value: observed,
          get children() {
            return createComponent(AdminAuthenticatedShell, {
              session: sessionFixture("current-cookie", 900).session,
              readiness: "ready",
              readinessRefresh,
              sessionConfirmed: true,
              redirecting: true,
              loggingOut: false,
              logoutError: null,
              onLogout: () => {},
              onRecheck: () => {},
            });
          },
        }),
      );
      return live;
    });
    await settleAuth();
    await check({
      router,
      counts: () => [readinessRefresh.peek(), stubs.pendingCountsCalls],
      publishRoute: (route) => injected.set(route),
    });
  } finally {
    try {
      disposeRoot();
      flush();
    } finally {
      try {
        restoreDocument();
      } finally {
        try {
          restoreWindow();
        } finally {
          try {
            restoreFetch();
          } finally {
            locale.restore();
          }
        }
      }
    }
  }
}

async function verifyRealNavigation({ router, counts }: MountedShell): Promise<void> {
  const observed = [counts()];
  const beforeQuery = router.current();
  router.push("/users?page=2");
  await settleAuth();
  observed.push(counts());
  const afterQuery = router.current();
  expect(afterQuery).not.toBe(beforeQuery);
  expect(afterQuery).toMatchObject({ kind: "matched", name: "Users" });
  expect(router.currentLocationString()).toBe("/users?page=2");

  router.push("/users");
  await settleAuth();
  observed.push(counts());
  const afterSame = router.current();
  expect(afterSame).not.toBe(afterQuery);
  expect(afterSame).toMatchObject({ kind: "matched", name: "Users" });
  expect(router.currentLocationString()).toBe("/users");

  // 挂载会读取一次待办数；两个计数随后只随页面身份变化递增。
  expect(observed).toEqual([
    [0, 1],
    [1, 2],
    [1, 2],
  ]);
}

async function verifyRepeatedUsers({ counts, publishRoute }: MountedShell): Promise<void> {
  let comparisons = 0;
  const firstParams = new Proxy<Record<string, string>>(
    {},
    {
      ownKeys(target) {
        comparisons += 1;
        return Reflect.ownKeys(target);
      },
    },
  );
  const repeatedParams: Record<string, string> = {};
  // 参数键值完全相同；不同原型让派生值通知外壳，再由实际路由比较处理等价输入。
  Object.setPrototypeOf(repeatedParams, null);

  const observed = [counts()];
  publishRoute({ kind: "matched", name: "Users", params: firstParams, query: {} });
  await settleAuth();
  observed.push(counts());
  const previousComparisons = comparisons;
  publishRoute({ kind: "matched", name: "Users", params: repeatedParams, query: {} });
  await settleAuth();
  observed.push(counts());

  // 派生层在原型不同时直接发布；枚举上一次的参数证明刷新回调比较了第二次 Users。
  expect(comparisons).toBe(previousComparisons + 1);
  expect(observed).toEqual([
    [0, 1],
    [1, 2],
    [1, 2],
  ]);
}

test("refreshes_pending_and_readiness_only_after_route_changes：只有路由变化才刷新两个计数", async () => {
  await withShell(verifyRealNavigation);
  await withShell(verifyRepeatedUsers);
});
