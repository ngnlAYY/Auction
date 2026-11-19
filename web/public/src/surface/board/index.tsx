// Public 当前拍卖看板的路由入口。
// 看板入口、世界选择、实时刷新、拍卖数据展示、目录与场次的对齐重读调度和预览/历史视图切换分别
// 归档到子模块，使用方直接从定义模块导入。

import type { CurrentAuctionItemResponse } from "@auction/contracts/dto/public/catalog/auction";
import { type Cell, createCell } from "@auction/web-shared/reactive/cell";
import { structuralEquals } from "@auction/web-shared/reactive/equality";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import { err, ok, type Result } from "@auction/web-shared/result";
import { Link, useRouter } from "@auction/web-shared/router";
import { useAppBootReady } from "@auction/web-shared/ui/bootstrap";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import {
  createComponent,
  createEffect,
  For,
  Match,
  onCleanup,
  Show,
  Switch,
  untrack,
  useContext,
} from "solid-js";

import { fetchHistoricalAuction } from "../../api/queries/current";
import { PublicCommandOwnerContext, PublicSessionResourceContext } from "..";
import { CatalogProxyScope } from "../catalog/actions";
import { CatalogCommandScope } from "../catalog/command-scope";
import { CatalogDetailHost } from "../catalog/detail/drawer";
import {
  LotDetailLookupContext,
  LotDetailLookupHost,
  useLotDetailLookup,
} from "../catalog/detail/lookup";
import { disposeLotView, LotViewRequestContext } from "../catalog/detail/lot-view";
import { useContentLoading } from "../loading";
import { usePublicPageBounds } from "../pagination/bounds";
import {
  normalizeWorldQuery,
  type PublicBoardQuery,
  publicAuctionHomeQuery,
  publicHref,
  queryAuctionId,
  redirectPublicAlias,
  usePublicQueryState,
} from "../query";
import { readPublicQuery, updatePublicQueryPairs } from "../query/location";
import { usePageRouteGuard } from "../query/route-page";
import {
  CurrentAuctionContext,
  PublicReadinessContext,
  SelectedWorldContext,
} from "../shell/contexts";
import { LotWatchScope } from "../watches";
import { AuctionData, useAuctionCatalog } from "./data";
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
    // 即使 replace 仍在同一历史条目，离开看板也放弃尚未交付的请求。
    viewRequest.set(null);
  });
  return (
    <LotDetailLookupContext value={lookup}>
      <AuctionBoard />
      <LotDetailLookupHost lookup={lookup} />
    </LotDetailLookupContext>
  );
}

export function AuctionPage(): JSX.Element {
  const query = usePublicQueryState().state;
  return (
    <Show
      when={query().auction.kind !== "invalid"}
      fallback={<InvalidAuctionLink world={query().world} />}
    >
      <AuctionAlias />
    </Show>
  );
}

function AuctionAlias(): JSX.Element {
  return redirectPublicAlias("/");
}

/** 拍卖看板；首屏从同源 Public API 读取当前拍卖，不渲染示例数据。 */
export function AuctionBoard(): JSX.Element {
  return mountAuctionBoard(usePublicQueryState().state);
}

interface AuctionBoardViewProps {
  readonly userId: string | null;
  readonly invalid: boolean;
  readonly failed: boolean;
  readonly worldCorrectionFailed: boolean;
  readonly empty: boolean;
  readonly pageBounds: ReturnType<typeof usePublicPageBounds>;
  readonly auction: CurrentAuctionItemResponse | null;
  readonly historical: boolean;
  readonly bootPrerequisitesReady: boolean;
  readonly catalog: ReturnType<typeof useAuctionCatalog>;
  readonly query: Cell<PublicBoardQuery>;
  readonly onRetry: () => void;
  readonly onRetryWorldCorrection: () => void;
}

interface WorldCorrection {
  readonly auctionId: string;
  readonly world: string;
}

type HistoricalBoard =
  | { readonly kind: "correct-world"; readonly correction: WorldCorrection }
  | {
      readonly kind: "loaded";
      readonly result: Result<CurrentAuctionItemResponse | null, string>;
    };

