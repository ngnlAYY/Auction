// 管理端路由用例的内存浏览器、重载记录和认证请求。

import { createRouter, type Router } from "@auction/web-shared/router";
import { installFakeWindow } from "@auction/web-shared/router/test-window";
import { type Component, createRoot, flush } from "solid-js";
import { installFixtureFetch, recordRequest } from "../../api/api-request-fixture";
import { loadRenderModule } from "../../solid-render-fixture.test";
import { cleanupScope, preserveProperty } from "../../test-scope-fixture";
import {
  type AuthTestDocument,
  installAuthDocument,
  settleAuth,
} from "../auth/auth-environment-fixture";
import { installAuthLocale } from "../auth/auth-locale-fixture.test";
import {
  installFetch,
  Stub,
  sessionBody,
  type Transport,
  transport,
} from "../auth/effects-session-fixture";
import { configReadReply, defaultProjection } from "../config/config-projection-fixture.test";
import { createComponent, createElement, insert } from "../solid-dom-fixture";
import { ADMIN_ROUTES } from "./route";
import { SOURCE_ROUTE, type SourceObservation, WRITE_PATH } from "./route-load-fixture";

/** 与认证回执替身一致的虚构 Cookie。 */
export const COOKIE = "fixture-cookie";
/** 标签页防循环标记的持久化键。 */
export const MARK = "hv.page-load-reload";
/** 固定分块失败时刻，避免测试之间跨过防循环窗口。 */
export const NOW = 1_800_000_000_000;
/** 仪表盘进入页面后立即请求的聚合快照。 */
export const DASHBOARD_API = "/api/v1/admin/dashboard?include=collection_wait";

interface Reload {
  readonly address: string;
  readonly mark: string | null;
}

function installRouteWindow(start: string) {
  const { fake, restore } = installFakeWindow(start);
  const storage = new Map<string, string>();
  const writes: { key: string; value: string }[] = [];
  const attempts: Reload[] = [];
  const reloads: Reload[] = [];
  const confirmations: string[] = [];
  let rejectStorage = false;
  let confirmUnload = false;
  let confirmManual = false;
  Object.assign(fake.location, {
    reload() {
      const reload = {
        address: fake.location.pathname + fake.location.search + fake.location.hash,
        mark: storage.get(MARK) ?? null,
      };
      attempts.push(reload);
      const event = new Event("beforeunload", { cancelable: true });
      Object.defineProperty(event, "returnValue", { value: "", writable: true });
      if (fake.dispatchEvent(event) || confirmUnload) reloads.push(reload);
    },
  });
  Object.assign(fake, {
    confirm(message: string) {
      confirmations.push(message);
      return confirmManual;
    },
    sessionStorage: {
      getItem: (key: string) => storage.get(key) ?? null,
      setItem(key: string, value: string) {
        writes.push({ key, value });
        if (rejectStorage) throw new DOMException("测试拒绝存储", "QuotaExceededError");
        storage.set(key, value);
      },
    },
  });
  return {
    fake,
    storage,
    writes,
    attempts,
    reloads,
    confirmations,
    rejectStorage: () => {
      rejectStorage = true;
    },
    acceptUnload: () => {
      confirmUnload = true;
    },
    acceptManual: () => {
      confirmManual = true;
    },
    restore,
  };
}

function installRouteTransport(stubs: Transport) {
  const restore = installFetch(stubs);
  const fallback = globalThis.fetch;
  const business = new Stub();
  const writes: ReturnType<typeof recordRequest>[] = [];
  const dashboardReads: string[] = [];
  const configReads: string[] = [];
  const detailReads: string[] = [];
  const snapshot = defaultProjection();
  const restoreRoutes = installFixtureFetch(async (input, init) => {
    const request = recordRequest(input, init);
    const url = new URL(request.url, "https://auction.test");
    const path = url.pathname + url.search;
    if (path === WRITE_PATH && request.method === "POST") {
      writes.push({ ...request, url: path });
      const reply = await business.request(init?.signal ?? undefined);
      if (reply.kind === "network") throw new TypeError("测试业务请求断线");
      return new Response(reply.body, { status: reply.status });
    }
    if (path === DASHBOARD_API) {
      dashboardReads.push(path);
      return apiError(503, "storage_unavailable");
    }
    if (url.pathname === "/api/v1/admin/config") {
      configReads.push(path);
      return configReadReply({
        ...snapshot,
        data: { ...snapshot.data, worker_restart_state: "consistent" },
      });
    }
    if (url.pathname === "/api/v1/admin/runtime-switches") {
      return Response.json({
        code: "ok",
        message: "",
        data: { database_intent: [], admin_applied: [] },
      });
    }
    if (url.pathname.startsWith("/api/v1/admin/deliveries/")) {
      detailReads.push(path);
      return apiError(404, "delivery_not_found");
    }
    if (url.pathname.startsWith("/api/v1/admin/settlement-returns/")) {
      detailReads.push(path);
      return apiError(404, "settlement_return_not_found");
    }
    if (/^\/api\/v1\/admin\/auctions\/[^/]+\/lots$/u.test(url.pathname)) {
      detailReads.push(path);
      return apiError(404, "auction_not_found");
    }
    return fallback(input, init);
  });
  return {
    business,
    writes,
    dashboardReads,
    configReads,
    detailReads,
    snapshot,
    restore() {
      restoreRoutes();
      restore();
    },
  };
}

