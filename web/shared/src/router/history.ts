// 同文档历史条目的落点与地址写入。

import { onCleanup } from "solid-js";

import { createUuidV4 } from "../platform/uuid";
import { notifyLocationWritten } from "./location-notice";
import type { ScrollPosition, ScrollTarget } from "./scroll";
import { fragmentTarget, hasFragmentTarget, restoreScroll, sameScrollPosition } from "./scroll";

type WriteMode = "push" | "replace";

interface HistorySnapshot {
  readonly key: string | null;
  readonly landing: ScrollTarget;
}

interface HistoryEntry {
  readonly key: string;
  landing: ScrollTarget;
}

const TOP: ScrollPosition = { x: 0, y: 0 };
const SCROLL_INPUT_WINDOW_MS = 250;
const SCROLL_KEYS = new Set([
  "ArrowDown",
  "ArrowLeft",
  "ArrowRight",
  "ArrowUp",
  "End",
  "Home",
  "PageDown",
  "PageUp",
  " ",
]);
const queryWriters = new WeakMap<Window, (route: string, mode: WriteMode) => boolean>();

/** 浏览器历史适配器。 */
export interface WebHistory {
  currentRoute(): string;
  currentEntryKey(): string;
  navigate(route: string, mode: WriteMode): boolean;
  goBack(): void;
  restoreScrollAfterCommit(): void;
}

/** 原生导航完成后刷新路由匹配；滚动由历史条目管理。 */
export interface WebHistoryOptions {
  readonly onLocationChanged: () => void;
}

function currentWindowRoute(includeEmptyQuery = false): string {
  const location = window.location;
  const { pathname, search, hash } = location;
  // location.search 将末尾的裸问号读成空串，比较写入地址时仍须区分。
  const emptyQuery =
    includeEmptyQuery && search === "" && location.href?.split("#", 1)[0]?.endsWith("?") ? "?" : "";
  return `${pathname || "/"}${search || emptyQuery}${hash}`;
}

function currentScroll(): ScrollPosition {
  return { x: window.scrollX ?? 0, y: window.scrollY ?? 0 };
}

function positionTarget(position: ScrollPosition): ScrollTarget {
  return { kind: "position", position };
}

function fragmentInRoute(route: string): ScrollTarget | null {
  const hash = route.indexOf("#");
  return hash === -1 ? null : fragmentTarget(route.slice(hash));
}

function readEntry(): HistorySnapshot | null {
  let state: unknown;
  try {
    state = window.history.state;
  } catch {
    return null;
  }
  if (!Array.isArray(state)) {
    return null;
  }
  const [first, second, rawKey]: unknown[] = state;
  const key = typeof rawKey === "string" ? rawKey : null;
  if (first === "fragment" && typeof second === "string" && key !== null) {
    return { key, landing: { kind: "fragment", id: second } };
  }
  if (
    typeof first === "number" &&
    typeof second === "number" &&
    Number.isFinite(first) &&
    Number.isFinite(second)
  ) {
    return { key, landing: positionTarget({ x: first, y: second }) };
  }
  return null;
}

function writeEntry(
  mode: WriteMode,
  entry: HistorySnapshot,
  route?: string,
  onWritten?: () => void,
): boolean {
  try {
    const { key, landing } = entry;
    const entryKey = key ?? createUuidV4();
    // 坐标继续兼容旧 [x, y]；未完成的片段在该条目里单独存为目标。
    const state =
      landing.kind === "fragment"
        ? ["fragment", landing.id, entryKey]
        : [landing.position.x, landing.position.y, entryKey];
    if (mode === "push") {
      window.history.pushState(state, "", route);
    } else {
      window.history.replaceState(state, "", route);
    }
  } catch {
    // 浏览器拒绝写入时，查询状态仍以原地址为准。
    return false;
  }
  onWritten?.();
  if (route !== undefined) {
    // 成功写入地址并登记条目后才发布快照，确保路由快照、已应用查询与地址取自同一次写入。
    notifyLocationWritten();
  }
  return true;
}

