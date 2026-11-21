// 验证共享能力的订阅生命周期，以及导航的读取与按需加载。

import { expect, test } from "bun:test";
import { createComponent, createRoot, type Element } from "solid-js";
import { findElement } from "../catalog/command-scope-fixture.test";
import { useConsignmentCapability } from "./capability";
import {
  controlEvent,
  failure,
  fakeClock,
  fakeFetch,
  mountConsignment,
  pageResponse,
  response,
  STATUS_PATH,
  settle,
  status,
  summary,
} from "./read-fixture.test";
import { createElement, loadedPageChunks, loadNavigation } from "./render-fixture.test";

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

test("关闭和失败时三套导航不占位、原页照常渲染，开启也只在点击后载入目录", async () => {
  const initiallyLoaded = loadedPageChunks();
  const { NavigationLinks, Navigation, ConsoleSidebar, RouteOutlet, PUBLIC_PAGES } =
    await loadNavigation();
  expect(PUBLIC_PAGES.ConsignmentCatalog?.kind).toBe("lazy");
  expect(PUBLIC_PAGES.ConsignmentDetail?.kind).toBe("lazy");
  expect(loadedPageChunks()).toEqual(initiallyLoaded);
  const clock = fakeClock();

  try {
    for (const [answer, enabled] of [
      [response(status(false)), false],
      [failure(503, "consignment_unavailable"), false],
      [response(status(true, false)), true],
    ] as const) {
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
        expect(
          page.find((node) => node.attributes["data-auction-page"] === "loaded"),
        ).not.toBeNull();
        expect(transport.requests.map(({ url }) => url.pathname)).toEqual([STATUS_PATH]);
        expect(entry()).toBeNull();
        expect(clock.activeIn(30_000)).toBe(0);
        pending.resolve(answer);
        await settle();
        expect(loadedPageChunks()).toEqual(initiallyLoaded);

        for (const surface of ["desktop", "mobile", "console"]) {
          const nav = page.find((node) => node.attributes["data-navigation"] === surface);
          expect(nav).not.toBeNull();
          expect(findElement(nav, (node) => node.attributes.href === "/consignment") !== null).toBe(
            enabled,
          );
        }
        if (enabled) {
          expect(entry()).not.toBeNull();
          controlEvent(entry(), "onClick");
          for (let step = 0; step < 10 && !page.text().includes("Fake Oak Staff"); step += 1) {
            await Bun.sleep(0);
            await settle();
          }
          expect(page.text()).toContain("Fake Oak Staff");
          expect(loadedPageChunks()).toContain("catalog");
          expect(transport.requests.filter(({ url }) => url.pathname === STATUS_PATH)).toHaveLength(
            1,
          );
        }
      } finally {
        page.dispose();
        transport.restore();
      }
    }
  } finally {
    clock.restore();
  }
});
