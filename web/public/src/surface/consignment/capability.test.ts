// 验证共享能力的订阅生命周期，以及导航的读取与按需加载。

import { expect, test } from "bun:test";
import { createComponent, createRoot, type Element } from "solid-js";
import { findElement } from "../catalog/command-scope-fixture.test";
import { consignmentMenuEntries, useConsignmentCapability } from "./capability";
import { pageResponse, summary } from "./order-fixture.test";
import {
  controlEvent,
  failure,
  fakeClock,
  fakeFetch,
  mountConsignment,
  response,
  STATUS_PATH,
  settle,
  status,
} from "./read-fixture.test";
import { createElement, loadNavigation, requestedPageChunks } from "./render-fixture.test";

test("首订阅离开后仍可刷新，末订阅取消在途读取，再订阅重新读取", async () => {
  const pending = Promise.withResolvers<Response>();
  let answer: () => Response | Promise<Response> = () => response(status(true));
  const transport = fakeFetch(() => answer());
  const subscribe = () => createRoot((dispose) => ({ dispose, read: useConsignmentCapability() }));
  const first = subscribe();
  const second = subscribe();
  try {
    await settle();
    expect(transport.requests.map(({ url }) => url.pathname)).toEqual([STATUS_PATH]);
    expect(first.read()?.kind).toBe("ready");
    expect(second.read()?.kind).toBe("ready");

    first.dispose();
    answer = () => response(status(false));
    second.read.refresh();
    await settle();
    expect(transport.requests).toHaveLength(2);
    expect(second.read()?.kind).toBe("disabled");

    answer = () => pending.promise;
    second.read.refresh();
    await settle();
    expect(transport.requests).toHaveLength(3);
    const signal = transport.requests[2]?.init?.signal;
    expect(signal?.aborted).toBe(false);
    second.dispose();
    expect(signal?.aborted).toBe(true);

    answer = () => response(status(true, false));
    const next = subscribe();
    try {
      expect(next.read()).toBeUndefined();
      await settle();
      expect(transport.requests).toHaveLength(4);
      expect(next.read()?.kind).toBe("read-only");
    } finally {
      next.dispose();
    }
  } finally {
    first.dispose();
    second.dispose();
    pending.resolve(response(status(false)));
    await settle();
    transport.restore();
  }
});

test.each([
  ["关闭", () => response(status(false)), false],
  ["查询失败", () => failure(503, "consignment_unavailable"), false],
  ["ready", () => response(status(true, true)), true],
  ["read-only", () => response(status(true, false)), true],
] as const)("能力查询待定与%s时的三套导航和页面加载", async (kind, answer, enabled) => {
  const initialRequestCount = requestedPageChunks().length;
  const requestsSinceStart = () => requestedPageChunks().slice(initialRequestCount);
  const { NavigationLinks, Navigation, ConsoleSidebar, RouteOutlet, PUBLIC_PAGES } =
    await loadNavigation();
  for (const routeName of [
    "ConsignmentCatalog",
    "ConsignmentDetail",
    "ConsignmentRegistration",
    "ConsignmentMine",
    "ConsignmentOwnedDetail",
  ]) {
    expect(PUBLIC_PAGES[routeName]?.kind).toBe("lazy");
  }
  expect(requestsSinceStart()).toEqual([]);
  const clock = fakeClock();

  try {
    const pending = Promise.withResolvers<Response>();
    const transport = fakeFetch(({ url }) =>
      url.pathname === STATUS_PATH ? pending.promise : pageResponse(url, [summary()]),
    );
    const route = { kind: "matched", name: "AuctionIndex", params: {}, query: {} } as const;
    const navProps = {
      currentRoute: route,
      scopedHref: (path: string) => `${path}?world=main`,
      auctionHomeHref: "/",
      resetHomeQuery: false,
      onHome() {},
    };
    const page = await mountConsignment("/", () => {
      const surfaces = [
        ["desktop", createComponent(NavigationLinks, navProps)],
        [
          "mobile",
          createComponent(Navigation, {
            ...navProps,
            open: false,
            onToggle() {},
            onClose() {},
          }),
        ],
        [
          "console",
          createComponent(ConsoleSidebar, {
            ...navProps,
            collapsed: false,
            onToggle() {},
            onNavigate() {},
          }),
        ],
      ] as const;
      const navigations = surfaces.map(([name, subtree]) => {
        const nav = createElement("div", { "data-navigation": name });
        nav.nodes.push(() => subtree);
        return nav;
      });
      return [
        ...navigations,
        createComponent(RouteOutlet, {
          pages: {
            ...PUBLIC_PAGES,
            AuctionIndex: {
              kind: "component",
              component: () => createElement("section", { "data-auction-page": "loaded" }),
            },
          },
          notFound: null,
        }),
      ] satisfies Element;
    });
    const entry = () => page.find((node) => node.attributes.href === "/consignment");
    try {
      await settle();
      expect(page.find((node) => node.attributes["data-auction-page"] === "loaded")).not.toBeNull();
      expect(transport.requests.map(({ url }) => url.pathname)).toEqual([STATUS_PATH]);
      expect(entry()).toBeNull();
      expect(requestsSinceStart()).toEqual([]);
      expect(clock.activeIn(30_000)).toBe(0);
      pending.resolve(answer());
      await settle();
      expect(requestsSinceStart()).toEqual([]);

      for (const surface of ["desktop", "mobile", "console"]) {
        const nav = page.find((node) => node.attributes["data-navigation"] === surface);
        expect(nav).not.toBeNull();
        expect(findElement(nav, (node) => node.attributes.href === "/consignment") !== null).toBe(
          enabled,
        );
      }
      // 最后一组才打开目录，避免已缓存的页面掩盖其他状态的提前加载。
      if (kind === "read-only") {
        expect(entry()).not.toBeNull();
        controlEvent(entry(), "onClick");
        for (let step = 0; step < 10 && !page.text().includes("Fake Oak Staff"); step += 1) {
          await Bun.sleep(0);
          await settle();
        }
        expect(page.text()).toContain("Fake Oak Staff");
        expect(requestsSinceStart()).toEqual(["catalog"]);
        expect(transport.requests.filter(({ url }) => url.pathname === STATUS_PATH)).toHaveLength(
          1,
        );
      }
    } finally {
      page.dispose();
      transport.restore();
    }
  } finally {
    clock.restore();
  }
});

test("账号弹出框寄售信息：只有我的挂单与我的订单，登记入口不在其中", () => {
  expect(consignmentMenuEntries({ kind: "ready", status: status(true) })).toEqual([
    { href: "/consignment/me", labelKey: "consignment-mine" },
    { href: "/consignment/orders", labelKey: "consignment-orders" },
  ]);
  expect(
    consignmentMenuEntries({ kind: "read-only", status: status(true, false) }).map((e) => e.href),
  ).toEqual(["/consignment/me", "/consignment/orders"]);
});

test("账号弹出框寄售信息：关闭、读取失败或能力未知时整个分组为空", () => {
  expect(consignmentMenuEntries({ kind: "disabled", status: status(false) })).toEqual([]);
  expect(consignmentMenuEntries({ kind: "failed", error: { kind: "unavailable" } })).toEqual([]);
  expect(consignmentMenuEntries(undefined)).toEqual([]);
});
