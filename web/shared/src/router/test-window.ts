// 共享路由测试用的浏览器历史、文档和动画帧替身。

import { createRoot } from "solid-js";

import type { Router } from "./context";
import { createRouter } from "./index";

const ROUTES = [
  { name: "Home", pattern: "/" },
  { name: "BidLog", pattern: "/bidlog" },
  { name: "Rules", pattern: "/rules" },
  { name: "Me", pattern: "/me" },
  { name: "Lot", pattern: "/lots/:code" },
] as const;

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
  readonly scrollX: number;
  readonly scrollY: number;
  scrollTo(x: number, y: number): void;
  scrollByUser(x: number, y: number): void;
  setViewport(x: number, y: number): void;
  requestAnimationFrame(callback: FrameRequestCallback): number;
  cancelAnimationFrame(handle: number): void;
  followFragment(hash: string, dispatchPop?: boolean, beforeHashChange?: () => void): void;
  commitFrame(): void;
}

function createFakeWindow(initialUrl: string, initialState: unknown): FakeWindow {
  const location = new URL(initialUrl, "https://auction.test");
  const events = new EventTarget();
  const entries: { url: string; state: unknown }[] = [{ url: location.href, state: initialState }];
  let index = 0;
  let scroll = { x: 0, y: 0 };
  let nextFrame = 0;
  const frames = new Map<number, FrameRequestCallback>();
  const setEntry = (state: unknown, url?: string | null): void => {
    location.href = new URL(url ?? location.href, location).href;
    entries[index] = { url: location.href, state };
  };
  const traverse = (destination: number): void => {
    const target = entries[destination];
    if (target !== undefined) {
      index = destination;
      location.href = target.url;
      events.dispatchEvent(new Event("popstate"));
    }
  };
  const history: FakeHistory = {
    get state() {
      return entries[index]?.state;
    },
    scrollRestoration: "auto",
    pushState(nextState, _title, url) {
      index += 1;
      entries.splice(index);
      setEntry(nextState, url);
    },
    replaceState(nextState, _title, url) {
      setEntry(nextState, url);
    },
    back: () => traverse(index - 1),
    forward: () => traverse(index + 1),
  };
  const fake = Object.assign(events, {
    location,
    history,
    scrollX: 0,
    scrollY: 0,
    scrollTo(x: number | ScrollToOptions, y?: number) {
      scroll = typeof x === "number" ? { x, y: y ?? 0 } : { x: x.left ?? 0, y: x.top ?? 0 };
      events.dispatchEvent(new Event("scroll"));
    },
    setViewport(x: number, y: number) {
      scroll = { x, y };
    },
    scrollByUser(x: number, y: number) {
      events.dispatchEvent(new Event("wheel"));
      fake.scrollTo(x, y);
    },
    requestAnimationFrame(callback: FrameRequestCallback): number {
      nextFrame += 1;
      frames.set(nextFrame, callback);
      return nextFrame;
    },
    cancelAnimationFrame(handle: number) {
      frames.delete(handle);
    },
    followFragment(hash: string, dispatchPop = true, beforeHashChange?: () => void) {
      const oldURL = location.href;
      history.pushState(null, "", hash);
      if (dispatchPop) {
        events.dispatchEvent(new Event("popstate"));
      }
      beforeHashChange?.();
      events.dispatchEvent(
        Object.assign(new Event("hashchange"), { oldURL, newURL: location.href }),
      );
    },
    commitFrame() {
      const callbacks = [...frames.values()];
      frames.clear();
      for (const callback of callbacks) {
        callback(0);
      }
    },
  });
  Object.defineProperties(fake, {
    scrollX: { get: () => scroll.x },
    scrollY: { get: () => scroll.y },
  });
  return fake;
}

interface OpenRouter {
  readonly router: Router;
  readonly fake: FakeWindow;
  disposeRouter(): void;
  restore(): void;
}

/** 装配能模拟同文档历史导航的路由替身。 */
export function openRouter(initialUrl: string, initialState: unknown = null): OpenRouter {
  const fake = createFakeWindow(initialUrl, initialState);
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

/** 提供可延迟挂载章节和调整文档高度的观察器替身。 */
export function installScrollDocument() {
  const mutations = new Set<() => void>();
  const seen: string[] = [];
  const headings = new Map<string, { scrollIntoView(): void; getClientRects(): object[] }>();
  const scrollRoot = { scrollWidth: 100, clientWidth: 100, scrollHeight: 1000, clientHeight: 100 };
  const properties = {
    document: {
      body: {},
      scrollingElement: scrollRoot,
      getElementById: (id: string) => headings.get(id) ?? null,
    },
    MutationObserver: class {
      constructor(private readonly callback: () => void) {}

      observe(): void {
        mutations.add(this.callback);
      }

      disconnect(): void {
        mutations.delete(this.callback);
      }
    },
  };
  const saved = Object.entries(properties).map(([name, value]) => {
    const original = Object.getOwnPropertyDescriptor(globalThis, name);
    Object.defineProperty(globalThis, name, { configurable: true, value });
    return { name, original };
  });
  const notify = () => {
    for (const mutation of [...mutations]) {
      mutation();
    }
  };
  return {
    seen,
    observers: () => mutations.size,
    mount(id: string) {
      headings.set(id, {
        scrollIntoView: () => seen.push(id),
        getClientRects: () => [{}],
      });
      notify();
    },
    resize(maxScroll: number) {
      scrollRoot.scrollHeight = maxScroll + scrollRoot.clientHeight;
      notify();
    },
    restore() {
      for (const { name, original } of saved) {
        if (original === undefined) {
          Reflect.deleteProperty(globalThis, name);
        } else {
          Object.defineProperty(globalThis, name, original);
        }
      }
    },
  };
}
