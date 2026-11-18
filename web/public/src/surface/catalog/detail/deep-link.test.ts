// 拍品直达链接的位置解析与看板处置判定。

import { expect, test } from "bun:test";
import type { CurrentAuctionItemResponse } from "@auction/contracts/dto/public/catalog/auction";
import { err, ok, type Result } from "@auction/web-shared/result";
import type { CatalogRow } from "../../../api/model";
import type { PublicBoardQuery } from "../../query";
import { queryAuctionId } from "../../query";
import type { LotViewTarget } from "./deep-link";

const { publicBoardQueryDefault } = await import("../../query");
const { catalogPage, catalogPosition, currentWorld, disposeLotView } = await import("./deep-link");

function target(): LotViewTarget {
  return {
    lotId: "lot-16",
    auctionId: "auction-7",
    world: "main",
    row: row("Clo16", "Cloth"),
    refreshGeneration: 0,
    rowReady: false,
  };
}

const DISPLAYED = { world: "main", id: "auction-7" };

function row(code: string, category: string): CatalogRow {
  return {
    lotId: "lot-16",
    code,
    name: "Phase Robe of Niflheim",
    gameEquipmentUrl: "",
    category,
    equipmentGroup: "",
    status: "live",
    sellerName: "seller",
    level: null,
    primaryAttributeInitials: null,
    avgRoll: null,
    startPrice: 1_000,
    currentPrice: 1_000,
    nextBid: 2_000,
    leaderName: null,
    remaining: "",
  };
}

function base(pageSize: number): PublicBoardQuery {
  return { ...publicBoardQueryDefault(), pageSize };
}

function current(
  auctionId: string,
  world: string,
): Result<CurrentAuctionItemResponse | null, string> {
  return ok({
    id: auctionId,
    world,
    number: 12,
    status: "live",
    opens_at: "2026-11-01T00:00:00Z",
    scheduled_ends_at: "2026-11-08T00:00:00Z",
    presentation_mode: "live",
    bidding_open: true,
    preview_revision: 1,
    proxy_default_mode: "fast",
  });
}

test("current_world_names_the_world_whose_current_auction_matches：当前场次命中时给出所在世界", () => {
  expect(
    currentWorld("auction-7", [current("auction-7", "main"), current("auction-9", "isekai")]),
  ).toBe("main");
  expect(
    currentWorld("auction-9", [current("auction-7", "main"), current("auction-9", "isekai")]),
  ).toBe("isekai");
});

test("unmatched_or_unreadable_currents_fall_back_to_the_historical_lookup：不命中或读取失败时交给往期查找", () => {
  // 往期场次：两个世界的当前场次都不是它。
  expect(
    currentWorld("auction-3", [current("auction-7", "main"), current("auction-9", "isekai")]),
  ).toBeNull();
  // 世界暂无当前场次。
  expect(currentWorld("auction-7", [ok(null), ok(null)])).toBeNull();
  // 所在世界读取失败时不下结论，交给往期查找；另一世界照常参与匹配。
  expect(currentWorld("auction-7", [err("network"), current("auction-9", "isekai")])).toBeNull();
  expect(currentWorld("auction-9", [err("network"), current("auction-9", "isekai")])).toBe(
    "isekai",
  );
});

test("page_follows_the_ordinal_inside_the_category：页码跟随分类内序号", () => {
  expect(catalogPage("Clo01", 10)).toBe(1);
  expect(catalogPage("Clo10", 10)).toBe(1);
  expect(catalogPage("Clo11", 10)).toBe(2);
  expect(catalogPage("Hea120", 50)).toBe(3);
  // 没有序号或序号非法时不猜页码。
  expect(catalogPage("Clo", 10)).toBeNull();
  expect(catalogPage("Clo00", 10)).toBeNull();
  expect(catalogPage("Clo16", 0)).toBeNull();
});

test("current_auction_lands_on_the_world_home_with_category_and_page：当前场次落到带分类与页码的世界首页", () => {
  let query = catalogPosition(row("Clo16", "Cloth"), "main", null, base(10));
  expect([query.world, queryAuctionId(query), query.category, query.page, query.pageSize]).toEqual([
    null,
    null,
    "clo",
    2,
    10,
  ]);
  // 异世界带世界参数；往期场次带场次参数。
  query = catalogPosition(
    row("Hea03", "Heavy"),
    "isekai",
    "67e55044-10b1-426f-9247-bb680e5fe0c8",
    base(20),
  );
  expect([query.world, queryAuctionId(query), query.category, query.page]).toEqual([
    "isekai",
    "67e55044-10b1-426f-9247-bb680e5fe0c8",
    "hea",
    1,
  ]);
});

test("unfilterable_category_keeps_the_first_unfiltered_page：不可筛选分类保留第一页且不带分类", () => {
  const query = catalogPosition(row("Lot07", "Accessory"), "main", null, base(10));
  expect([query.category, query.page]).toEqual([null, 1]);
});

test("request_waits_until_the_board_shows_the_target_world_and_page：看板未落到目标世界与页面时等待", () => {
  // 外壳的世界选择还没跟上地址栏：展示的是另一个世界的场次，不下结论。
  expect(disposeLotView(target(), "isekai", DISPLAYED, true, ["lot-1"])).toBe("wait");
  // 目录页尚未读出。
  expect(disposeLotView(target(), "main", DISPLAYED, true, [])).toBe("wait");
});

test("row_opens_the_drawer_when_it_is_on_the_page：目录当前页有这一行时交给行打开抽屉", () => {
  expect(disposeLotView(target(), "main", DISPLAYED, false, ["lot-15", "lot-16"])).toBe("row");
});

test("falls_back_to_the_read_only_drawer_when_the_row_is_not_there：这一页没有它时退回只读抽屉", () => {
  // 页码因撤拍空号偏后：这一页没有它。
  expect(disposeLotView(target(), "main", DISPLAYED, false, ["lot-21"])).toBe("read_only");
  // 场次已经轮换：看板展示的是同一世界的另一场，不必等目录读完。
  expect(disposeLotView(target(), "main", { ...DISPLAYED, id: "auction-8" }, true, [])).toBe(
    "read_only",
  );
  expect(disposeLotView(target(), "main", null, false, [])).toBe("read_only");
});
