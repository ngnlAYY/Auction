// Public 当前拍卖看板的路由入口。
// 看板入口、世界选择、实时刷新、拍卖数据展示、目录与场次的对齐重读调度和预览/历史视图切换分别
// 归档到子模块，使用方直接从定义模块导入。

import type { CurrentAuctionItemResponse } from "@auction/contracts/dto/public/catalog/auction";
import { structuralEquals } from "@auction/web-shared/reactive/equality";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import { err, ok } from "@auction/web-shared/result";
import { useAppBootReady } from "@auction/web-shared/ui/bootstrap";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { createEffect, Match, onCleanup, Switch, useContext } from "solid-js";

import { fetchHistoricalAuction } from "../../api/queries/current";
import { CatalogCommandScope } from "../catalog/command-scope";
import { LotViewRequestContext } from "../catalog/detail/deep-link";
import { CatalogDetailHost } from "../catalog/detail/drawer";
import {
  LotDetailLookupContext,
  LotDetailLookupHost,
  useLotDetailLookup,
} from "../catalog/detail/lookup";
import { PublicSessionResourceContext } from "../index";
import { useContentLoading } from "../loading";
import {
  normalizeWorldQuery,
  redirectPublicAlias,
  updatePublicQueryState,
  usePublicQueryState,
} from "../query";
import {
  CurrentAuctionContext,
  PublicReadinessContext,
  SelectedWorldContext,
} from "../shell/contexts";
import { LotWatchScope } from "../watches";
import { AuctionData } from "./data";
import {
  PublicDisplayedAuctionContext,
  PublicPageRealtimeTargetContext,
  type PublicRealtimeTarget,
  publicRealtimeTargetFromAuction,
} from "./realtime";
import { PublicRefreshGenerationContext, refreshPublicReads } from "./refresh";
import { WorldTabs } from "./world";

export function AuctionIndex(): JSX.Element {
  // 拍品直达链接在目录当前页找不到对应行时退回的只读抽屉；宿主挂在看板之外，不随场次切换
  // 或目录重读卸载。
  const lookup = useLotDetailLookup();
  const viewRequest = useContext(LotViewRequestContext);
  onCleanup(() => {
    // 离开看板即放弃尚未处置的直达请求，之后再回到看板不会突然弹出抽屉。
    viewRequest.set(null);
  });
  return (
    <LotDetailLookupContext value={lookup}>
      <CatalogCommandScope>
        <AuctionBoard />
      </CatalogCommandScope>
      <LotDetailLookupHost lookup={lookup} />
    </LotDetailLookupContext>
  );
}

export function AuctionPage(): JSX.Element {
  return redirectPublicAlias("/");
}

