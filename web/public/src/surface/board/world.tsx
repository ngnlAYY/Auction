// Public 世界切换控制器、页面按钮组与世界标签辅助。控制器随外壳常驻，页面按钮组只消费它；
// Solid 版的世界切换位于各页面，Rust 版仍保留各页面自己的世界选择器。

import type { WorldsResponse } from "@auction/contracts/dto/public/account";
import type { Cell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import type { RouteMatch } from "@auction/web-shared/router";
import { useRouter } from "@auction/web-shared/router";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { createEffect, For, Show, untrack, useContext } from "solid-js";

import { fetchWorlds } from "../../api/queries/current";
import {
  normalizedWorldQuery,
  type PublicBoardQuery,
  readPublicQuery,
  routeUsesSingleWorld,
  scopedWorld,
  updatePublicQueryState,
  type WorldScope,
  type WorldSwitchInput,
  type WorldSwitchOption,
  worldScopeFamily,
  worldScopeFromQuery,
  worldScopeSwitchQuery,
  worldSwitchCurrent,
  worldSwitchOptions,
} from "../query";
import { WorldSwitchContext } from "../shell/contexts";

export type { WorldSwitchOption };

/** 页面按钮组可读的世界切换接口；写地址栏的动作只经 `choose`。 */
export interface WorldSwitch {
  readonly options: () => readonly WorldSwitchOption[];
  readonly current: () => string;
  readonly choose: (value: string) => void;
  /** 目录已加载且至少有两个开放世界；目录未加载时沿用旧版可切换的回退。 */
  readonly selectable: () => boolean;
}

/** 控制器依赖外壳已有的选中世界与查询状态；两者都由外壳创建并常驻。 */
export interface CreateWorldSwitchOptions {
  readonly selectedWorld: Cell<string>;
  readonly shellQueryState: Cell<PublicBoardQuery>;
}

interface WorldCatalog {
  readonly enabled: readonly string[];
  readonly loaded: boolean;
}

/**
 * 创建随外壳常驻的世界切换控制器。
 * 目录同步、单世界页失效纠偏与地址写回都不依赖页面是否渲染按钮组；关闭世界的历史仍由
 * 跨世界页面按不带 `world` 的全部范围读取。
 */
export function createWorldSwitch(props: CreateWorldSwitchOptions): WorldSwitch {
  const router = useRouter();
  const worlds = createTaskResource(
    () => null,
    async (_dependencies, context) => fetchWorlds(context.signal),
  );
  const catalog = createValueMemo((): WorldCatalog => {
    const snapshot = worlds();
    if (snapshot?.ok === true) {
      return { enabled: headerWorlds(snapshot.value), loaded: true };
    }
    // 目录未返回或读取失败时沿用旧版回退：两个冻结世界都开放。
    return { enabled: ["main", "isekai"], loaded: false };
  });
  const routeName = createValueMemo(() => routeNameOf(router.current()));
  const family = createValueMemo(() => worldScopeFamily(routeName()));
  const queryWorld = createValueMemo(() => normalizedWorldQuery(props.shellQueryState().world));
  const historicalUrl = createValueMemo(
    () => isBoardRouteName(routeName()) && props.shellQueryState().auctionId !== null,
  );
  const switchInput = createValueMemo(
    (): WorldSwitchInput => ({
      family: family(),
      world: queryWorld(),
      enabledWorlds: catalog().enabled,
      worldsLoaded: catalog().loaded,
      historicalUrl: historicalUrl(),
    }),
  );
  const options = createValueMemo(() => worldSwitchOptions(switchInput()));
  // 当前项只反映地址实际表达的范围；自动纠偏未成功写回时不能提前显示目标世界。
  const current = createValueMemo(() => worldSwitchCurrent(switchInput()));
  const selectable = createValueMemo(() => !catalog().loaded || catalog().enabled.length >= 2);
  const setSelectedWorld = (world: string): void => {
    // Cell 的同值写入也会通知；有效单一世界未变时不得重启顶栏当前场次读取。
    if (props.selectedWorld.peek() !== world) {
      props.selectedWorld.set(world);
    }
  };
  createEffect(
    () => ({
      catalog: catalog(),
      route: routeName(),
      // 外壳查询可能与路由同时变化；即使路由未变，也按最新地址重新判定。
      query: props.shellQueryState(),
    }),
    (state) => {
      // 纠偏必须以同一份最新地址栏与路由为准：外壳状态可能仍是上一页的值。
      const latestRoute = untrack(() => router.current());
      const latestRouteName = routeNameOf(latestRoute);
      const latestQuery = readPublicQuery();
      const latestFamily = worldScopeFamily(latestRouteName);
      const latestWorld = normalizedWorldQuery(latestQuery.world);
      const requestedScope = worldScopeFromQuery(latestFamily, latestWorld);
      const requestedWorld = scopedWorld(
        requestedScope,
        state.catalog.enabled,
        state.catalog.loaded,
      );
      if (!state.catalog.loaded) {
        // 目录返回前仍按地址同步单一世界，但不据此判断关闭世界或改写地址。
        setSelectedWorld(requestedWorld);
        return;
      }
      // 跨世界页与看板的往期深链保留目标世界；论坛即使带 auction 也不是往期看板。
      const latestHistorical =
        latestFamily === "multi" ||
        (isBoardRouteName(latestRouteName) && latestQuery.auctionId !== null);
      const latestActive =
        latestHistorical || state.catalog.enabled.length === 0
          ? requestedWorld
          : activeSingleWorld(state.catalog.enabled, requestedWorld);
      const shouldRewrite =
        routeUsesSingleWorld(latestRouteName) &&
        !latestHistorical &&
        state.catalog.enabled.length > 0 &&
        latestActive !== requestedWorld;
      if (shouldRewrite) {
        // 仅实际使用单一世界的路由纠偏；写入失败时保持原地址和选中世界，等待下一次重试。
        const written = updatePublicQueryState(
          props.shellQueryState,
          (query) => {
            query.world = latestActive !== "main" ? latestActive : null;
          },
          "replace",
        );
        if (!written) {
          return;
        }
      }
      setSelectedWorld(latestActive);
    },
  );
  const choose = (value: string): void => {
    // 唯一开放世界也可能在“全部”地址下显示为当前项；重复点击不能把隐式缺省写成显式参数。
    // 当前项同样以地址为准，纠偏失败后点击开放世界不会被误判成重复选择。
    if (value === current()) {
      return;
    }
    const scope = pickerWorldScope(value);
    if (scope === null) {
      return;
    }
    const catalogNow = catalog();
    const nextSelected = scopedWorld(scope, catalogNow.enabled, catalogNow.loaded);
    const currentRoute = untrack(() => router.current());
    const currentRouteName = routeNameOf(currentRoute);
    // 补偿池等页面也用 `auction` 参数，只有看板上的往期场次随世界切换离开。
    const historicalBoard =
      isBoardRouteName(currentRouteName) && readPublicQuery().auctionId !== null;
    // 用户主动切换要建立可返回的历史条目；写入口同时推进共享位置版本。
    const written = updatePublicQueryState(
      props.shellQueryState,
      (query) => {
        Object.assign(
          query,
          worldScopeSwitchQuery(currentRouteName, query, scope, historicalBoard),
        );
      },
      "push",
    );
    if (written) {
      setSelectedWorld(nextSelected);
    }
  };
  return { options, current, choose, selectable };
}

/** 页面内的世界按钮组；没有可切换项时整组不渲染，也不建立资源或 effect。 */
export function WorldTabs(): JSX.Element {
  const worldSwitch = useContext(WorldSwitchContext);
  const enabledCount = () => worldSwitch.options().filter((option) => !option.disabled).length;
  const disabledCurrent = () =>
    worldSwitch
      .options()
      .some((option) => option.disabled && option.value === worldSwitch.current());
  return (
    <Show when={enabledCount() > 1 || disabledCurrent()}>
      <nav class="public-auction-filter public-world-filter" aria-label={t("world-label")}>
        <span class="public-auction-filter__label">{t("world-label")}</span>
        <For each={worldSwitch.options()} keyed={(option) => option.value}>
          {(option) => (
            <button
              type="button"
              class="public-auction-filter__tab"
              aria-current={worldSwitch.current() === option().value ? "true" : "false"}
              aria-pressed={worldSwitch.current() === option().value ? "true" : "false"}
              disabled={option().disabled}
              onClick={() => worldSwitch.choose(option().value)}
            >
              {option().value === "all"
                ? t("auction-history-world-all")
                : worldLabel(option().value)}
            </button>
          )}
        </For>
      </nav>
    </Show>
  );
}

function isBoardRouteName(routeName: string | null): boolean {
  return routeName === "AuctionIndex" || routeName === "AuctionPage";
}

function routeNameOf(route: RouteMatch): string | null {
  return route.kind === "matched" ? route.name : null;
}

function activeSingleWorld(enabled: readonly string[], requested: string): string {
  if (enabled.includes(requested)) {
    return requested;
  }
  // 目录沿用接口键顺序，但纠偏没有目标世界时仍以 Main 作为默认世界。
  return enabled.find((world) => world === "main") ?? enabled[0] ?? "main";
}

function pickerWorldScope(value: string): WorldScope | null {
  if (value === "all" || value === "main" || value === "isekai") {
    return value;
  }
  return null;
}

function headerWorlds(worlds: WorldsResponse): string[] {
  // 只接受两个冻结世界里已开放的；按钮顺序固定为主世界在前、异世界在后，不随接口的键顺序变化。
  return Object.entries(worlds.worlds)
    .filter(([world, status]) => isSupportedWorld(world) && status.enabled)
    .map(([world]) => world)
    .sort((left, right) => worldRank(left) - worldRank(right));
}

/** 历史筛选保留两个受支持世界；业务开关只隐藏当前/可操作世界，不抹除 settled 事实。 */
export function supportedWorlds(worlds: WorldsResponse): string[] {
  const supported = Object.entries(worlds.worlds)
    .filter(([world]) => isSupportedWorld(world))
    .map(([world]) => world);
  // 旧版页面筛选器固定以 Main 为首项。
  supported.sort((left, right) => worldRank(left) - worldRank(right));
  return supported;
}

function worldRank(world: string): number {
  if (world === "main") {
    return 0;
  }
  return world === "isekai" ? 1 : 2;
}

function isSupportedWorld(world: string): boolean {
  return world === "main" || world === "isekai";
}

export function worldLabel(world: string): string {
  if (world === "main") {
    return t("auction-world-main");
  }
  return world === "isekai" ? t("auction-world-isekai") : world;
}
