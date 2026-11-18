// Public 世界目录辅助与切换控制器：历史筛选保留关闭世界，按钮只消费配置开放的世界。

import { expect, jest, test } from "bun:test";
import type { WorldsResponse } from "@auction/contracts/dto/public/account";
import { createPageVisibility } from "@auction/web-shared/platform/visibility";
import { createCell } from "@auction/web-shared/reactive/cell";
import { ok } from "@auction/web-shared/result";
import { createRouter, Link, RouterContext } from "@auction/web-shared/router";
import { installFakeWindow } from "@auction/web-shared/router/test-window";
import { createEffect, createRoot, flush } from "solid-js";

import {
  type PublicSessionResource,
  PublicSessionResourceContext,
  type PublicSessionResult,
} from "../index";
import { NavigationLinks } from "../presentation";
import {
  readPublicQuery,
  usePublicQueryState,
  worldScopeHref,
  worldScopeNavigationWorld,
} from "../query";
import { PUBLIC_PAGES, PUBLIC_ROUTES } from "../router/route";
import { createWorldSwitch, supportedWorlds } from "./world";

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

function worldsReply(worlds: WorldsResponse): Response {
  return Response.json({ code: "ok", message: "ok", data: worlds });
}

function worldsUnavailable(): Response {
  return Response.json({ code: "storage_unavailable", message: "", data: null }, { status: 503 });
}

