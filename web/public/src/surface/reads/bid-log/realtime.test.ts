// 出价记录与看板经过真实外壳和 WebSocket 入口，观察普通竞价触发的 GET。

import { expect, jest, onTestFinished, spyOn, test } from "bun:test";
import { flush } from "solid-js";
import { SocketStub } from "../../../api/realtime/subscription-fixture";
import {
  AUCTION_ID,
  dispatch,
  END_AT,
  LOT_ID,
  lotDetailReceipt,
  mountCompetition,
  settle,
  waitFor,
} from "../../commands/competition-fixture.test";
import { installDialogFocus } from "../../shell/dialog-focus-fixture.test";
import { DomElement } from "../../shell/navigation-fixture.test";
import { reject, success } from "../../shell/session-fixture";

const OTHER_WORLD = "0190f2b4-1c2d-4e5f-8a6b-000000000001";
const SETTLED = "0190f2b4-1c2d-4e5f-8a6b-000000000002";
const LONG_NAME = "Legendary Cotton Robe of the Elementalist";

test.each([
  ["empty", "idle"],
  ["invalid", "unavailable"],
  ["failed", "degraded"],
] as const)("出价记录目录进入 %s 终态后，顶栏显示 %s", async (state, expected) => {
  const page = await mountCompetition("main", true);
  const result = Promise.withResolvers<Response>();
  const badge = () =>
    page.find((element) => element.getAttribute("data-state") !== null)?.getAttribute("data-state");
  try {
    page.server.readRounds = async () => (await result.promise).clone();
    page.go(state === "invalid" ? "/bidlog?auction=bad" : "/bidlog");
    await waitFor(() => page.reads.some((read) => read.startsWith("/api/v1/auctions?")));
    await settle();
    expect(badge()).toBe(state === "invalid" ? expected : "connecting");
    result.resolve(
      state === "failed"
        ? reject("internal_error", 500)
        : success({ auctions: [], page: 1, page_size: 200, total: 0 }),
    );
    await settle();
    expect(badge()).toBe(expected);
    expect(page.reads.some((read) => read.startsWith("/api/v1/events?"))).toBe(false);
  } finally {
    result.resolve(reject("internal_error", 500));
    page.dispose();
  }
});

test("换场后的日志合并新场通知，旧场抽屉仍按自己的窗口补读", async () => {
  const sockets = installSockets();
  const focus = installDialogFocus();
  const page = await mountCompetition();
  const hasClass = (css: string) => (element: DomElement) => element.getAttribute("class") === css;
  const bidLog = (auction: string, pageSize: number, lot: string | null) =>
    page.reads.filter((path) => {
      const url = new URL(path, "https://auction.test");
      return (
        url.pathname === `/api/v1/auctions/${auction}/bid-log` &&
        url.searchParams.get("lot") === lot &&
        url.searchParams.get("page_size") === String(pageSize)
      );
    }).length;
  const reads = () => [
    page.reads.filter((path) => path === `/api/v1/lots/${LOT_ID}`).length,
    bidLog(AUCTION_ID, 4, LOT_ID),
    bidLog(AUCTION_ID, 10, LOT_ID),
  ];
  const flushRequests = async () => {
    for (let turn = 0; turn < 128; turn += 1) {
      await Promise.resolve();
      flush();
    }
  };
  try {
    focus.attach(page.find);
    page.server.readEvents = replay;
    page.server.readRounds = (url) => {
      const auctions =
        url.searchParams.get("status") === "live"
          ? [round(AUCTION_ID, "main", "live", 12), round(OTHER_WORLD, "isekai", "live", 11)]
          : [];
      return success({ auctions, total: auctions.length, page: 1, page_size: 200 });
    };
    page.server.readBidLog = (url) => {
      const pageSize = Number(url.searchParams.get("page_size"));
      const facts = url.pathname.includes(AUCTION_ID)
        ? [5, 4, 3, 2, 1].slice(0, pageSize).map((number) => bidFact(number, LONG_NAME, AUCTION_ID))
        : [];
      return success({
        bids: facts,
        total: facts.length === 0 ? 0 : 5,
        page: 1,
        page_size: pageSize,
      });
    };
    page.go(`/bidlog?auction=${AUCTION_ID}`);
    await waitFor(() => page.find(hasClass("public-lot-view")) !== null);
    dispatch(page.need(hasClass("public-lot-view")), "Click");
    await waitFor(() => page.find(hasClass("public-catalog-detail__more")) !== null);
    dispatch(page.need(hasClass("public-catalog-detail__more")), "Click");
    await waitFor(() => reads()[2] === 1);
    await settle();
    expect(reads()).toEqual([1, 1, 1]);

    jest.useFakeTimers();
    const random = spyOn(Math, "random").mockReturnValue(0);
    onTestFinished(() => {
      jest.useRealTimers();
      random.mockRestore();
    });
    sockets.need(AUCTION_ID).bid(1);
    await flushRequests();
    jest.advanceTimersByTime(90);
    page.go(`/bidlog?auction=${OTHER_WORLD}`);
    await flushRequests();
    expect(page.find(hasClass("public-catalog-history"))).not.toBeNull();
    expect(bidLog(OTHER_WORLD, 20, null)).toBe(1);
    const nextSocket = sockets.need(OTHER_WORLD);
    expect(nextSocket.readyState).toBe(SocketStub.OPEN);
    nextSocket.bid(1);
    await flushRequests();
    expect(reads()).toEqual([1, 1, 1]);

    // A 场在 100 ms 到期，B 场自己的 100 ms 窗口从 90 ms 开始。
    jest.advanceTimersByTime(10);
    await flushRequests();
    expect(reads()).toEqual([2, 2, 2]);
    expect(bidLog(AUCTION_ID, 20, null)).toBe(1);
    expect(bidLog(OTHER_WORLD, 20, null)).toBe(1);

    jest.advanceTimersByTime(10);
    nextSocket.bid(2);
    await flushRequests();
    jest.advanceTimersByTime(79);
    await flushRequests();
    expect(bidLog(OTHER_WORLD, 20, null)).toBe(1);
    jest.advanceTimersByTime(1);
    await flushRequests();
    expect(bidLog(OTHER_WORLD, 20, null)).toBe(2);
    jest.advanceTimersByTime(250);
    await flushRequests();
    expect(bidLog(OTHER_WORLD, 20, null)).toBe(2);
    expect(reads()).toEqual([2, 2, 2]);
  } finally {
    page.dispose();
    focus.restore();
    sockets.restore();
  }
});

