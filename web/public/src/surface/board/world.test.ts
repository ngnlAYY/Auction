// Public 世界目录辅助与切换控制器：历史筛选保留关闭世界，按钮只消费配置开放的世界。

import { expect, test } from "bun:test";
import type { WorldsResponse } from "@auction/contracts/dto/public/account";
import { type Cell, createCell } from "@auction/web-shared/reactive/cell";
import type { RouteMatch, Router } from "@auction/web-shared/router";
import { RouterContext } from "@auction/web-shared/router";
import { createEffect, createRoot, flush } from "solid-js";

import type { PublicBoardQuery } from "../query/board";
import { readPublicQuery } from "../query/location";
import { createWorldSwitch, supportedWorlds, type WorldSwitch } from "./world";

function worldsResponse(entries: readonly (readonly [string, boolean])[]): WorldsResponse {
  return {
    worlds: Object.fromEntries(entries.map(([world, enabled]) => [world, { enabled }])),
  };
}

test("supported_worlds_keeps_closed_worlds_and_filters_unknown_keys：关闭世界的历史仍可筛选", () => {
  const worlds = worldsResponse([
    ["isekai", false],
    ["unknown", true],
    ["main", true],
  ]);
  expect(supportedWorlds(worlds)).toEqual(["main", "isekai"]);
});

test("supported_worlds_still_returns_disabled_supported_worlds：全部受支持世界关闭时目录键仍保留", () => {
  expect(
    supportedWorlds(
      worldsResponse([
        ["main", false],
        ["isekai", false],
      ]),
    ),
  ).toEqual(["main", "isekai"]);
});

interface FakeHistory {
  readonly state: unknown;
  pushState(state: unknown, title: string, url?: string | null): void;
  replaceState(state: unknown, title: string, url?: string | null): void;
}

interface FakeWindow {
  readonly location: { pathname: string; search: string; hash: string };
  readonly history: FakeHistory;
  failHistoryWrites(fail: boolean): void;
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
  };
}

function fakeRouter(routeName: string): Router {
  const matched: RouteMatch = { kind: "matched", name: routeName, params: {}, query: {} };
  return {
    current: () => matched,
    currentLocationString: () => "/",
    push() {},
    replace() {},
    goBack() {},
    restoreScrollAfterCommit() {},
  };
}

function installWorldsFetch(worlds: WorldsResponse): () => void {
  const previous = globalThis.fetch;
  globalThis.fetch = Object.assign(
    async () =>
      new Response(JSON.stringify({ code: "ok", message: "ok", data: worlds }), {
        status: 200,
        headers: { "content-type": "application/json" },
      }),
    { preconnect: previous.preconnect },
  );
  return () => {
    globalThis.fetch = previous;
  };
}

interface MountedWorldSwitch {
  readonly controller: WorldSwitch;
  readonly selected: Cell<string>;
  readonly query: Cell<PublicBoardQuery>;
  selectedUpdates(): number;
  browserSearch(): string;
  failHistoryWrites(fail: boolean): void;
  dispose(): void;
}

interface MountOptions {
  readonly routeName: string;
  readonly worlds: WorldsResponse;
  readonly initialUrl?: string;
  readonly failHistoryWrites?: boolean;
}

function mountWorldSwitch(options: MountOptions): MountedWorldSwitch {
  const fakeWindow = createFakeWindow(options.initialUrl ?? "/");
  if (options.failHistoryWrites === true) {
    fakeWindow.failHistoryWrites(true);
  }
  const previousWindow = Object.getOwnPropertyDescriptor(globalThis, "window");
  Object.defineProperty(globalThis, "window", {
    configurable: true,
    writable: true,
    value: fakeWindow,
  });
  const restoreFetch = installWorldsFetch(options.worlds);
  const previousRouterDefault = RouterContext.defaultValue;
  RouterContext.defaultValue = fakeRouter(options.routeName);
  const initialQuery = readPublicQuery();
  const selected = createCell(initialQuery.world === "isekai" ? "isekai" : "main");
  const query = createCell(initialQuery);
  let selectedUpdates = 0;
  let controller!: WorldSwitch;
  let disposeRoot = (): void => {};
  createRoot((dispose) => {
    disposeRoot = dispose;
    controller = createWorldSwitch({ selectedWorld: selected, shellQueryState: query });
    createEffect(
      () => selected(),
      () => {
        selectedUpdates += 1;
      },
    );
  });
  flush();
  let closed = false;
  return {
    controller,
    selected,
    query,
    selectedUpdates: () => selectedUpdates,
    browserSearch: () => fakeWindow.location.search,
    failHistoryWrites: (fail) => fakeWindow.failHistoryWrites(fail),
    dispose() {
      if (closed) {
        return;
      }
      closed = true;
      disposeRoot();
      flush();
      RouterContext.defaultValue = previousRouterDefault;
      restoreFetch();
      if (previousWindow === undefined) {
        Reflect.deleteProperty(globalThis, "window");
      } else {
        Object.defineProperty(globalThis, "window", previousWindow);
      }
    },
  };
}