function apiError(status: number, code: string): Response {
  return Response.json({ code, message: "", data: null, request_id: null }, { status });
}

type RouteWindow = ReturnType<typeof installRouteWindow>;
type RouteTransport = ReturnType<typeof installRouteTransport>;
type Layout = typeof import("./route-load-fixture");

/** 当前挂载的浏览器、真实路由与内存请求。 */
export interface Fixture {
  readonly document: AuthTestDocument;
  readonly router: Router;
  readonly browser: RouteWindow;
  readonly stubs: Transport;
  readonly api: RouteTransport;
  readonly observation: SourceObservation;
  readonly bootPage: ReturnType<Layout["deferAdminPage"]> | null;
  loadWarningCount(): number;
  deferPage(name?: string): ReturnType<Layout["deferAdminPage"]>;
  go(address: string): Promise<void>;
}

/** 在冷启动 /setup 地址直接挂载生产路由宿主，记录它发出的会话请求。 */
export async function withSetup(
  check: (document: AuthTestDocument, stubs: Transport, api: RouteTransport) => void,
): Promise<void> {
  const [layout, entry] = await Promise.all([
    loadRenderModule<Layout>(
      new URL("./route-load-fixture.tsx", import.meta.url).pathname,
      "SetupHarness",
    ),
    loadRenderModule<{ readonly RouterHost: Component }>(
      new URL("../index.tsx", import.meta.url).pathname,
      "RouterHost",
    ),
  ]);
  const scope = cleanupScope();
  try {
    const locale = installAuthLocale();
    scope.defer(locale.restore);
    scope.defer(installRouteWindow("/setup").restore);
    const installed = installAuthDocument();
    scope.defer(installed.restore);
    const { document } = installed;
    const stubs = transport();
    const api = installRouteTransport(stubs);
    scope.defer(api.restore);
    createRoot((dispose) => {
      scope.defer(() => {
        dispose();
        flush();
      });
      const main = createElement("div", { id: "main", "aria-busy": "true" });
      document.add(main);
      insert(
        main,
        createComponent(layout.SetupHarness, {
          locale: locale.session,
          routerHost: entry.RouterHost,
        }),
      );
    });
    await settleAuth();
    check(document, stubs, api);
  } finally {
    scope.dispose();
  }
}

/** 作用域结束时还原浏览器、传输、页面表和响应式挂载。 */
export async function withLayout(
  check: (fixture: Fixture) => Promise<void>,
  {
    start = SOURCE_ROUTE.pattern,
    authenticate = true,
    deferOnBoot = "",
    onRouterCreated,
  }: {
    readonly start?: string;
    readonly authenticate?: boolean;
    readonly deferOnBoot?: string;
    readonly onRouterCreated?: (router: Router) => void;
  } = {},
): Promise<void> {
  const layout = await loadRenderModule<Layout>(
    new URL("./route-load-fixture.tsx", import.meta.url).pathname,
    "RouteLoadHarness",
    "installSourcePage",
    "deferAdminPage",
  );
  const scope = cleanupScope();
  try {
    const locale = await installAuthLocale();
    scope.defer(locale.restore);
    const browser = installRouteWindow(start);
    scope.defer(browser.restore);
    const installed = installAuthDocument(authenticate ? `auction_admin_csrf=${COOKIE}` : "");
    scope.defer(installed.restore);
    const { document } = installed;
    const stubs = transport();
    const api = installRouteTransport(stubs);
    scope.defer(api.restore);
    scope.defer(preserveProperty(Date, "now"));
    Date.now = () => NOW;
    scope.defer(preserveProperty(console, "warn"));
    let loadWarnings = 0;
    console.warn = (message: unknown) => {
      if (message === "管理后台页面分块加载失败") loadWarnings += 1;
    };
    scope.defer(layout.installSourcePage());
    const deferPage = (name = "Status") => {
      const deferred = layout.deferAdminPage(name);
      scope.defer(deferred.restore);
      return deferred;
    };
    const bootPage = deferOnBoot === "" ? null : deferPage(deferOnBoot);
    const observation: SourceObservation = { mounts: [], unmounts: 0 };
    const router = createRoot((dispose) => {
      scope.defer(() => {
        dispose();
        flush();
      });
      const router = createRouter({ routes: [...ADMIN_ROUTES, SOURCE_ROUTE] });
      onRouterCreated?.(router);
      const main = createElement("div", { id: "main", "aria-busy": "true" });
      document.add(main);
      insert(
        main,
        createComponent(layout.RouteLoadHarness, { router, observation, locale: locale.session }),
      );
      return router;
    });
    await settleAuth();
    if (authenticate) {
      if (stubs.get.pendingCount() !== 1) throw new Error("缺少初次会话查询");
      stubs.get.finish(sessionBody());
      await settleAuth();
    }
    const fixture: Fixture = {
      document,
      router,
      browser,
      stubs,
      api,
      observation,
      bootPage,
      deferPage,
      loadWarningCount: () => loadWarnings,
      async go(address) {
        router.push(address);
        await settleAuth();
      },
    };
    await check(fixture);
  } finally {
    scope.dispose();
  }
}