class AuctionSocket extends SocketStub {
  readonly auctionId: string | null;

  constructor(url: string) {
    super();
    this.auctionId = new URL(url).searchParams.get("auction_id");
  }

  bid(sequence: number) {
    this.dispatchEvent(
      new MessageEvent("message", {
        data: JSON.stringify({
          schema_version: 2,
          event_id: `00000000-0000-4000-8000-${String(sequence).padStart(12, "0")}`,
          kind: "lot.bid_accepted",
          stream_sequence: sequence,
          created_at: "2026-11-17T00:00:00Z",
          payload: {
            auction_id: this.auctionId,
            lot_id: LOT_ID,
            bid_id: `11111111-0000-4000-8000-${String(sequence).padStart(12, "0")}`,
            amount: 1_000 + sequence,
            effective_end: END_AT,
            state_version: sequence,
            fact_index: 1,
            mechanism: "ordinary",
          },
        }),
      }),
    );
  }
}

function installSockets() {
  const original = Object.getOwnPropertyDescriptor(globalThis, "WebSocket");
  const sockets: AuctionSocket[] = [];
  class TrackedSocket extends AuctionSocket {
    constructor(url: string) {
      super(url);
      sockets.push(this);
    }
  }
  Object.defineProperty(globalThis, "WebSocket", { configurable: true, value: TrackedSocket });
  return {
    need(id: string) {
      const socket = sockets.findLast((current) => current.auctionId === id);
      if (socket === undefined) throw new Error(`未订阅测试场次 ${id}`);
      return socket;
    },
    restore() {
      if (original === undefined) Reflect.deleteProperty(globalThis, "WebSocket");
      else Object.defineProperty(globalThis, "WebSocket", original);
    },
  };
}

function round(id: string, world: string, status: string, sequence: number) {
  return {
    id,
    world_id: world,
    status,
    sequence,
    scheduled_open_at: "2026-11-17T00:00:00Z",
    effective_end_at: END_AT,
    lot_count: 1,
    sold_count: 0,
    unsold_count: 0,
    sold_amount: 0,
    state_version: 1,
  };
}

function replay(url: URL): Response {
  return success({
    auction_id: url.searchParams.get("auction_id"),
    events: [],
    next_after: Number(url.searchParams.get("after")),
    has_more: false,
    refetch_required: false,
  });
}

class PortalTooltip extends DomElement {
  parentNode: {
    removeChild(child: PortalTooltip): void;
    isSameNode(node: unknown): boolean;
  } | null = null;
  style = { setProperty() {} };
  textContent = "";

  setAttribute(name: string, value: string) {
    this.attributes[name] = value;
  }
  addEventListener() {}
  removeEventListener() {}
}

