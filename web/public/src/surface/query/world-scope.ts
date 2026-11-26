// Public 世界范围的纯函数：路由类别、地址栏读写、导航继承与页面世界按钮组。
// Solid 版由外壳提供控制器、页面内按钮组切换世界；Rust 版仍保留各页面自己的世界选择器。

import { auctionParameter } from "./auction";
import type { PublicBoardQuery } from "./board";
import { publicAuctionHomeQuery } from "./links";
import type { PublicQueryWrite } from "./state";

/** 世界按钮组可表达的完整世界范围；只有跨世界页面才允许 `all`。 */
export type WorldScope = "all" | "main" | "isekai";
/** 路由按地址栏 world 参数的含义分为跨世界与单世界两类。 */
export type WorldScopeFamily = "multi" | "single";
/** 地址栏 world 参数归一化后的取值；其他输入都不能进入范围函数。 */
export type NormalizedWorldQuery = "main" | "isekai" | null;

/** 页面世界按钮组的一项；关闭的世界只能以当前地址指向的禁用项出现。 */
export interface WorldSwitchOption {
  readonly value: string;
  readonly disabled: boolean;
}

/** 计算世界按钮组选项与当前值的全部输入。 */
export interface WorldSwitchInput {
  readonly family: WorldScopeFamily;
  readonly world: NormalizedWorldQuery;
  /** 世界目录中开放的世界；目录尚未加载或读取失败时由回退列表替代。 */
  readonly enabledWorlds: readonly string[];
  readonly worldsLoaded: boolean;
  /** 单世界页当前地址是否为往期看板深链；这类地址不能因目录开关被改写。 */
  readonly historicalUrl: boolean;
}

/** 世界目录不可用时沿用原有回退，按两个冻结世界都开放处理。 */
const FALLBACK_ENABLED_WORLDS: readonly string[] = ["main", "isekai"];

/** 在读取地址栏状态的边界归一化一次；未知值与空值都表示该页面自己的缺省世界。 */
export function normalizedWorldQuery(world: string | null): NormalizedWorldQuery {
  return world === "main" || world === "isekai" ? world : null;
}

/** 路由名属于哪一类；`null`（未匹配）按 single。
 * multi 只有 AuctionList、History、BidLog、Fulfillment、ConsignmentCatalog。 */
export function worldScopeFamily(routeName: string | null): WorldScopeFamily {
  return routeName === "AuctionList" ||
    routeName === "History" ||
    routeName === "BidLog" ||
    routeName === "Fulfillment" ||
    routeName === "ConsignmentCatalog"
    ? "multi"
    : "single";
}

/** 地址里的世界真正决定页面内容、失效时需要纠偏的单世界路由。
 * 其余单世界路由只让顶栏在内存里取开放世界，不能把自动回退写成用户的显式选择。 */
export function routeUsesSingleWorld(routeName: string | null): boolean {
  return (
    routeName === "AuctionIndex" ||
    routeName === "AuctionPage" ||
    routeName === "LotDetail" ||
    routeName === "Forum"
  );
}

/**
 * 地址栏 `world` 在该类页面上的含义。
 * 入参已由 `normalizedWorldQuery` 收窄为 `"main" | "isekai" | null`。
 */
export function worldScopeFromQuery(
  family: WorldScopeFamily,
  world: NormalizedWorldQuery,
): WorldScope {
  if (family === "multi" && world === null) {
    return "all";
  }
  return world === "isekai" ? "isekai" : "main";
}

/** 读取页缺省为全部世界；地址世界非法时没有可读范围。 */
export function roundReadWorldScope(query: PublicBoardQuery): WorldScope | null {
  return query.invalidWorld === null
    ? worldScopeFromQuery("multi", normalizedWorldQuery(query.world))
    : null;
}

/** 把世界范围写成该类页面的地址栏取值；跨世界页的主世界必须显式写 `main`。
 * 单世界页默认省略主世界；导航继承显式选择时由 `explicitMain` 保留 `world=main`。 */
export function worldQueryForScope(
  family: WorldScopeFamily,
  scope: WorldScope,
  explicitMain = false,
): string | null {
  if (family === "multi") {
    return scope === "all" ? null : scope;
  }
  return scope === "isekai" ? "isekai" : explicitMain ? "main" : null;
}

/**
 * 需要单一世界的地方（顶栏当前场次、看板）使用的世界。
 * 全部世界优先 Main；Main 未开放时取目录顺序中的第一个开放世界。目录未加载时仍为 Main。
 */
