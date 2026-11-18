// Public 世界范围的纯函数：路由类别、地址栏读写、导航继承与世界按钮组。

import { expect, test } from "bun:test";

import type {
  NormalizedWorldQuery,
  WorldScope,
  WorldScopeFamily,
  WorldSwitchInput,
} from "./world-scope";

const { publicBoardQueryDefault } = await import("./board");
const {
  routeUsesSingleWorld,
  scopedWorld,
  worldQueryForScope,
  worldScopeFamily,
  worldScopeFromQuery,
  worldScopeHref,
  worldScopeNavigationWorld,
  worldScopeSwitchQuery,
  worldSwitchCurrent,
  worldSwitchOptions,
} = await import("./world-scope");
const { publicAuctionHomeQuery } = await import("./links");

const MULTI_ROUTES = ["AuctionList", "History", "BidLog", "Fulfillment"] as const;
const SINGLE_ROUTES = [
  null,
  "AuctionIndex",
  "AuctionPage",
  "LotDetail",
  "Rules",
  "Settings",
] as const;

test("world_scope_family_matches_the_four_multi_world_routes：只有四个跨世界路由属于 multi", () => {
  for (const routeName of MULTI_ROUTES) {
    expect(worldScopeFamily(routeName)).toBe("multi");
  }
  for (const routeName of SINGLE_ROUTES) {
    expect(worldScopeFamily(routeName)).toBe("single");
  }
  expect(worldScopeFamily("UnknownRoute")).toBe("single");
});

test("world_scope_uses_single_world_only_for_content_routes：只有依赖世界内容的单世界页纠偏地址", () => {
  for (const routeName of ["AuctionIndex", "AuctionPage", "LotDetail", "Forum"]) {
    expect(routeUsesSingleWorld(routeName)).toBe(true);
  }
  for (const routeName of [
    null,
    "AuctionList",
    "History",
    "BidLog",
    "Fulfillment",
    "Rules",
    "About",
    "Blacklist",
    "CompensationPool",
    "Settings",
    "Login",
    "Me",
    "Account",
    "Status",
  ]) {
    expect(routeUsesSingleWorld(routeName)).toBe(false);
  }
});

test("world_scope_query_round_trips_follow_each_family_default：两类页面的世界读写遵循各自缺省", () => {
  const table: readonly [
    family: WorldScopeFamily,
    query: NormalizedWorldQuery,
    scope: WorldScope,
    written: string | null,
  ][] = [
    ["multi", null, "all", null],
    ["multi", "main", "main", "main"],
    ["multi", "isekai", "isekai", "isekai"],
    ["single", null, "main", null],
    ["single", "main", "main", null],
    ["single", "isekai", "isekai", "isekai"],
  ];
  for (const [family, query, scope, written] of table) {
    expect(worldScopeFromQuery(family, query)).toBe(scope);
    expect(worldQueryForScope(family, scope)).toBe(written);
  }
  expect(worldQueryForScope("single", "all")).toBeNull();
  expect(worldQueryForScope("single", "main")).toBeNull();
  expect(worldQueryForScope("single", "main", true)).toBe("main");
  expect(worldQueryForScope("multi", "main", true)).toBe("main");
  expect(scopedWorld("all")).toBe("main");
  expect(scopedWorld("all", ["main", "isekai"], true)).toBe("main");
  expect(scopedWorld("all", ["isekai"], true)).toBe("isekai");
  expect(scopedWorld("all", [], true)).toBe("main");
  expect(scopedWorld("all", ["isekai"], false)).toBe("main");
  expect(scopedWorld("main")).toBe("main");
  expect(scopedWorld("isekai")).toBe("isekai");
});

function switchInput(overrides: Partial<WorldSwitchInput> = {}): WorldSwitchInput {
  return {
    family: "multi",
    world: null,
    enabledWorlds: ["main", "isekai"],
    worldsLoaded: true,
    historicalUrl: false,
    ...overrides,
  };
}