function installTooltipDom() {
  const focus = installDialogFocus();
  const visible = new Set<PortalTooltip>();
  const getElementById = document.getElementById.bind(document);
  const body = {
    appendChild(tooltip: PortalTooltip) {
      tooltip.parentNode = body;
      visible.add(tooltip);
    },
    removeChild(tooltip: PortalTooltip) {
      tooltip.parentNode = null;
      visible.delete(tooltip);
    },
    isSameNode(node: unknown) {
      return node === document.body;
    },
  };
  Object.assign(document.body, body);
  Object.assign(document, {
    createElement: (tag: string) => new PortalTooltip(tag, {}),
    getElementById: (id: string) =>
      [...visible].find((tooltip) => tooltip.getAttribute("id") === id) ?? getElementById(id),
  });
  return { ...focus, texts: () => [...visible].map((tooltip) => tooltip.textContent) };
}

function bidFact(number: number, name: string, auctionId = OTHER_WORLD) {
  return {
    id: `11111111-0000-4000-8000-${String(number).padStart(12, "0")}`,
    auction_id: auctionId,
    lot_id: LOT_ID,
    lot_number: 1,
    amount: 1000 + number,
    equipment_name: name,
    equipment_category: "Cloth",
    bidder_name: "Player",
    valid: true,
    accepted_state_version: number,
    fact_index: 1,
    mechanism: "ordinary",
    created_at: "2026-11-17T00:00:00.123456Z",
  };
}

test("全部世界的异世界场次接收自己的出价，旧主世界流与已结算记录互不刷新", async () => {
  const sockets = installSockets();
  const page = await mountCompetition();
  const logReads = (id: string) =>
    page.reads.filter((path) => path.startsWith(`/api/v1/auctions/${id}/bid-log?`));
  const hasReplay = (id: string) =>
    page.reads.some((path) => path.startsWith(`/api/v1/events?auction_id=${id}&`));
  try {
    page.server.readEvents = replay;
    page.server.readRounds = (url) => {
      const status = url.searchParams.get("status");
      const auctions =
        status === "live"
          ? [round(AUCTION_ID, "main", "live", 12), round(OTHER_WORLD, "isekai", "live", 11)]
          : status === "settled"
            ? [round(SETTLED, "isekai", "settled", 10)]
            : [];
      return success({ auctions, total: auctions.length, page: 1, page_size: 200 });
    };
    page.server.readBidLog = (url) =>
      success({
        bids: [],
        total: 0,
        page: 1,
        page_size: Number(url.searchParams.get("page_size")),
      });
    await waitFor(() => hasReplay(AUCTION_ID));
    const mainSocket = sockets.need(AUCTION_ID);
    page.go(`/bidlog?auction=${OTHER_WORLD}`);
    await waitFor(() => logReads(OTHER_WORLD).length === 1);
    await settle();
    expect(hasReplay(OTHER_WORLD)).toBe(true);
    expect(mainSocket.readyState).toBe(SocketStub.CLOSING);
    expect(logReads(OTHER_WORLD)[0]).toContain("world=isekai");
    expect(page.location.searchParams.has("world")).toBe(false);
    const otherSocket = sockets.need(OTHER_WORLD);
    mainSocket.bid(1);
    await Bun.sleep(275);
    expect(logReads(OTHER_WORLD)).toHaveLength(1);
    otherSocket.bid(1);
    await waitFor(() => logReads(OTHER_WORLD).length === 2);

    page.go(`/bidlog?auction=${SETTLED}`);
    await waitFor(() => logReads(SETTLED).length === 1);
    await settle();
    otherSocket.bid(2);
    const defaultSocket = sockets.need(AUCTION_ID);
    expect(defaultSocket).not.toBe(mainSocket);
    expect(defaultSocket.readyState).toBe(SocketStub.OPEN);
    defaultSocket.bid(1);
    await Bun.sleep(275);
    expect(logReads(SETTLED)).toHaveLength(1);
    expect(logReads(OTHER_WORLD)).toHaveLength(2);
  } finally {
    page.dispose();
    sockets.restore();
  }
});

