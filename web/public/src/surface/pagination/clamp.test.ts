// 各读取页的实际分页控件处理纠偏重试与历史导航。

import { expect, spyOn, test } from "bun:test";
import { createCell } from "@auction/web-shared/reactive/cell";
import { ok } from "@auction/web-shared/result";
import { Pager } from "@auction/web-shared/ui/components/pager";
import { LocaleSessionContext, t } from "@auction/web-shared/ui/i18n";
import { For, flush, Match } from "solid-js";

import { resetStoreForTest } from "../../api/swr-cache";
import { page as accountPage } from "../account";
import { poolPage } from "../compensation";
import { CompensationLotDetails } from "../compensation/lots";
import { CompensationPoolTable } from "../compensation/table";
import {
  PublicCommandOwnerContext,
  PublicSessionResourceContext,
  type PublicSessionResult,
} from "../index";
import { AuctionHistoryPage, AuctionList } from "../pages/history";
import { usePublicQueryState } from "../query";
import { BidLog } from "../reads/bid-log";
import { BidLogTable } from "../reads/bid-log/table";
import { Fulfillment } from "../reads/fulfillment";
import { FulfillmentTable } from "../reads/fulfillment/table";
import { History } from "../reads/history";
import { PriceHistoryTable } from "../reads/history/table";
import { PublicPager } from ".";
import { mountReadPage } from "./browser-fixture";

type MountedPage = ReturnType<typeof mountReadPage>;

const AUCTION = "00000000-0000-4000-8000-000000000001";
const USER = "00000000-0000-4000-8000-000000000002";
const SHORT_PAGE = { page: 3, page_size: 10, total: 1 };
const COMPENSATION_ROUND = {
  auction_id: AUCTION,
  auction_sequence: 1,
  world_id: "main",
  opening_balance: 0,
  tax_income: 0,
  discount_demand: 0,
  compensation_out: 0,
  closing_balance: 0,
  reversal_count: 0,
  settled_at: "2026-01-01T00:00:00Z",
};

function installAccountSession(): void {
  PublicSessionResourceContext.defaultValue = Object.assign(
    createCell<PublicSessionResult>(
      ok({
        authenticated: true,
        user_id: USER,
        username: "viewer",
        expires_at: null,
        absolute_expires_at: null,
      }),
    ),
    { state: () => "ready" as const, restart() {}, cancel() {}, clear() {} },
  );
  PublicCommandOwnerContext.defaultValue = createCell<string | null>(null);
}

const LIST_PAGES = [
  {
    name: "往期拍卖",
    address: "/auctions?page=3&page_size=10",
    render: AuctionList,
    branch: Match,
    read: { ...SHORT_PAGE, auctions: [] },
  },
  {
    name: "成交历史",
    address: "/history?name=Robe&page=3&page_size=10",
    render: History,
    branch: For,
    read: { key: "test", page: { ...SHORT_PAGE, rows: [] } },
  },
  {
    name: "出价记录",
    address: `/bidlog?auction=${AUCTION}&page=3&page_size=10`,
    render: BidLog,
    branch: Match,
    read: { ...SHORT_PAGE, bids: [] },
  },
  {
    name: "履约进度",
    address: `/fulfillment?auction=${AUCTION}&page=3&page_size=10`,
    render: Fulfillment,
    branch: Match,
    read: {
      page: { ...SHORT_PAGE, auction: { id: AUCTION, world: "main" }, lots: [] },
      staleSince: null,
    },
  },
  {
    name: "补偿池批次",
    address: `/compensation-pool?auction=${AUCTION}&page=3&movement_page=4&page_size=10`,
    render: poolPage,
    branch: Match,
    read: { ...SHORT_PAGE, rounds: [], balance: 0, updated_at: COMPENSATION_ROUND.settled_at },
  },
] as const;

const TABLES = new Set<unknown>([
  AuctionHistoryPage,
  PriceHistoryTable,
  BidLogTable,
  FulfillmentTable,
  CompensationPoolTable,
  CompensationLotDetails,
]);