test("world_switch_options_list_only_enabled_worlds：按钮组只列配置开放的世界", () => {
  const both = switchInput();
  expect(worldSwitchOptions(both)).toEqual([
    { value: "all", disabled: false },
    { value: "main", disabled: false },
    { value: "isekai", disabled: false },
  ]);
  expect(worldSwitchCurrent(both)).toBe("all");

  const onlyMain = switchInput({ enabledWorlds: ["main"] });
  expect(worldSwitchOptions(onlyMain)).toEqual([{ value: "main", disabled: false }]);
  expect(worldSwitchCurrent(onlyMain)).toBe("main");

  const onlyIsekai = switchInput({ enabledWorlds: ["isekai"] });
  expect(worldSwitchOptions(onlyIsekai)).toEqual([{ value: "isekai", disabled: false }]);
  // “全部”地址仍按不带 world 读取；当前值只把唯一开放世界标成选中，不写回地址。
  expect(worldSwitchCurrent(onlyIsekai)).toBe("isekai");

  const none = switchInput({ enabledWorlds: [] });
  expect(worldSwitchOptions(none)).toEqual([]);
  expect(worldSwitchCurrent(none)).toBe("main");
});

test("world_switch_options_expose_an_explicitly_requested_closed_world：跨世界页保留关闭世界深链", () => {
  const closedIsekai = switchInput({ world: "isekai", enabledWorlds: ["main"] });
  expect(worldSwitchOptions(closedIsekai)).toEqual([
    { value: "main", disabled: false },
    { value: "isekai", disabled: true },
  ]);
  expect(worldSwitchCurrent(closedIsekai)).toBe("isekai");

  const closedMain = switchInput({ world: "main", enabledWorlds: ["isekai"] });
  expect(worldSwitchOptions(closedMain)).toEqual([
    { value: "isekai", disabled: false },
    { value: "main", disabled: true },
  ]);
  expect(worldSwitchCurrent(closedMain)).toBe("main");
});

test("world_switch_options_keep_a_closed_single_world_current：单世界页保留地址指向的关闭世界供重试", () => {
  const historicalClosed = switchInput({
    family: "single",
    enabledWorlds: ["isekai"],
    historicalUrl: true,
  });
  expect(worldSwitchOptions(historicalClosed)).toEqual([
    { value: "isekai", disabled: false },
    { value: "main", disabled: true },
  ]);
  expect(worldSwitchCurrent(historicalClosed)).toBe("main");

  const closedIsekai = switchInput({
    family: "single",
    world: "isekai",
    enabledWorlds: ["main"],
    historicalUrl: true,
  });
  expect(worldSwitchOptions(closedIsekai)).toEqual([
    { value: "main", disabled: false },
    { value: "isekai", disabled: true },
  ]);
  expect(worldSwitchCurrent(closedIsekai)).toBe("isekai");

  const explicitClosed = switchInput({
    family: "single",
    world: "isekai",
    enabledWorlds: ["main"],
    historicalUrl: false,
  });
  expect(worldSwitchOptions(explicitClosed)).toEqual([
    { value: "main", disabled: false },
    { value: "isekai", disabled: true },
  ]);
  // 当前项取地址里的关闭世界，而不是自动纠偏想要写成的开放世界。
  expect(worldSwitchCurrent(explicitClosed)).toBe("isekai");

  const ordinaryClosed = switchInput({
    family: "single",
    enabledWorlds: ["isekai"],
    historicalUrl: false,
  });
  expect(worldSwitchOptions(ordinaryClosed)).toEqual([{ value: "isekai", disabled: false }]);
  // 无 world 的单世界地址按主世界解释；选项隐藏时仍不把自动回退当成地址事实。
  expect(worldSwitchCurrent(ordinaryClosed)).toBe("main");

  const noWorldsHistory = switchInput({
    family: "single",
    world: "isekai",
    enabledWorlds: [],
    historicalUrl: true,
  });
  expect(worldSwitchOptions(noWorldsHistory)).toEqual([{ value: "isekai", disabled: true }]);
  expect(worldSwitchCurrent(noWorldsHistory)).toBe("isekai");
});

test("world_switch_options_fall_back_before_the_world_catalog_loads：目录未返回或失败时两个世界都开放", () => {
  const pending = switchInput({ enabledWorlds: [], worldsLoaded: false });
  expect(worldSwitchOptions(pending)).toEqual([
    { value: "all", disabled: false },
    { value: "main", disabled: false },
    { value: "isekai", disabled: false },
  ]);
  expect(worldSwitchCurrent(pending)).toBe("all");

  const pendingSingle = switchInput({
    family: "single",
    world: "isekai",
    enabledWorlds: [],
    worldsLoaded: false,
  });
  expect(worldSwitchOptions(pendingSingle)).toEqual([
    { value: "main", disabled: false },
    { value: "isekai", disabled: false },
  ]);
  expect(worldSwitchCurrent(pendingSingle)).toBe("isekai");
});

