// 账户出价和代理刷新时，查看入口随记录标识复用。

import { expect, spyOn, test } from "bun:test";
import { createCell } from "@auction/web-shared/reactive/cell";
import { ok } from "@auction/web-shared/result";
import { createRoot, For, flush, mapArray } from "solid-js";

import {
  PublicCommandOwnerContext,
  PublicSessionResourceContext,
  type PublicSessionResult,
} from "../index";
import { mountReadPage } from "../pagination/browser-fixture";
import { page as accountPage, ownedBidRowKey, ownedProxyRowKey } from ".";

const FIRST_ROW = { id: "bid-1", authorization_id: "proxy-1", lot_id: "lot-1" };

test.each([
  ["出价", ownedBidRowKey],
  ["代理", ownedProxyRowKey],
] as const)("%s 列表按记录身份保留查看目标，同拍品的另一条记录另建行", (_name, keyed) => {
  const newer = { id: "bid-2", authorization_id: "proxy-2", lot_id: "lot-2" };
  const replacement = { ...FIRST_ROW, id: "bid-3", authorization_id: "proxy-3" };
  const rows = createCell([FIRST_ROW]);
  createRoot((dispose) => {
    try {
      const view = mapArray(rows, (row) => ({ viewLotId: () => row().lot_id }), { keyed });
      const [focused] = view();
      expect(focused?.viewLotId()).toBe("lot-1");

      rows.set([newer, { ...FIRST_ROW }]);
      flush();
      const [newRow, oldRow] = view();
      expect(oldRow).toBe(focused);
      expect(focused?.viewLotId()).toBe("lot-1");
      expect(newRow?.viewLotId()).toBe("lot-2");

      rows.set([newer, replacement]);
      flush();
      expect(view()[0]).toBe(newRow);
      expect(view()[1]).not.toBe(focused);
      expect(view()[1]?.viewLotId()).toBe("lot-1");
    } finally {
      dispose();
    }
  });
});

test.each([
  [1001, 7, 1001, 7],
  [1002, 7, 1001, 7],
  [7, 1002, 7, 1001],
])("账户入口双表第 %i、%i 页只发一次有效 GET", async (bidPage, proxyPage, bid, proxy) => {
  const user = "00000000-0000-4000-8000-000000000002";
  const originalSession = PublicSessionResourceContext.defaultValue;
  const originalCommandOwner = PublicCommandOwnerContext.defaultValue;
  const session = Object.assign(
    createCell<PublicSessionResult>(
      ok({
        authenticated: true,
        user_id: user,
        username: "viewer",
        expires_at: null,
        absolute_expires_at: null,
      }),
    ),
    {
      state: () => "ready" as const,
      restart() {},
      cancel() {},
      clear() {},
    },
  );
  const fetcher = spyOn(globalThis, "fetch").mockImplementation(
    Object.assign(
      async () =>
        Response.json({
          code: "ok",
          message: "ok",
          data: {
            bids: [],
            proxies: [],
            bid_total: 10_010,
            proxy_total: 10_010,
            page: bid,
            bid_page: bid,
            proxy_page: proxy,
            page_size: 10,
          },
        }),
      { preconnect() {} },
    ),
  );
  const mounted = mountReadPage(
    `/me?bid_page=${bidPage}&proxy_page=${proxyPage}`,
    () => {
      PublicSessionResourceContext.defaultValue = session;
      PublicCommandOwnerContext.defaultValue = createCell<string | null>(null);
      accountPage();
    },
    ({ tag, props, children }) => {
      // 走账户入口的已认证分支，只装配受保护的出价与代理表。
      if (tag === For && Array.isArray(props.each) && props.each[0] === user) {
        const item = children[0];
        if (typeof item === "function") item(() => user);
      }
      if (
        typeof tag === "function" &&
        ["AccountReadScope", "AuthenticatedAccountData"].includes(tag.name)
      ) {
        Reflect.apply(tag, undefined, [props]);
      }
    },
  );
  try {
    await Bun.sleep(0);
    flush();
    const urls = fetcher.mock.calls.map(([input]) => new URL(String(input), mounted.location));
    expect(
      urls.filter((url) => url.pathname === "/api/v1/me/bids").map((url) => url.search),
    ).toEqual([`?bid_page=${bid}&page_size=10&proxy_page=${proxy}`]);
    expect(mounted.location.searchParams.get("bid_page")).toBe(String(bid));
    expect(mounted.location.searchParams.get("proxy_page")).toBe(String(proxy));
  } finally {
    mounted.dispose();
    PublicSessionResourceContext.defaultValue = originalSession;
    PublicCommandOwnerContext.defaultValue = originalCommandOwner;
    fetcher.mockRestore();
  }
});
