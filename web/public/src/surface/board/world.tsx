// Public 看板世界选择与世界标签辅助。

import type { WorldsResponse } from "@auction/contracts/dto/public/account";
import type { Cell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import type { RouteMatch } from "@auction/web-shared/router";
import { useRouter } from "@auction/web-shared/router";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { createEffect, For, useContext } from "solid-js";

import { fetchWorlds } from "../../api/queries/current";
import {
  type PublicBoardQuery,
  PublicLocationRevisionContext,
  publicAuctionHomeQuery,
  updatePublicQueryState,
} from "../query";
import { PublicWorldQueryContext } from "../shell/contexts";

/** 公开端世界选择器；世界是否可用由服务端目录决定，切换后重新读取权威当前拍卖。
 * URL 写回经外壳共享的 query hook 状态信号（context），保证 hook 缓存与地址栏一致，
 * 浏览器前进/后退时 hook 的相等判断才能正确触发恢复。 */
export function WorldPicker(props: { readonly selectedWorld: Cell<string> }): JSX.Element {
  const router = useRouter();
  const historyOwnsWorldFilter = createValueMemo(() =>
    routeOwnsHistoryWorldFilter(router.current()),
  );
  const boardRoute = createValueMemo(() => isBoardRoute(router.current()));
  const shellQueryState = useContext(PublicWorldQueryContext);
  const locationRevisionContext = useContext(PublicLocationRevisionContext);
  if (locationRevisionContext === null) {
    throw new Error("WorldPicker 必须在 PublicLocationRevisionContext 内渲染");
  }
  const locationRevision = locationRevisionContext.revision;
  const historicalUrl = createValueMemo(
    () => historyOwnsWorldFilter() || shellQueryState().auctionId !== null,
  );
  const worlds = createTaskResource(
    () => null,
    async (_dependencies, context) => fetchWorlds(context.signal),
  );
  const loadedWorlds = createValueMemo((): { options: string[]; loaded: boolean } => {
    const snapshot = worlds();
    return snapshot?.ok === true
      ? { options: headerWorlds(snapshot.value), loaded: true }
      : { options: ["main", "isekai"], loaded: false };
  });
  const current = () => props.selectedWorld();
  // 关闭世界的 settled 深链仍显示其真实世界，但不提供新的当前世界入口。
  const historicalDisabledWorld = createValueMemo(
    () => historicalUrl() && !loadedWorlds().options.includes(current()),
  );
  const options = createValueMemo(() => {
    const next = loadedWorlds().options.slice();
    if (historicalDisabledWorld()) {
      next.push(current());
    }
    return next;
  });
  const active = createValueMemo(() => {
    const worldsNow = options();
    if (worldsNow.includes(current())) {
      return current();
    }
    // 旧版顶部选项沿用接口键顺序，但没有 URL 状态时仍以 Main 作为默认世界。
    return worldsNow.find((world) => world === "main") ?? worldsNow[0] ?? "main";
  });
  const canCanonicalize = createValueMemo(
    () => loadedWorlds().loaded && options().length > 0 && !historicalUrl(),
  );
  createEffect(
    () => ({
      worldsLoaded: loadedWorlds().loaded,
      active: active(),
      canCanonicalize: canCanonicalize(),
      historicalUrl: historicalUrl(),
    }),
    (state) => {
      if (
        !state.worldsLoaded ||
        state.historicalUrl ||
        props.selectedWorld.peek() === state.active
      ) {
        return;
      }
      props.selectedWorld.set(state.active);
      if (!state.canCanonicalize) {
        return;
      }
      // 仅当前世界失效时纠偏；历史筛选与指定场次深链不得被目录开关抹除。
      updatePublicQueryState(
        shellQueryState,
        (query) => {
          query.world = state.active !== "main" ? state.active : null;
        },
        "replace",
      );
    },
  );
  const optionsEmpty = createValueMemo(() => options().length === 0);

  return (
    <label class="public-world-switcher">
      <span>{t("world-label")}</span>
      <select
        id="public-world"
        name="world"
        value={active()}
        disabled={optionsEmpty()}
        onChange={(event) => {
          const world = event.currentTarget.value;
          props.selectedWorld.set(world);
          if (historyOwnsWorldFilter()) {
            return;
          }
          // 补偿池等页面也用 `auction` 参数，只有看板上的往期场次随世界切换离开。
          const historicalBoard = boardRoute() && shellQueryState.peek().auctionId !== null;
          // 用户主动切换要建立可返回的历史条目。
          updatePublicQueryState(
            shellQueryState,
            (query) => {
              Object.assign(query, worldSwitchQuery(query, world, historicalBoard));
            },
            "push",
          );
          if (historicalBoard) {
            // pushState 不经过 Router：推进位置版本，让看板的查询状态重读地址栏，
            // 正文与顶栏随之换成目标世界的当前场次。
            locationRevision.update((revision) => Math.min(revision + 1, Number.MAX_SAFE_INTEGER));
          }
        }}
      >
        <For each={options()} keyed={false}>
          {(world) => (
            <option
              value={world()}
              selected={active() === world()}
              disabled={historicalDisabledWorld() && world() === current()}
            >
              {worldLabel(world())}
            </option>
          )}
        </For>
      </select>
    </label>
  );
}

function isBoardRoute(route: RouteMatch): boolean {
  return (
    route.kind === "matched" && (route.name === "AuctionIndex" || route.name === "AuctionPage")
  );
}

function routeOwnsHistoryWorldFilter(route: RouteMatch): boolean {
  return (
    route.kind === "matched" &&
    (route.name === "AuctionList" ||
      route.name === "BidLog" ||
      route.name === "Fulfillment" ||
      route.name === "History")
  );
}

/** 页头切换世界后的查询。看板正在展示往期场次时回到目标世界的首页（清除场次、筛选与页码，
 * 每页条数取本地偏好），与顶栏首页入口一致，避免页头世界与正文、顶栏的场次不一致；
 * 其余情况只改世界。 */
function worldSwitchQuery(
  current: PublicBoardQuery,
  world: string,
  historicalBoard: boolean,
): PublicBoardQuery {
  if (historicalBoard) {
    return publicAuctionHomeQuery(world);
  }
  return {
    ...current,
    // 主世界沿用旧版无参数 URL。
    world: world !== "main" ? world : null,
  };
}

function headerWorlds(worlds: WorldsResponse): string[] {
  // 旧版顶部选择器直接沿用 worlds 对象的稳定键顺序，但只接受两个冻结世界。
  return Object.entries(worlds.worlds)
    .filter(([world, status]) => isSupportedWorld(world) && status.enabled)
    .map(([world]) => world);
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