test("导航范围只取地址里的有效显式世界", () => {
  expect(worldScopeNavigationWorld({ world: null }, "BidLog")).toBeNull();
  expect(worldScopeNavigationWorld({ world: "all" }, "Fulfillment")).toBeNull();
  expect(worldScopeNavigationWorld({ world: "main" }, "Me")).toBe("main");
  expect(worldScopeNavigationWorld({ world: "isekai" }, "AuctionIndex")).toBe("isekai");
  expect(worldScopeHref("/", "isekai")).toBe("/?world=isekai");
  expect(worldScopeHref("/", worldScopeNavigationWorld({ world: "main" }, "BidLog"))).toBe(
    "/?world=main",
  );
  expect(worldScopeHref("/", worldScopeNavigationWorld({ world: null }, "BidLog"))).toBe("/");
});

test("进入历史成交价时不预置世界筛选", () => {
  for (const world of [null, "main", "isekai"] as const) {
    expect(worldScopeHref("/history", world)).toBe("/history");
  }
});

test("历史成交价的已应用世界不作为导航范围", () => {
  for (const world of [null, "main", "isekai"] as const) {
    expect(worldScopeNavigationWorld({ world }, "History")).toBeNull();
  }
});

test("public_auction_home_query_keeps_the_explicit_navigation_world：首页查询保留显式主世界", () => {
  expect(publicAuctionHomeQuery(null).world).toBeNull();
  expect(publicAuctionHomeQuery("main").world).toBe("main");
  expect(publicAuctionHomeQuery("isekai").world).toBe("isekai");
});

test("world_scope_switch_clears_auction_list_dependencies：往期拍卖切换世界清空场次、类型与页码", () => {
  const scopes: readonly [WorldScope, string | null][] = [
    ["all", null],
    ["main", "main"],
    ["isekai", "isekai"],
  ];
  for (const [scope, world] of scopes) {
    const next = worldScopeSwitchQuery("AuctionList", scope, false);
    expect(next).toEqual({ world, auctionId: null, category: null, page: 1 });
  }
});

test("world_scope_switch_keeps_history_filters_but_resets_page：价格历史切换世界保留筛选并回到首页", () => {
  const scopes: readonly [WorldScope, string | null][] = [
    ["all", null],
    ["main", "main"],
    ["isekai", "isekai"],
  ];
  for (const [scope, world] of scopes) {
    const next = worldScopeSwitchQuery("History", scope, false);
    expect(next).toEqual({ world, page: 1 });
  }
});

test("world_scope_switch_clears_round_and_lot_filters_for_bidlog_and_fulfillment：出价记录与履约进度切换世界清空场次和 lot", () => {
  const scopes: readonly [WorldScope, NormalizedWorldQuery][] = [
    ["all", null],
    ["main", "main"],
    ["isekai", "isekai"],
  ];
  for (const routeName of ["BidLog", "Fulfillment"] as const) {
    for (const [scope, world] of scopes) {
      const next = worldScopeSwitchQuery(routeName, scope, false);
      expect(next).toEqual({
        world,
        auctionId: null,
        bidLogLot: null,
        page: 1,
      });
    }
  }
});

test("world_scope_switch_on_single_world_pages_only_changes_the_world：单世界页面切换世界只改世界", () => {
  const cases: readonly [WorldScope, string | null][] = [
    ["all", null],
    ["main", "main"],
    ["isekai", "isekai"],
  ];
  for (const [scope, world] of cases) {
    const next = worldScopeSwitchQuery("Rules", scope, false);
    expect(next).toEqual({ world });
  }
});

test("world_scope_switch_on_a_historical_board_returns_to_the_world_home：历史看板切换世界回到目标世界首页", () => {
  const isekai = worldScopeSwitchQuery("AuctionIndex", "isekai", true);
  expect(isekai).toEqual({
    ...publicBoardQueryDefault(),
    world: "isekai",
  });
  expect(worldScopeSwitchQuery("AuctionIndex", "main", true)).toEqual({
    ...publicBoardQueryDefault(),
    world: "main",
  });
});