/** 查询 replace 沿用当前落点；push 继承同页待定位的片段或从当前视口开始。 */
export function writeLocationPreservingScroll(route: string, mode: WriteMode): boolean {
  const writer = queryWriters.get(window);
  if (writer !== undefined) {
    return writer(route, mode);
  }
  const saved = mode === "replace" ? readEntry() : null;
  return (
    route !== currentWindowRoute(true) &&
    writeEntry(mode, saved ?? { key: null, landing: positionTarget(currentScroll()) }, route)
  );
}

/** 每个条目只拥有一个落点。未完成的片段在离开后仍可在前进或后退时继续等待。 */
export function createWebHistory(options: WebHistoryOptions): WebHistory {
  if ("scrollRestoration" in window.history) {
    window.history.scrollRestoration = "manual";
  }

  const snapshot = readEntry();
  const initialFragment = fragmentTarget(window.location.hash);
  const entries = new Map<string, HistoryEntry>();
  let active: HistoryEntry = {
    key: snapshot?.key ?? createUuidV4(),
    landing: initialFragment ?? snapshot?.landing ?? positionTarget(currentScroll()),
  };
  entries.set(active.key, active);
  if (snapshot?.key == null || initialFragment !== null) {
    void writeEntry("replace", active);
  }

  let activeRoute = currentWindowRoute();
  let waiting = initialFragment !== null || snapshot !== null;
  let stopRestore: (() => void) | null = null;
  let departurePosition: ScrollPosition | null = null;
  let nativeFragmentPending = false;
  let inputAt: number | null = null;
  let ownScroll: ScrollPosition | null = null;
  let applyingScroll = false;

  const stopWaiting = (): void => {
    stopRestore?.();
    stopRestore = null;
    waiting = false;
  };
  const activate = (entry: HistoryEntry, restore: boolean): void => {
    stopWaiting();
    active = entry;
    activeRoute = currentWindowRoute();
    entries.set(entry.key, entry);
    departurePosition = null;
    nativeFragmentPending = false;
    inputAt = null;
    waiting = restore;
  };
  const recordPosition = (position: ScrollPosition, persist: boolean): void => {
    active.landing = positionTarget(position);
    if (!persist) {
      return;
    }
    const saved = readEntry();
    if (
      saved?.key !== active.key ||
      saved.landing.kind !== "position" ||
      !sameScrollPosition(saved.landing.position, position)
    ) {
      void writeEntry("replace", active);
    }
  };
  const saveOnDeparture = (persist: boolean): void => {
    if (departurePosition !== null) {
      recordPosition(departurePosition, persist);
      departurePosition = null;
    }
  };
  const performScroll = (scroll: () => void): void => {
    const before = currentScroll();
    applyingScroll = true;
    try {
      scroll();
    } finally {
      const after = currentScroll();
      if (!sameScrollPosition(before, after)) {
        ownScroll = after;
      }
      applyingScroll = false;
    }
  };
  const noteScrollInput = (event: Event): void => {
    if (!event.defaultPrevented) {
      inputAt = performance.now();
    }
  };
  const noteScrollKey = (event: KeyboardEvent): void => {
    if (SCROLL_KEYS.has(event.key)) {
      noteScrollInput(event);
    }
  };
  const trackScroll = (): void => {
    if (applyingScroll) {
      return;
    }
    const key = readEntry()?.key ?? null;
    if (key === null ? activeRoute !== currentWindowRoute() : key !== active.key) {
      return;
    }
    const next = currentScroll();
    // 自己调用滚动后可能异步收到事件；输入事件才是用户滚动的依据。
    const fromModule = ownScroll !== null && sameScrollPosition(ownScroll, next);
    ownScroll = null;
    if (fromModule) {
      return;
    }
    // 无输入佐证的滚动也让出视口，但不能把布局截短的位置写进历史。
    stopWaiting();
    if (
      nativeFragmentPending ||
      (inputAt !== null && performance.now() - inputAt <= SCROLL_INPUT_WINDOW_MS)
    ) {
      departurePosition = next;
    }
    nativeFragmentPending = false;
  };
  const restoreScrollAfterCommit = (): void => {
    if (waiting && stopRestore === null) {
      stopRestore = restoreScroll(active.landing, performScroll, () => {
        stopWaiting();
        recordPosition(currentScroll(), true);
      });
    }
  };
  const enterFromHistory = (): void => {
    const stored = readEntry();
    if (stored?.key === active.key && activeRoute === currentWindowRoute()) {
      return;
    }
    const fragment = stored === null ? fragmentTarget(window.location.hash) : null;
    const native = fragment?.kind === "fragment" && hasFragmentTarget(fragment.id);
    const missing = fragment !== null && !native;
    // popstate 时浏览器已切换条目，来源的新落点只能先记在内存。
    saveOnDeparture(false);
    const key = stored?.key ?? createUuidV4();
    const next = entries.get(key) ?? {
      key,
      landing: stored?.landing ?? fragment ?? positionTarget(currentScroll()),
    };
    activate(next, stored !== null || missing);
    // 原生片段可能在 popstate 之后才滚动；离开前，落点仍由目标 ID 表示。
    nativeFragmentPending = native;
    if (stored?.key == null) {
      void writeEntry("replace", next);
    }
    options.onLocationChanged();
    if (missing) {
      // 原生同页链接不必等待路由重新挂载；popstate 和 hashchange 共用已登记的目标。
      restoreScrollAfterCommit();
    }
  };
  const handleHashChange = (event: HashChangeEvent): void => {
    if (event.newURL === window.location.href && activeRoute !== currentWindowRoute()) {
      enterFromHistory();
    }
  };
  const leaveDocument = (): void => saveOnDeparture(true);

  const navigateTo = (route: string, mode: WriteMode, landing: ScrollTarget, restore: boolean) => {
    if (route === currentWindowRoute(true)) {
      return false;
    }
    if (mode === "push") {
      saveOnDeparture(true);
    }
    const next = { key: mode === "push" ? createUuidV4() : active.key, landing };
    return writeEntry(mode, next, route, () => activate(next, restore));
  };
  queryWriters.set(window, (route, mode) => {
    if (mode === "push") {
      const fragment = fragmentInRoute(route);
      const pending =
        waiting &&
        active.landing.kind === "fragment" &&
        fragment?.kind === "fragment" &&
        fragment.id === active.landing.id &&
        route.split(/[?#]/u, 1)[0] === window.location.pathname;
      const written = navigateTo(
        route,
        mode,
        pending ? fragment : positionTarget(currentScroll()),
        pending,
      );
      if (written && pending) {
        // 查询写入复用页面，新条目仍要自行安排待定位的片段。
        restoreScrollAfterCommit();
      }
      return written;
    }
    // replace 只改 URL；落点仍归同一个条目，等待和用户位置都继续有效。
    return (
      route !== currentWindowRoute(true) &&
      writeEntry("replace", active, route, () => {
        activeRoute = currentWindowRoute();
      })
    );
  });

  const scrollInputs = ["wheel", "touchmove", "pointerdown"] as const;
  for (const type of scrollInputs) {
    window.addEventListener(type, noteScrollInput, { passive: true });
  }
  window.addEventListener("keydown", noteScrollKey);
  window.addEventListener("scroll", trackScroll, { passive: true });
  window.addEventListener("pagehide", leaveDocument);
  window.addEventListener("popstate", enterFromHistory);
  window.addEventListener("hashchange", handleHashChange);
  onCleanup(() => {
    for (const type of scrollInputs) {
      window.removeEventListener(type, noteScrollInput);
    }
    window.removeEventListener("keydown", noteScrollKey);
    window.removeEventListener("scroll", trackScroll);
    window.removeEventListener("pagehide", leaveDocument);
    window.removeEventListener("popstate", enterFromHistory);
    window.removeEventListener("hashchange", handleHashChange);
    stopWaiting();
    queryWriters.delete(window);
  });

  return {
    currentRoute: currentWindowRoute,
    currentEntryKey: () => active.key,
    navigate(route, mode) {
      const fragment = fragmentInRoute(route);
      if (!navigateTo(route, mode, fragment ?? positionTarget(TOP), fragment !== null)) {
        return false;
      }
      performScroll(() => window.scrollTo({ left: 0, top: 0, behavior: "instant" }));
      return true;
    },
    goBack() {
      saveOnDeparture(true);
      window.history.back();
    },
    restoreScrollAfterCommit,
  };
}
