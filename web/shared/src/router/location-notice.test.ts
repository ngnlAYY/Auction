// 共享路由的地址通知契约：直接写 History 只有显式通知才刷新地址快照，且路由销毁后不再响应。

import { expect, test } from "bun:test";
import { createRoot } from "solid-js";

import type { Router } from "./context";
import { createRouter, notifyLocationWritten } from "./index";

const ROUTES = [
  { name: "Home", pattern: "/" },
  { name: "BidLog", pattern: "/bidlog" },
] as const;

interface FakeHistory {
  readonly state: unknown;
  scrollRestoration: "auto" | "manual";
  pushState(state: unknown, title: string, url?: string | null): void;
  replaceState(state: unknown, title: string, url?: string | null): void;
}

interface FakeWindow {
  readonly location: { pathname: string; search: string; hash: string };
  readonly history: FakeHistory;
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
  const history: FakeHistory = {
    get state() {
      return state;
    },
    scrollRestoration: "auto",
    pushState(nextState, _title, url) {
      state = nextState;
      applyUrl(url);
    },
    replaceState(nextState, _title, url) {
      state = nextState;
      applyUrl(url);
    },
  };
  return {
    location,
    history,
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

interface OpenRouter {
  readonly router: Router;
  readonly fake: FakeWindow;
  disposeRouter(): void;
  restore(): void;
}

function openRouter(initialUrl: string): OpenRouter {
  const fake = createFakeWindow(initialUrl);
  const previous = Object.getOwnPropertyDescriptor(globalThis, "window");
  Object.defineProperty(globalThis, "window", {
    configurable: true,
    writable: true,
    value: fake,
  });
  let dispose = (): void => {};
  let disposed = false;
  const router = createRoot((disposeRoot) => {
    dispose = disposeRoot;
    return createRouter({ routes: ROUTES });
  });
  return {
    router,
    fake,
    disposeRouter() {
      if (disposed) {
        return;
      }
      disposed = true;
      dispose();
    },
    restore() {
      if (previous === undefined) {
        Reflect.deleteProperty(globalThis, "window");
        return;
      }
      Object.defineProperty(globalThis, "window", previous);
    },
  };
}

test("通知后 currentLocationString 读取直接写入的查询", () => {
  const opened = openRouter("/?world=main&auction=auction-uuid");
  try {
    // Router 初始化先规范化掉未声明的查询。
    expect(opened.router.currentLocationString()).toBe("/");
    opened.fake.history.replaceState(null, "", "/?world=main&auction=auction-uuid");
    expect(opened.router.currentLocationString()).toBe("/");
    notifyLocationWritten();
    expect(opened.router.currentLocationString()).toBe("/?world=main&auction=auction-uuid");
  } finally {
    opened.disposeRouter();
    opened.restore();
  }
});

test("通知只刷新地址快照，不重新匹配路由", () => {
  const opened = openRouter("/");
  try {
    opened.fake.history.pushState(null, "", "/bidlog?auction=auction-uuid#lots");
    notifyLocationWritten();
    expect(opened.router.currentLocationString()).toBe("/bidlog?auction=auction-uuid#lots");
    const route = opened.router.current();
    expect(route.kind).toBe("matched");
    if (route.kind === "matched") {
      expect(route.name).toBe("Home");
    }
  } finally {
    opened.disposeRouter();
    opened.restore();
  }
});

test("路由销毁后通知不再生效", () => {
  const opened = openRouter("/");
  try {
    opened.fake.history.replaceState(null, "", "/?world=main");
    notifyLocationWritten();
    expect(opened.router.currentLocationString()).toBe("/?world=main");
    opened.disposeRouter();
    opened.fake.history.replaceState(null, "", "/?world=isekai");
    notifyLocationWritten();
    expect(opened.router.currentLocationString()).toBe("/?world=main");
  } finally {
    opened.disposeRouter();
    opened.restore();
  }
});
