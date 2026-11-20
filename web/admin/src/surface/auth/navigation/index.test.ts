// 侧栏详情路由归属父分区，顶级路由只高亮自身。

import { expect, test } from "bun:test";
import { createRouter } from "@auction/web-shared/router";
import type { RouteMatch } from "@auction/web-shared/router/pattern";
import { installFakeWindow } from "@auction/web-shared/router/test-window";
import { createRoot, flush } from "solid-js";
import { loadRenderModule } from "../../../solid-render-fixture.test";
import { ADMIN_ROUTES } from "../../router/route";
import { createComponent, mountNode } from "../../solid-dom-fixture";
import { installAuthDocument } from "../auth-environment-fixture";
import { installAuthLocale } from "../auth-locale-fixture.test";

const route = (name: string, params: Record<string, string> = {}): RouteMatch => ({
  kind: "matched",
  name,
  params,
  query: {},
});

async function linksFor(currentRoute: RouteMatch) {
  const { NavigationHarness } = await loadRenderModule<typeof import("./navigation-fixture")>(
    new URL("./navigation-fixture.tsx", import.meta.url).pathname,
    "NavigationHarness",
  );
  const locale = await installAuthLocale();
  let restoreWindow = (): void => {};
  let restoreDocument = (): void => {};
  let disposeRoot = (): void => {};
  try {
    restoreWindow = installFakeWindow("/auctions").restore;
    const installed = installAuthDocument();
    restoreDocument = installed.restore;
    const { document } = installed;
    createRoot((dispose) => {
      disposeRoot = dispose;
      const router = createRouter({ routes: ADMIN_ROUTES });
      mountNode(document, createComponent(NavigationHarness, { router, currentRoute }));
    });
    flush();
    return document.querySelectorAll("a").map((element) => ({
      href: element.getAttribute("href"),
      current: element.getAttribute("aria-current"),
    }));
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
          locale.restore();
        }
      }
    }
  }
}

test("detail_routes_highlight_their_parent_section：详情路由高亮父分区", async () => {
  const lots = await linksFor(route("AuctionLots", { auction_id: "auction" }));
  expect(lots.find((link) => link.href === "/auctions")?.current).toBe("page");
  expect(lots.find((link) => link.href === "/deliveries")?.current).toBe("false");
  expect(lots.find((link) => link.href === "/settlement-returns")?.current).toBe("false");

  const delivery = await linksFor(route("DeliveryDetail", { delivery_id: "delivery" }));
  expect(delivery.find((link) => link.href === "/deliveries")?.current).toBe("page");
  expect(delivery.find((link) => link.href === "/auctions")?.current).toBe("false");

  const ret = await linksFor(route("SettlementReturnDetail", { return_id: "case" }));
  expect(ret.find((link) => link.href === "/settlement-returns")?.current).toBe("page");
  expect(ret.find((link) => link.href === "/deliveries")?.current).toBe("false");
});

test("top_level_routes_highlight_themselves：顶级路由高亮自身", async () => {
  for (const [name, href] of [
    ["Dashboard", "/"],
    ["Alerts", "/alerts"],
    ["Auctions", "/auctions"],
    ["Audit", "/audit"],
  ] as const) {
    const links = await linksFor(route(name));
    expect(links.find((link) => link.href === href)?.current).toBe("page");
  }
});