/** 先装配看板的公开读取，再挂载随身份更换的正文与命令宿主。 */
export function mountAuctionBoard(
  queryStateSignal: Cell<PublicBoardQuery>,
  View = AuctionBoardView,
): JSX.Element {
  const selectedWorld = useContext(SelectedWorldContext);
  const refreshGeneration = useContext(PublicRefreshGenerationContext);
  const readiness = useContext(PublicReadinessContext);
  const session = useContext(PublicSessionResourceContext);
  const owner = useContext(PublicCommandOwnerContext);
  const currentAuction = useContext(CurrentAuctionContext);
  const routeStillOnBoard = usePageRouteGuard("AuctionIndex");
  const invalid = createValueMemo(() => queryStateSignal().auction.kind === "invalid");
  const pageBounds = usePublicPageBounds(queryStateSignal, "AuctionIndex", (query) =>
    query.auction.kind === "invalid" || query.invalidCategory !== null
      ? []
      : [["page", query.pageSize]],
  );
  const queryBlocked = () =>
    queryStateSignal().invalidCategory !== null || pageBounds.blocked("page");
  // 同路径的历史/当前切换也必须订阅查询，不能仅在 popstate 或首页按钮时重读。
  const requestedAuction = createValueMemo(() => queryAuctionId(queryStateSignal()));
  const historical = createValueMemo(() => requestedAuction() !== null);
  const historyRequest = createValueMemo(() => (queryBlocked() ? null : requestedAuction()));
  const historicalCurrent = createTaskResource(
    () => [refreshGeneration(), historyRequest()] as const,
    async ([, requested], context) => {
      const result =
        requested === null ? ok(null) : await fetchHistoricalAuction(requested, context.signal);
      if (!context.alive()) {
        return [requested, err("network_error")] as const;
      }
      return [requested, result] as const;
    },
  );
  const historicalBoard = createValueMemo((): HistoricalBoard | null => {
    const requested = historyRequest();
    const loaded = historicalCurrent();
    if (requested === null || loaded === undefined || loaded[0] !== requested) {
      return null;
    }
    const result = loaded[1];
    if (!result.ok || result.value === null) {
      return { kind: "loaded", result };
    }
    const auction = result.value;
    const world = normalizeWorldQuery(auction.world);
    if (auction.id !== requested || world === null) {
      return { kind: "loaded", result: err("invalid_response") };
    }
    if (queryStateSignal().world !== world) {
      return { kind: "correct-world", correction: { auctionId: requested, world } };
    }
    // 写入成功后还要等外壳读到同一地址，顶栏、订阅和目录才可发布该场次。
    return selectedWorld() === world ? { kind: "loaded", result } : null;
  });
  const worldCorrection = createValueMemo(() => {
    const board = historicalBoard();
    return board?.kind === "correct-world" ? board.correction : null;
  });
  const rejectedWorld = createCell<WorldCorrection | null>(null);
  const correctWorld = ({ auctionId, world }: WorldCorrection): void => {
    if (!routeStillOnBoard()) return;
    const latest = readPublicQuery();
    // 异步定位只纠正仍指向本场且有效的查询，保留其他筛选的原始字节。
    if (
      queryAuctionId(latest) !== auctionId ||
      latest.invalidCategory !== null ||
      latest.world === world
    ) {
      return;
    }
    const written = updatePublicQueryPairs(
      queryStateSignal,
      ["world"],
      [["world", world]],
      "replace",
    );
    rejectedWorld.set(written ? null : { auctionId, world });
  };
  createEffect(worldCorrection, (correction) => {
    if (correction === null) {
      if (rejectedWorld.peek() !== null) rejectedWorld.set(null);
    } else if (!structuralEquals(rejectedWorld.peek(), correction)) {
      correctWorld(correction);
    }
  });
  const worldCorrectionFailed = createValueMemo(() => {
    const correction = worldCorrection();
    return correction !== null && structuralEquals(rejectedWorld(), correction);
  });
  const pageTarget = useContext(PublicPageRealtimeTargetContext);
  const displayedAuction = useContext(PublicDisplayedAuctionContext);
  createEffect(
    () => [requestedAuction(), historicalBoard()] as const,
    ([requested, board]) => {
      const auction = board?.kind === "loaded" && board.result.ok ? board.result.value : null;
      if (!structuralEquals(displayedAuction.peek(), auction)) {
        displayedAuction.set(auction);
      }
      if (requested === null) {
        if (pageTarget.peek()?.[0].kind === "auction") {
          pageTarget.set(null);
        }
        return;
      }
      let target: PublicRealtimeTarget = { kind: "checking" };
      if (auction !== null) {
        target = publicRealtimeTargetFromAuction(auction);
      } else if (board?.kind === "loaded") {
        target = { kind: "identity_error", targetId: requested };
      }
      const next = [{ kind: "auction", id: requested } as const, target] as const;
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
    displayedAuction.set(null);
  });
  const current = createValueMemo(() => {
    // 显式目标非法时关闭目录与出价入口，直到玩家主动离开错误地址。
    if (invalid()) {
      return null;
    }
    if (historical()) {
      const board = historicalBoard();
      return board?.kind === "loaded" ? board.result : null;
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
  useAppBootReady(
    () =>
      bootPrerequisitesReady() &&
      (terminalWithoutAuction() ||
        worldCorrectionFailed() ||
        queryStateSignal().invalidCategory !== null ||
        pageBounds.failed("page")),
  );
  useContentLoading(
    () => !invalid() && !queryBlocked() && !worldCorrectionFailed() && current() === null,
  );
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
  // 公开读与补读游标早于身份子树建立，换号只替换私有展示和命令宿主。
  const catalog = useAuctionCatalog(
    auction,
    historical,
    auctionReading,
    queryStateSignal,
    queryBlocked,
  );

  return For({
    get each() {
      return [owner()];
    },
    keyed: (userId) => userId,
    children: (userId) =>
      createComponent(CatalogCommandScope, {
        userId: untrack(userId),
        get auction() {
          return auction();
        },
        get auctionInvalid() {
          return invalid();
        },
        get children() {
          // 目录分支可以卸载，页面宿主只保留未发送草稿与抽屉。
          return createComponent(View, {
            userId: untrack(userId),
            get invalid() {
              return invalid();
            },
            get failed() {
              return failed();
            },
            get worldCorrectionFailed() {
              return worldCorrectionFailed();
            },
            get empty() {
              return empty();
            },
            pageBounds,
            get auction() {
              return auction();
            },
            get historical() {
              return historical();
            },
            get bootPrerequisitesReady() {
              return bootPrerequisitesReady();
            },
            catalog,
            query: queryStateSignal,
            onRetry: () => refreshPublicReads(refreshGeneration),
            onRetryWorldCorrection: () => {
              const correction = untrack(worldCorrection);
              if (correction !== null) correctWorld(correction);
            },
          });
        },
      }),
  });
}

function AuctionBoardView(props: AuctionBoardViewProps): JSX.Element {
  const router = useRouter();
  const routeStillOnBoard = usePageRouteGuard("AuctionIndex");
  const request = useContext(LotViewRequestContext);
  const lookup = useContext(LotDetailLookupContext);
  createEffect(
    () => {
      const queued = request();
      if (queued === null || queued.entryKey !== router.currentEntryKey()) return null;
      const ready = props.catalog.readySnapshot();
      const reading =
        !props.invalid &&
        !props.failed &&
        !props.empty &&
        props.query().invalidCategory === null &&
        !props.pageBounds.failed("page") &&
        (props.auction === null || (ready === null && props.catalog.reading()));
      const disposition = disposeLotView(
        queued.target,
        props.query().world ?? "main",
        props.auction,
        reading,
        ready?.rows.map((row) => row.lotId) ?? [],
      );
      return { queued, disposition };
    },
    (delivery) => {
      if (delivery === null) return;
      const { queued, disposition } = delivery;
      if (request.peek() !== queued || queued.entryKey !== untrack(router.currentEntryKey)) return;
      // 空场和最终读取失败都在本层收口，只有对齐的目录行允许接手请求。
      if (disposition === "row" && !queued.target.rowReady) {
        request.set({ ...queued, target: { ...queued.target, rowReady: true } });
      } else if (disposition === "read_only") {
        request.set(null);
        lookup?.view(queued);
      }
    },
  );
  return (
    <Switch
      fallback={
        // 新世界的当前场次仍在读取时，三个终态分支都不命中；按钮组必须仍在页面上，
        // 否则用户切到另一个世界后要等待读取结束才能切回。
        <div class="public-auction-group-toolbar public-world-toolbar">
          <WorldTabs />
        </div>
      }
    >
      <Match when={props.invalid}>
        <InvalidAuctionLink world={props.query().world} />
      </Match>
      <Match when={props.query().invalidCategory}>
        {(values) => (
          <div class="ui-async-state ui-async-state--error">
            <p id="public-catalog-category-error" role="status" aria-live="polite">
              {values().length > 1 ? t("filter-error-duplicate") : t("filter-category-invalid")}
              <bdi> {values().join(" / ")}</bdi>
            </p>
            <button
              class="ui-async-state__retry"
              type="button"
              aria-describedby="public-catalog-category-error"
              onClick={() => {
                if (routeStillOnBoard()) {
                  updatePublicQueryPairs(props.query, ["type", "page"], [], "push");
                }
              }}
            >
              {t("filter-category-clear")}
            </button>
          </div>
        )}
      </Match>
      <Match when={props.pageBounds.failed("page")}>
        <AsyncState
          kind="error"
          title={t("data-error")}
          detail={t("pagination-clamp-error")}
          retryLabel={t("data-retry")}
          onRetry={props.pageBounds.retry}
        />
      </Match>
      {/* 读取失败或该世界没有场次时看板的筛选工具栏不存在：世界按钮组单独成行，
          否则切到没有场次的世界之后就没有入口切回来。 */}
      <Match when={props.worldCorrectionFailed}>
        <div class="public-auction-group-toolbar public-world-toolbar">
          <WorldTabs />
        </div>
        <AsyncState
          kind="error"
          title={t("page-auctions-title")}
          detail={t("world-correction-error")}
          retryLabel={t("data-retry")}
          onRetry={props.onRetryWorldCorrection}
        />
      </Match>
      <Match when={props.failed}>
        <div class="public-auction-group-toolbar public-world-toolbar">
          <WorldTabs />
        </div>
        <AsyncState
          kind="error"
          title={t("page-auctions-title")}
          detail={t("data-error")}
          retryLabel={t("data-retry")}
          // 与对齐重读、恢复可见共用刷新入口：先失效公开读缓存，重试才真正回到服务端。
          onRetry={() => props.onRetry()}
        />
      </Match>
      <Match when={props.empty}>
        <div class="public-auction-group-toolbar public-world-toolbar">
          <WorldTabs />
        </div>
        <AsyncState
          kind="empty"
          title={t("auction-empty-no-auction")}
          detail={t("auction-empty-no-auction-hint")}
        />
      </Match>
      <Match when={props.auction}>
        {(loaded) => (
          <LotWatchScope auctionId={loaded().id}>
            <CatalogProxyScope auctionId={loaded().id}>
              <AuctionData
                auction={loaded()}
                historical={props.historical}
                catalog={props.catalog}
                query={props.query}
                bootPrerequisitesReady={props.bootPrerequisitesReady}
              />
            </CatalogProxyScope>
            {/* 详情抽屉挂在标记作用域内（底部标记按钮需要它），并与目录共存亡。 */}
            <CatalogDetailHost auctionId={loaded().id} />
          </LotWatchScope>
        )}
      </Match>
    </Switch>
  );
}

function InvalidAuctionLink(props: { readonly world: string | null }): JSX.Element {
  useAppBootReady(() => true);
  return (
    <section class="ui-async-state ui-async-state--error" role="alert">
      <h2 class="ui-async-state__title">{t("auction-invalid-link")}</h2>
      <p class="ui-async-state__detail">
        <Link href={publicHref("/", publicAuctionHomeQuery(props.world))}>
          {t("auction-view-current")}
        </Link>
      </p>
    </section>
  );
}