function mountWorldSwitch(options: {
  readonly worlds: WorldsResponse;
  readonly initialUrl?: string;
  readonly failHistoryWrites?: boolean;
  readonly firstReadFails?: boolean;
}) {
  const { fake, restore } = installFakeWindow(options.initialUrl ?? "/");
  const visibilityDocument = Object.assign(new EventTarget(), {
    hidden: false,
    scrollingElement: null,
    getElementById: () => null,
  });
  const previousDocument = Object.getOwnPropertyDescriptor(globalThis, "document");
  Object.defineProperty(globalThis, "document", {
    configurable: true,
    value: visibilityDocument,
  });
  const previousFetch = globalThis.fetch;
  const requests: (RequestCache | undefined)[] = [];
  let respond =
    options.firstReadFails === true ? worldsUnavailable : () => worldsReply(options.worlds);
  globalThis.fetch = Object.assign(
    async (input: URL | RequestInfo, init?: RequestInit) => {
      expect(input).toBe("/api/v1/worlds");
      requests.push(init?.cache);
      return respond();
    },
    { preconnect: previousFetch.preconnect },
  );
  const previousRouterDefault = RouterContext.defaultValue;
  const mounted = createRoot((disposeRoot) => {
    const router = createRouter({ routes: PUBLIC_ROUTES });
    RouterContext.defaultValue = router;
    fake.rejectWrites(options.failHistoryWrites === true);
    const query = usePublicQueryState().state;
    const selected = createCell(readPublicQuery().world === "isekai" ? "isekai" : "main");
    const pageVisibility = createPageVisibility();
    const controller = createWorldSwitch({
      selectedWorld: selected,
      shellQueryState: query,
      pageVisibility,
    });
    pageVisibility.install(controller.revalidateIfNeeded);
    let selectedUpdates = 0;
    createEffect(selected, () => {
      selectedUpdates += 1;
    });
    flush();
    return {
      router,
      controller,
      selected,
      query,
      selectedUpdates: () => selectedUpdates,
      disposeRoot,
      pageVisibility,
    };
  });
  return {
    ...mounted,
    requests,
    location: fake.location,
    browserSearch: () => fake.location.search,
    failHistoryWrites: fake.rejectWrites,
    show(visible: boolean) {
      visibilityDocument.hidden = !visible;
      visibilityDocument.dispatchEvent(new Event("visibilitychange"));
      flush();
    },
    respondWith(reply: () => Response) {
      respond = reply;
    },
    dispose() {
      mounted.pageVisibility.cleanup();
      mounted.disposeRoot();
      flush();
      RouterContext.defaultValue = previousRouterDefault;
      globalThis.fetch = previousFetch;
      if (previousDocument === undefined) {
        Reflect.deleteProperty(globalThis, "document");
      } else {
        Object.defineProperty(globalThis, "document", previousDocument);
      }
      restore();
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

test("非法场次的地址不会被自动改成开放世界", async () => {
  const worlds = worldsResponse([
    ["main", true],
    ["isekai", false],
  ]);
  const search = "?world=isekai&auction=truncated&page=3";
  const invalid = mountWorldSwitch({ worlds, initialUrl: `/${search}` });
  try {
    await flushWorlds();
    expect(invalid.browserSearch()).toBe(search);
    expect(invalid.selected.peek()).toBe("isekai");
  } finally {
    invalid.dispose();
  }

  const current = mountWorldSwitch({
    worlds,
    initialUrl: "/?world=isekai",
  });
  try {
    await waitFor(() => current.selected.peek() === "main");
    expect(new URLSearchParams(current.browserSearch()).has("world")).toBe(false);
  } finally {
    current.dispose();
  }
});

test("世界自动纠偏只删 world，保留其他查询片段的字节", async () => {
  const retained = "%74ype=%62ad&keep=%2b%20x&keep=%7e";
  const search = `?%77orld=isekai&${retained}`;
  const mounted = mountWorldSwitch({
    initialUrl: `/${search}`,
    worlds: worldsResponse([
      ["main", true],
      ["isekai", false],
    ]),
  });
  try {
    await waitFor(() => mounted.selected.peek() === "main");
    expect(mounted.browserSearch()).toBe(`?${retained}`);
    expect(mounted.query.peek().invalidCategory).toEqual(["bad"]);
  } finally {
    mounted.dispose();
  }
});

test("world_switch_choose_does_not_notify_an_unchanged_single_world：全部与主世界之间切换不重复通知", async () => {
  const mounted = mountWorldSwitch({
    initialUrl: "/history",
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
    expect(mounted.query.peek().world).toBe("main");
    expect(new URLSearchParams(mounted.browserSearch()).get("world")).toBe("main");
    // 主动选回开放世界后恢复“只有一个可选项时隐藏”的规则。
    expect(mounted.controller.options()).toEqual([{ value: "main", disabled: false }]);
  } finally {
    mounted.dispose();
  }
});

const ONLY_MAIN = worldsResponse([
  ["main", true],
  ["isekai", false],
]);
const BOTH_WORLDS = worldsResponse([
  ["main", true],
  ["isekai", true],
]);
const DIRECTORY_TTL = 300_000;

test("可见时世界目录新鲜期内不取数，周期到达后再验证并更新选项", async () => {
  jest.useFakeTimers();
  const mounted = mountWorldSwitch({ worlds: ONLY_MAIN });
  try {
    await waitFor(() => !mounted.controller.selectable());
    mounted.respondWith(() => worldsReply(BOTH_WORLDS));
    jest.advanceTimersByTime(DIRECTORY_TTL - 1);
    mounted.show(false);
    mounted.show(true);
    await flushWorlds();
    expect(mounted.requests).toHaveLength(1);
    expect(mounted.controller.options()).toEqual([{ value: "main", disabled: false }]);

    jest.advanceTimersByTime(1);
    await flushWorlds();
    expect(mounted.requests).toHaveLength(2);
    expect(mounted.requests.at(-1)).toBe("no-cache");
    expect(mounted.controller.options()).toEqual([
      { value: "main", disabled: false },
      { value: "isekai", disabled: false },
    ]);
  } finally {
    mounted.dispose();
    jest.useRealTimers();
  }
});

test("世界目录在后台超过周期时，恢复可见只补读一次", async () => {
  jest.useFakeTimers();
  const mounted = mountWorldSwitch({ worlds: ONLY_MAIN });
  try {
    await waitFor(() => !mounted.controller.selectable());
    mounted.respondWith(() => worldsReply(BOTH_WORLDS));
    mounted.show(false);
    jest.advanceTimersByTime(DIRECTORY_TTL);
    await flushWorlds();
    expect(mounted.requests).toHaveLength(1);

    mounted.show(true);
    mounted.show(true);
    await flushWorlds();
    expect(mounted.requests).toHaveLength(2);
    expect(mounted.requests.at(-1)).toBe("no-cache");
    expect(mounted.controller.selectable()).toBe(true);
  } finally {
    mounted.dispose();
    jest.useRealTimers();
  }
});

test.each([true, false])(
  "失败的世界目录恢复可见时提前重读（首次失败 %s）",
  async (firstReadFails) => {
    jest.useFakeTimers();
    const mounted = mountWorldSwitch({ worlds: ONLY_MAIN, firstReadFails });
    try {
      await flushWorlds();
      expect(mounted.requests).toHaveLength(1);
      if (!firstReadFails) {
        mounted.respondWith(worldsUnavailable);
        jest.advanceTimersByTime(DIRECTORY_TTL);
        await flushWorlds();
        expect(mounted.requests).toHaveLength(2);
        expect(mounted.controller.options()).toEqual([{ value: "main", disabled: false }]);
      }
      const beforeVisible = mounted.requests.length;
      mounted.respondWith(() => worldsReply(firstReadFails ? ONLY_MAIN : BOTH_WORLDS));
      mounted.show(false);
      mounted.show(true);
      await flushWorlds();
      expect(mounted.requests).toHaveLength(beforeVisible + 1);
      expect(mounted.requests.at(-1)).toBe("no-cache");
      expect(mounted.controller.options()).toHaveLength(firstReadFails ? 1 : 2);
    } finally {
      mounted.dispose();
      jest.useRealTimers();
    }
  },
);

function mobileHref(mounted: ReturnType<typeof mountWorldSwitch>, target: string): string {
  const jsx = Object.getOwnPropertyDescriptor(globalThis, "jsx");
  if (jsx === undefined) {
    throw new Error("测试 JSX 转译未装配");
  }
  const links: string[] = [];
  Object.defineProperty(globalThis, "jsx", {
    ...jsx,
    value(tag: unknown, attrs: Record<string, unknown> | null) {
      if (tag === Link && typeof attrs?.href === "string") {
        links.push(attrs.href);
      }
      return null;
    },
  });
  const previousSession = PublicSessionResourceContext.defaultValue;
  try {
    createRoot((dispose) => {
      try {
        const snapshot = createCell<PublicSessionResult | undefined>(
          ok({
            authenticated: true,
            user_id: "00000000-0000-4000-8000-000000000031",
            username: "viewer",
            expires_at: null,
            absolute_expires_at: null,
          }),
        );
        const session: PublicSessionResource = Object.assign(snapshot, {
          state: (): "ready" => "ready",
          restart() {},
          cancel() {},
          clear: () => snapshot.set(undefined),
        });
        PublicSessionResourceContext.defaultValue = session;
        const route = mounted.router.current();
        const world = worldScopeNavigationWorld(
          mounted.query.peek(),
          route.kind === "matched" ? route.name : null,
        );
        NavigationLinks({
          auctionHomeHref: worldScopeHref("/", world),
          currentRoute: route,
          scopedHref: (path) => worldScopeHref(path, world),
          resetHomeQuery: false,
          onHome() {},
        });
      } finally {
        dispose();
      }
    });
  } finally {
    PublicSessionResourceContext.defaultValue = previousSession;
    Object.defineProperty(globalThis, "jsx", jsx);
  }
  const href = links.find((link) => new URL(link, mounted.location).pathname === target);
  if (href === undefined) {
    throw new Error(`手机导航缺少链接 ${target}`);
  }
  return href;
}

test.each([
  "/forum?world=isekai",
  "/?world=isekai&auction=67e55044-10b1-426f-9247-bb680e5fe0c8&page=3",
])("主动从异世界切回主世界后，进入出价记录时保留范围 %s", async (initialUrl) => {
  const mounted = mountWorldSwitch({ initialUrl, worlds: BOTH_WORLDS });
  try {
    await flushWorlds();
    mounted.controller.choose("main");
    flush();
    expect(mounted.location.searchParams.get("world")).toBe("main");
    expect(mounted.query.peek().auction.kind).toBe("unspecified");
    expect(mounted.query.peek().page).toBe(1);
    mounted.router.push(mobileHref(mounted, "/bidlog"));
    flush();
    expect(mounted.router.currentLocationString()).toBe("/bidlog?world=main");
    expect(mounted.controller.current()).toBe("main");
  } finally {
    mounted.dispose();
  }
});

test("手机从显式异世界进入我的及账户别名，再进入履约进度时继承世界", async () => {
  const mounted = mountWorldSwitch({ initialUrl: "/forum?world=isekai", worlds: BOTH_WORLDS });
  try {
    await flushWorlds();
    mounted.router.push(mobileHref(mounted, "/account"));
    flush();
    expect(mounted.router.currentLocationString()).toBe("/account?world=isekai");
    const account = PUBLIC_PAGES.Account;
    if (account?.kind !== "lazy") {
      throw new Error("账户别名路由未装配");
    }
    const renderAlias = await account.load();
    createRoot((dispose) => {
      try {
        renderAlias({ params: {}, query: {} });
        flush();
      } finally {
        dispose();
      }
    });
    expect(mounted.router.currentLocationString()).toBe("/me?world=isekai");
    mounted.router.push(mobileHref(mounted, "/fulfillment"));
    flush();
    expect(mounted.router.currentLocationString()).toBe("/fulfillment?world=isekai");
    expect(mounted.controller.current()).toBe("isekai");
  } finally {
    mounted.dispose();
  }
});

test.each([
  ["main", "isekai"],
  ["isekai", "main"],
] as const)("关闭当前世界后自动回退到 %s，下一跳不继承回退值", async (enabled, disabled) => {
  const mounted = mountWorldSwitch({
    initialUrl: `/forum?world=${disabled}`,
    worlds: worldsResponse([
      [enabled, true],
      [disabled, false],
    ]),
  });
  try {
    await waitFor(() => mounted.selected.peek() === enabled);
    expect(mounted.location.pathname).toBe("/forum");
    expect(mounted.location.searchParams.has("world")).toBe(false);
    mounted.router.push(mobileHref(mounted, "/bidlog"));
    flush();
    expect(mounted.router.currentLocationString()).toBe("/bidlog");
    expect(mounted.query.peek().world).toBeNull();
  } finally {
    mounted.dispose();
  }
});