function mountClampPage(address: string, render: () => void) {
  resetStoreForTest();
  const previousLocale = LocaleSessionContext.defaultValue;
  LocaleSessionContext.defaultValue = {
    current: () => "en",
    selected: () => "en",
    isReady: () => true,
    error: () => null,
    bootstrap() {},
    switch() {},
  };
  const fetcher = spyOn(globalThis, "fetch").mockImplementation(
    Object.assign(async () => new Response("{}", { status: 503 }), { preconnect() {} }),
  );
  const pagers: unknown[] = [];
  const mounted = mountReadPage(address, render, (element) => {
    const { tag, props, children } = element;
    if (tag === For && Array.isArray(props.each) && props.each[0] === USER) {
      const item = children[0];
      if (typeof item === "function") item(() => USER);
    }
    if (typeof tag !== "function") return;
    if (tag === Pager) {
      pagers.push(Reflect.apply(tag, undefined, [props]));
    } else if (
      tag === PublicPager ||
      TABLES.has(tag) ||
      [
        "AccountReadScope",
        "AuthenticatedAccountData",
        "AccountPage",
        "BidSection",
        "ProxySection",
        "CompensationRoundRow",
      ].includes(tag.name)
    ) {
      Reflect.apply(tag, undefined, [props]);
    }
  });
  return {
    mounted,
    pagers,
    dispose() {
      mounted.dispose();
      LocaleSessionContext.defaultValue = previousLocale;
      fetcher.mockRestore();
      resetStoreForTest();
    },
  };
}

// Bun 的 JSX 替身由测试把成功响应交给页面的结果分支，分页组件及其回调沿用真实代码。
function showResult(mounted: MountedPage, tag: unknown, page: unknown, from = 0): void {
  const render = mounted.elements
    .slice(from)
    .find((element) => element.tag === tag && typeof element.children[0] === "function")
    ?.children[0];
  if (typeof render !== "function") throw new Error("未装配列表结果");
  mounted.run(() => render(() => page));
  flush();
}

function expectRejectedClamp(
  probe: ReturnType<typeof mountClampPage>,
  index: number,
  key: string,
): void {
  const { mounted } = probe;
  const controls = mounted.visible(probe.pagers[index]);
  const jump = controls.find(({ tag, props }) => tag === "input" && props.name === "page")?.props;
  expect(jump?.value).toBe("3");
  expect(jump?.max).toBe("1");
  expect(mounted.location.searchParams.get(key)).toBe("3");
  expect(
    controls.some(
      ({ tag, children }) => tag === "span" && children.includes(t("pagination-clamp-error")),
    ),
  ).toBe(true);
  const retry = controls.find(
    ({ tag, children }) => tag === "button" && children.includes(t("data-retry")),
  )?.props.onClick;
  if (typeof retry !== "function") throw new Error("未挂载纠偏重试");

  mounted.rejectWrites(false);
  retry();
  flush();
  expect(mounted.location.searchParams.get(key) ?? "1").toBe("1");
  expect(mounted.writes.at(-1)?.mode).toBe("replace");
}

test.each(LIST_PAGES)(
  "$name 总数只剩一页时回显已应用页码并可重试纠偏",
  ({ address, render, branch, read }) => {
    const probe = mountClampPage(address, render);
    try {
      probe.mounted.rejectWrites(true);
      showResult(probe.mounted, branch, read);
      expectRejectedClamp(probe, 0, "page");
    } finally {
      probe.dispose();
    }
  },
);

test.each(["bid_page", "proxy_page"] as const)(
  "账户 %s 单页时保留纠偏重试，另一表页码独立",
  (key) => {
    const previousSession = PublicSessionResourceContext.defaultValue;
    const previousOwner = PublicCommandOwnerContext.defaultValue;
    const bid = key === "bid_page" ? 3 : 7;
    const proxy = key === "proxy_page" ? 3 : 7;
    const probe = mountClampPage(`/me?bid_page=${bid}&proxy_page=${proxy}`, () => {
      installAccountSession();
      accountPage();
    });
    try {
      probe.mounted.rejectWrites(true);
      showResult(probe.mounted, Match, {
        bids: [],
        proxies: [],
        page_size: 10,
        bid_total: bid === 3 ? 1 : 100,
        proxy_total: proxy === 3 ? 1 : 100,
      });
      expect(
        probe.mounted.elements.filter(({ tag, props }) => tag === "div" && props.hidden === true),
      ).toHaveLength(0);
      expectRejectedClamp(probe, key === "bid_page" ? 0 : 1, key);
      expect(
        probe.mounted.location.searchParams.get(key === "bid_page" ? "proxy_page" : "bid_page"),
      ).toBe("7");
    } finally {
      probe.dispose();
      PublicSessionResourceContext.defaultValue = previousSession;
      PublicCommandOwnerContext.defaultValue = previousOwner;
    }
  },
);