async function waitFor(predicate: () => boolean): Promise<void> {
  for (let attempt = 0; attempt < 200; attempt += 1) {
    flush();
    if (predicate()) {
      return;
    }
    await Promise.resolve();
  }
  throw new Error("世界控制器没有在等待窗口内完成读取或纠偏");
}

async function flushWorlds(): Promise<void> {
  for (let attempt = 0; attempt < 32; attempt += 1) {
    await Promise.resolve();
    flush();
  }
}

test("world_switch_controller_filters_disabled_worlds_and_orders_main_first：控制器按配置过滤并固定主世界在前", async () => {
  const onlyMain = mountWorldSwitch({
    routeName: "AuctionIndex",
    worlds: {
      worlds: {
        isekai: { enabled: false },
        main: { enabled: true },
      },
    },
  });
  try {
    await waitFor(() => !onlyMain.controller.selectable());
    expect(onlyMain.controller.options()).toEqual([{ value: "main", disabled: false }]);
  } finally {
    onlyMain.dispose();
  }

  const both = mountWorldSwitch({
    routeName: "AuctionIndex",
    worlds: {
      worlds: {
        isekai: { enabled: true },
        main: { enabled: true },
      },
    },
  });
  try {
    await flushWorlds();
    expect(both.controller.options()).toEqual([
      { value: "main", disabled: false },
      { value: "isekai", disabled: false },
    ]);
    expect(both.controller.selectable()).toBe(true);
  } finally {
    both.dispose();
  }
});

test("world_switch_choose_keeps_the_old_world_when_history_write_fails：写入失败不提交选择且可重试", async () => {
  const mounted = mountWorldSwitch({
    routeName: "AuctionIndex",
    worlds: {
      worlds: {
        main: { enabled: true },
        isekai: { enabled: true },
      },
    },
  });
  try {
    await flushWorlds();
    const before = mounted.query.peek();
    mounted.failHistoryWrites(true);
    mounted.controller.choose("isekai");
    flush();
    expect(mounted.selected.peek()).toBe("main");
    expect(mounted.query.peek()).toBe(before);
    expect(mounted.controller.current()).toBe("main");
    expect(new URLSearchParams(mounted.browserSearch()).get("world")).toBeNull();

    mounted.failHistoryWrites(false);
    mounted.controller.choose("isekai");
    flush();
    expect(mounted.selected.peek()).toBe("isekai");
    expect(mounted.query.peek().world).toBe("isekai");
    expect(mounted.controller.current()).toBe("isekai");
    expect(new URLSearchParams(mounted.browserSearch()).get("world")).toBe("isekai");
  } finally {
    mounted.dispose();
  }
});

test("world_switch_choose_does_not_notify_an_unchanged_single_world：全部与主世界之间切换不重复通知", async () => {
  const mounted = mountWorldSwitch({
    routeName: "History",
    worlds: {
      worlds: {
        main: { enabled: true },
        isekai: { enabled: true },
      },
    },
  });
  try {
    await flushWorlds();
    const updates = mounted.selectedUpdates();
    mounted.controller.choose("main");
    flush();
    expect(mounted.query.peek().world).toBe("main");
    mounted.controller.choose("all");
    flush();
    expect(mounted.query.peek().world).toBeNull();
    expect(mounted.controller.current()).toBe("all");
    expect(mounted.selectedUpdates()).toBe(updates);
  } finally {
    mounted.dispose();
  }
});

test("forum_correction_failure_can_retry_on_the_same_controller：论坛纠偏失败后同一实例可重试", async () => {
  const auctionId = "67e55044-10b1-426f-9247-bb680e5fe0c8";
  const initialUrl = `/forum?world=isekai&auction=${auctionId}`;
  const worlds: WorldsResponse = {
    worlds: {
      isekai: { enabled: false },
      main: { enabled: true },
    },
  };
  const mounted = mountWorldSwitch({
    routeName: "Forum",
    worlds,
    initialUrl,
    failHistoryWrites: true,
  });
  try {
    await waitFor(
      () =>
        !mounted.controller.selectable() &&
        mounted.controller.options().some((option) => option.disabled),
    );
    // 写入失败后当前项仍是地址里的异世界；开放的主世界可作为重试入口。
    expect(mounted.selected.peek()).toBe("isekai");
    expect(mounted.query.peek().world).toBe("isekai");
    expect(new URLSearchParams(mounted.browserSearch()).get("world")).toBe("isekai");
    expect(mounted.controller.current()).toBe("isekai");
    expect(mounted.controller.options()).toEqual([
      { value: "main", disabled: false },
      { value: "isekai", disabled: true },
    ]);

    mounted.failHistoryWrites(false);
    mounted.controller.choose("main");
    await waitFor(() => mounted.controller.current() === "main");
    expect(mounted.selected.peek()).toBe("main");
    expect(mounted.query.peek().world).toBeNull();
    expect(new URLSearchParams(mounted.browserSearch()).get("world")).toBeNull();
    // 纠偏成功后恢复“只有一个可选项时隐藏”的规则。
    expect(mounted.controller.options()).toEqual([{ value: "main", disabled: false }]);
  } finally {
    mounted.dispose();
  }
});