/** 拍卖看板；首屏从同源 Public API 读取当前拍卖，不渲染示例数据。 */
export function AuctionBoard(): JSX.Element {
  const selectedWorld = useContext(SelectedWorldContext);
  const refreshGeneration = useContext(PublicRefreshGenerationContext);
  const queryState = usePublicQueryState();
  const queryStateSignal = queryState.state;
  const readiness = useContext(PublicReadinessContext);
  const session = useContext(PublicSessionResourceContext);
  const currentAuction = useContext(CurrentAuctionContext);
  // 同路径的历史/当前切换也必须订阅查询，不能仅在 popstate 或首页按钮时重读。
  const requestedAuction = createValueMemo(() => queryStateSignal().auctionId);
  const historical = createValueMemo(() => requestedAuction() !== null);
  const historicalCurrent = createTaskResource(
    () => [refreshGeneration(), requestedAuction()] as const,
    async ([, requested], context) => {
      const result =
        requested === null ? ok(null) : await fetchHistoricalAuction(requested, context.signal);
      if (!context.alive()) {
        return [requested, err("network_error")] as const;
      }
      return [requested, result] as const;
    },
  );
  const pageTarget = useContext(PublicPageRealtimeTargetContext);
  createEffect(
    () => ({
      requested: requestedAuction(),
      loaded: historicalCurrent(),
      query: queryStateSignal(),
    }),
    (state) => {
      if (state.requested === null) {
        return;
      }
      let target: PublicRealtimeTarget;
      if (state.loaded === undefined || state.loaded[0] !== state.requested) {
        target = { kind: "checking" };
      } else {
        const result = state.loaded[1];
        if (result.ok && result.value !== null && result.value.id === state.requested) {
          const auction = result.value;
          const world = normalizeWorldQuery(auction.world);
          if (state.query.world !== world) {
            updatePublicQueryState(
              queryStateSignal,
              (query) => {
                query.world = world;
              },
              "replace",
            );
          }
          target = publicRealtimeTargetFromAuction(auction);
        } else {
          target = { kind: "identity_error", targetId: state.requested };
        }
      }
      const next = [{ kind: "auction", id: state.requested } as const, target] as const;
      if (!structuralEquals(pageTarget.peek(), next)) {
        pageTarget.set(next);
      }
    },
  );
  onCleanup(() => {
    const current = pageTarget.peek();
    if (current !== null && current[0].kind === "auction") {
      pageTarget.set(null);
    }
  });
  // 标题与截止时间由外壳 ticker 展示：把正文实际展示的历史场次交给它，回到当前场次或
  // 离开页面时清空。只公布与请求同一拍卖 UUID 的已加载结果；写入前用 peek 比较，不订阅自身。
  const displayedAuction = useContext(PublicDisplayedAuctionContext);
  createEffect(
    () => ({ requested: requestedAuction(), loaded: historicalCurrent() }),
    (state) => {
      let next: CurrentAuctionItemResponse | null = null;
      if (
        state.requested !== null &&
        state.loaded !== undefined &&
        state.loaded[0] === state.requested
      ) {
        const result = state.loaded[1];
        if (result.ok && result.value !== null && result.value.id === state.requested) {
          next = result.value;
        }
      }
      if (!structuralEquals(displayedAuction.peek(), next)) {
        displayedAuction.set(next);
      }
    },
  );
  onCleanup(() => {
    displayedAuction.set(null);
  });
  const current = createValueMemo(() => {
    if (historical()) {
      const loaded = historicalCurrent();
      if (loaded === undefined || loaded[0] !== requestedAuction()) {
        return null;
      }
      return loaded[1];
    }
    const loaded = currentAuction();
    if (loaded === undefined || loaded[0] !== selectedWorld()) {
      return null;
    }
    return loaded[1];
  });
  // 目录对齐重读要等展示中的场次本轮读取也返回才判定用尽；以订阅方式读状态，读取结束即重渲染。
  const auctionReading = createValueMemo(() =>
    historical() ? historicalCurrent.state() === "pending" : currentAuction.state() === "pending",
  );
  const bootPrerequisitesReady = () =>
    readiness() !== undefined && session() !== undefined && currentAuction() !== undefined;
  const terminalWithoutAuction = createValueMemo(() => {
    const loaded = current();
    return loaded !== null && (!loaded.ok || loaded.value === null);
  });
  // 错误与空拍卖本身就是可操作的最终首屏；有拍卖时由 AuctionData 等待主要 lots。
  useAppBootReady(() => bootPrerequisitesReady() && terminalWithoutAuction());
  useContentLoading(() => current() === null);
  const failed = createValueMemo(() => {
    const loaded = current();
    return loaded !== null && !loaded.ok;
  });
  const empty = createValueMemo(() => {
    const loaded = current();
    return loaded?.ok === true && loaded.value === null;
  });
  const auction = createValueMemo(() => {
    const loaded = current();
    return loaded?.ok ? loaded.value : null;
  });

  return (
    <Switch
      fallback={
        // 新世界的当前场次仍在读取时，三个终态分支都不命中；按钮组必须仍在页面上，
        // 否则用户切到另一个世界后要等待读取结束才能切回。
        <div class="public-world-toolbar">
          <WorldTabs />
        </div>
      }
    >
      {/* 读取失败或该世界没有场次时看板的筛选工具栏不存在：世界按钮组单独成行，
          否则切到没有场次的世界之后就没有入口切回来。 */}
      <Match when={failed()}>
        <div class="public-world-toolbar">
          <WorldTabs />
        </div>
        <AsyncState
          kind="error"
          title={t("page-auctions-title")}
          detail={t("data-error")}
          retryLabel={t("data-retry")}
          // 与对齐重读、恢复可见共用刷新入口：先失效公开读缓存，重试才真正回到服务端。
          onRetry={() => refreshPublicReads(refreshGeneration)}
        />
      </Match>
      <Match when={empty()}>
        <div class="public-world-toolbar">
          <WorldTabs />
        </div>
        <AsyncState
          kind="empty"
          title={t("auction-empty-no-auction")}
          detail={t("auction-empty-no-auction-hint")}
        />
      </Match>
      <Match when={auction()}>
        {(loaded) => (
          <LotWatchScope auctionId={loaded().id}>
            <AuctionData
              auction={loaded()}
              historical={historical()}
              auctionReading={auctionReading()}
              bootPrerequisitesReady={bootPrerequisitesReady()}
            />
            {/* 详情抽屉挂在标记作用域内（底部标记按钮需要它），并与目录共存亡。 */}
            <CatalogDetailHost auctionId={loaded().id} />
          </LotWatchScope>
        )}
      </Match>
    </Switch>
  );
}