test("补偿池明细纠偏写入失败可重试，保留所选场次", () => {
  const address = `/compensation-pool?auction=${AUCTION}&movement_page=3&movement_page_size=10`;
  const probe = mountClampPage(address, poolPage);
  try {
    const { mounted } = probe;
    mounted.rejectWrites(true);
    const poolStart = mounted.elements.length;
    showResult(mounted, Match, {
      ...SHORT_PAGE,
      page: 1,
      rounds: [COMPENSATION_ROUND],
      balance: 0,
      updated_at: COMPENSATION_ROUND.settled_at,
    });
    const detailsStart = mounted.elements.length;
    showResult(mounted, For, COMPENSATION_ROUND, poolStart);
    showResult(mounted, Match, { ...SHORT_PAGE, lots: [] }, detailsStart);
    expectRejectedClamp(probe, 1, "movement_page");
    expect(mounted.location.searchParams.get("auction")).toBe(AUCTION);
  } finally {
    probe.dispose();
  }
});

test.each([
  {
    name: "出价记录",
    pathname: "/bidlog",
    raw: `auction=${AUCTION}&lot=%4fne12&buyer=Alice&buyer=Bad%0aName&other=%2f`,
    originalPage: "bidlog_page=%30%32",
    restoredPage: "page=2",
    parameter: "page",
    pageField: "page",
    other: { bidLogLot: "One12" },
    render: BidLog,
    read: { page: 2, page_size: 10, total: 40, bids: [] },
  },
  {
    name: "账户竞价",
    pathname: "/me",
    raw: "proxy_%70age=0005&buyer=Alice&buyer=Bad%0aName&other=%2f",
    originalPage: "bid_page=%30%32",
    restoredPage: "bid_page=%30%32",
    parameter: "bid_page",
    pageField: "bidPage",
    other: { proxyPage: 5 },
    render: () => {
      installAccountSession();
      return accountPage();
    },
    read: {
      bids: [],
      proxies: [],
      bid_page: 2,
      proxy_page: 5,
      page_size: 10,
      bid_total: 40,
      proxy_total: 50,
    },
  },
] as const)("$name 通过分页控件前进后退时同步地址、快照和已应用查询", (scenario) => {
  const {
    name,
    pathname,
    raw,
    originalPage,
    restoredPage,
    parameter,
    pageField,
    other,
    render,
    read,
  } = scenario;
  const initial = `${pathname}?${raw}&${originalPage}`;
  const previousAddress = `${pathname}?${raw}&${restoredPage}`;
  const nextAddress = `${pathname}?${raw}&${parameter}=3`;
  const previousSession = PublicSessionResourceContext.defaultValue;
  const previousOwner = PublicCommandOwnerContext.defaultValue;
  const query: { state?: ReturnType<typeof usePublicQueryState>["state"] } = {};
  const probe = mountClampPage(initial, () => {
    query.state = usePublicQueryState().state;
    render();
  });
  try {
    const { mounted } = probe;
    showResult(mounted, Match, read);
    const next = mounted
      .visible(probe.pagers[0])
      .find(({ tag, props }) => tag === "button" && props["aria-label"] === t("catalog-next"))
      ?.props.onClick;
    if (typeof next !== "function") throw new Error(`${name}未装配下一页控件`);
    const expectApplied = (address: string, page: number) => {
      expect(mounted.location.href).toBe(`https://auction.test${address}`);
      expect(mounted.router.currentLocationString()).toBe(address);
      expect(query.state?.peek()).toMatchObject({ ...other, [pageField]: page });
    };

    expectApplied(previousAddress, 2);
    next();
    expect(mounted.router.currentLocationString()).toBe(nextAddress);
    flush();
    expectApplied(nextAddress, 3);
    mounted.back();
    expectApplied(previousAddress, 2);
    mounted.forward();
    expectApplied(nextAddress, 3);
  } finally {
    probe.dispose();
    PublicSessionResourceContext.defaultValue = previousSession;
    PublicCommandOwnerContext.defaultValue = previousOwner;
  }
});