test.each([
  ["settled", SETTLED, 1],
  ["live", AUCTION_ID, 2],
] as const)("只读抽屉在 %s 场次按归属补读，并响应权威刷新", async (status, auctionId, expected) => {
  const sockets = installSockets();
  const focus = installDialogFocus();
  const page = await mountCompetition();
  const drawerReads = () => {
    const detail = page.reads.filter((path) => path === `/api/v1/lots/${LOT_ID}`).length;
    const history = page.reads
      .filter((path) => path.startsWith(`/api/v1/auctions/${auctionId}/bid-log?lot=${LOT_ID}&`))
      .map((path) => new URL(path, "https://auction.test").searchParams.get("page_size"));
    return [
      detail,
      history.filter((size) => size === "4").length,
      history.filter((size) => size === "10").length,
    ];
  };
  const hasClass = (css: string) => (element: DomElement) => element.getAttribute("class") === css;
  try {
    focus.attach(page.find);
    page.server.readEvents = replay;
    page.server.readRounds = (url) => {
      const rounds =
        url.searchParams.get("status") === status ? [round(auctionId, "main", status, 12)] : [];
      return success({ auctions: rounds, total: rounds.length, page: 1, page_size: 200 });
    };
    if (status === "settled") {
      page.server.readLot = (id) => lotDetailReceipt(id, 1500, "Leader", auctionId);
    }
    page.server.readBidLog = (url) => {
      const pageSize = Number(url.searchParams.get("page_size"));
      const facts = [5, 4, 3, 2, 1]
        .slice(0, pageSize)
        .map((index) => bidFact(index, LONG_NAME, auctionId));
      return success({ bids: facts, total: 5, page: 1, page_size: pageSize });
    };
    page.go(`/bidlog?auction=${auctionId}`);
    await waitFor(() => page.find(hasClass("public-lot-view")) !== null);
    dispatch(page.need(hasClass("public-lot-view")), "Click");
    await waitFor(() => page.find(hasClass("public-catalog-detail__more")) !== null);
    dispatch(page.need(hasClass("public-catalog-detail__more")), "Click");
    await waitFor(() => drawerReads()[2] === 1);
    await settle();
    expect(drawerReads()).toEqual([1, 1, 1]);
    const live = sockets.need(AUCTION_ID);
    expect(live.readyState).toBe(SocketStub.OPEN);
    live.bid(1);
    if (status === "settled") {
      await Bun.sleep(275);
    } else {
      await waitFor(() => drawerReads().every((count) => count === expected));
    }
    expect(drawerReads()).toEqual([expected, expected, expected]);

    // 模拟浏览器拒绝整页 reload 后的权威写代号路径，仍应补读同一个拍品及其历史。
    dispatch(page.need(hasClass("public-connection-refresh")), "Click");
    await waitFor(() => drawerReads().every((count) => count === expected + 1));
    expect(drawerReads()).toEqual([expected + 1, expected + 1, expected + 1]);
  } finally {
    page.dispose();
    focus.restore();
    sockets.restore();
  }
});

test("实时插入前置记录时旧行保留身份，固定提示收起且可在原记录重新打开", async () => {
  const sockets = installSockets();
  const tooltip = installTooltipDom();
  const page = await mountCompetition();
  try {
    tooltip.attach(page.find);
    page.server.readEvents = replay;
    page.server.readRounds = (url) => {
      const auctions =
        url.searchParams.get("status") === "live" ? [round(OTHER_WORLD, "isekai", "live", 11)] : [];
      return success({ auctions, total: auctions.length, page: 1, page_size: 200 });
    };
    let facts = [bidFact(1, LONG_NAME)];
    page.server.readBidLog = (url) =>
      success({
        bids: facts,
        total: facts.length,
        page: 1,
        page_size: Number(url.searchParams.get("page_size")),
      });
    page.go(`/bidlog?auction=${OTHER_WORLD}`);
    const original = (element: DomElement) => element.getAttribute("data-full-value") === LONG_NAME;
    const inserted = (element: DomElement) =>
      element.getAttribute("data-full-value") === "New Staff";
    await waitFor(() => page.find(original) !== null);
    const anchor = page.need(original);
    anchor.scrollWidth = 160;
    anchor.clientWidth = 80;
    tooltip.captureClick(anchor);
    dispatch(anchor, "MouseEnter");
    dispatch(anchor, "Click");
    expect(anchor.getAttribute("aria-expanded")).toBe("true");
    expect(tooltip.texts()).toEqual([LONG_NAME]);

    const other = sockets.need(OTHER_WORLD);
    other.bid(1);
    const readCount = () =>
      page.reads.filter((path) => path.startsWith(`/api/v1/auctions/${OTHER_WORLD}/bid-log?`))
        .length;
    await waitFor(() => readCount() === 2);
    await settle();
    expect(tooltip.texts()).toEqual([LONG_NAME]);

    facts = [bidFact(2, "New Staff"), ...facts];
    other.bid(2);
    await waitFor(() => page.find(inserted) !== null);
    expect(readCount()).toBe(3);
    expect(page.find(original)).toBe(anchor);
    expect(anchor.getAttribute("aria-expanded")).toBe("false");
    expect(tooltip.texts()).toEqual([]);
    dispatch(anchor, "Click");
    expect(tooltip.texts()).toEqual([LONG_NAME]);
  } finally {
    page.dispose();
    tooltip.restore();
    sockets.restore();
  }
});