export function scopedWorld(
  scope: WorldScope,
  enabledWorlds: readonly string[] = [],
  worldsLoaded = false,
): string {
  if (scope !== "all") {
    return scope;
  }
  if (!worldsLoaded) {
    return "main";
  }
  return enabledWorlds.includes("main") ? "main" : (enabledWorlds[0] ?? "main");
}

/** 世界按钮组的选项集合；已关闭世界只在当前地址表达的世界为它时作为禁用项出现。 */
export function worldSwitchOptions(input: WorldSwitchInput): readonly WorldSwitchOption[] {
  const enabled = effectiveEnabledWorlds(input);
  const available = enabled.map((value) => ({ value, disabled: false }));
  if (input.family === "multi") {
    const options =
      enabled.length >= 2 ? [{ value: "all", disabled: false }, ...available] : available;
    if (input.world !== null && !enabled.includes(input.world)) {
      options.push({ value: input.world, disabled: true });
    }
    return options;
  }
  const requested = worldScopeFromQuery("single", input.world);
  // 地址显式指向关闭的世界，或往期看板深链的缺省世界已关闭时，保留为禁用的当前项；
  // 自动纠偏写地址失败后仍能显示实际范围，并允许用户点开放世界重试。
  if (!enabled.includes(requested) && (input.world !== null || input.historicalUrl)) {
    return [...available, { value: requested, disabled: true }];
  }
  return available;
}

/** 按钮组当前值。跨世界页只有一个开放世界时，“全部”落到那个唯一的世界；
 * 单世界页只认地址表达的世界，不能把尚未写成功的纠偏目标提前当成当前项。 */
export function worldSwitchCurrent(input: WorldSwitchInput): string {
  const options = worldSwitchOptions(input);
  const enabled = effectiveEnabledWorlds(input);
  if (input.family === "multi") {
    const scope = worldScopeFromQuery("multi", input.world);
    if (scope !== "all") {
      return scope;
    }
    if (options.some((option) => option.value === "all")) {
      return "all";
    }
    return enabled.includes("main") ? "main" : (enabled[0] ?? "main");
  }
  return worldScopeFromQuery("single", input.world);
}

/** 历史成交价的显式世界属于本页筛选；其他路由只从地址取显式世界。 */
export function worldScopeNavigationWorld(
  query: { readonly world: string | null },
  routeName: string | null,
): NormalizedWorldQuery {
  return routeName === "History" ? null : normalizedWorldQuery(query.world);
}

/** 历史成交价的导航入口省略世界，避免把导航范围当成已应用的搜索条件。 */
export function worldScopeHref(path: string, world: NormalizedWorldQuery): string {
  if (world === null || path === "/history") {
    return path;
  }
  const separator = path.endsWith("?") ? "" : "?";
  return `${path}${separator}world=${world}`;
}

/**
 * 世界按钮切换范围后，只写回该操作拥有的字段。
 * 跨世界页面会清掉与旧世界绑定的场次选择；其余页面沿用原来的单世界切换规则。
 */
export function worldScopeSwitchQuery(
  routeName: string | null,
  scope: WorldScope,
  historicalBoard: boolean,
): Partial<PublicQueryWrite> {
  if (routeName === "AuctionList") {
    return {
      world: worldQueryForScope("multi", scope),
      auction: auctionParameter(null),
      category: null,
      page: 1,
    };
  }
  if (routeName === "History") {
    return {
      world: worldQueryForScope("multi", scope),
      page: 1,
    };
  }
  if (routeName === "BidLog" || routeName === "Fulfillment") {
    return {
      world: worldQueryForScope("multi", scope),
      auction: auctionParameter(null),
      bidLogLot: null,
      page: 1,
    };
  }
  // 主动选择要保留显式世界，使后续跨世界页能区分主世界和缺省范围。
  return worldSwitchQuery(scope === "all" ? null : scope, historicalBoard);
}

/** 单世界页面切换世界后的查询。看板正在展示往期场次时回到目标世界的首页（清除场次、筛选与页码，
 * 每页条数取本地偏好），与首页入口一致，避免所选世界与正文场次不一致；
 * 其余情况只改世界。 */
export function worldSwitchQuery(
  world: NormalizedWorldQuery,
  historicalBoard: boolean,
): Partial<PublicQueryWrite> {
  if (historicalBoard) {
    return publicAuctionHomeQuery(world);
  }
  return { world };
}

function effectiveEnabledWorlds(input: WorldSwitchInput): readonly string[] {
  return input.worldsLoaded ? input.enabledWorlds : FALLBACK_ENABLED_WORLDS;
}
