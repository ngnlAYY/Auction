// Public 看板正文：目录读取、当前场次对齐、开场补读、拍品直达与分页筛选装配。

import type { CurrentAuctionItemResponse } from "@auction/contracts/dto/public/catalog/auction";
import type { PublicLotEnvelopeResponse } from "@auction/contracts/dto/public/catalog/lot";
import { type BaseErrorKind, baseApiErrorMessage } from "@auction/web-shared/net/errors";
import { type Cell, createCell } from "@auction/web-shared/reactive/cell";
import { structuralEquals } from "@auction/web-shared/reactive/equality";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import { err, ok, type Result, unreachable } from "@auction/web-shared/result";
import { useAppBootReady } from "@auction/web-shared/ui/bootstrap";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { Match, Show, Switch, useContext } from "solid-js";

import { type CatalogRow, catalogLevelFilterParse, lotEnvelopeTakeRows } from "../../api/model";
import { fetchCurrentAuction } from "../../api/queries/current";
import { fetchAuctionList } from "../../api/queries/directory";
import { fetchCurrentLots } from "../../api/queries/lots";
import { PublicSessionResourceContext, PublicSessionView } from "..";
import { CatalogGroupFilters } from "../catalog/filters";
import { CatalogTable } from "../catalog/tables";
import { useContentLoading } from "../loading";
import { publicPageMatches, updatePublicPage, updatePublicPageSize } from "../pagination";
import {
  catalogCategoryKeys,
  type PreviewHistoryLoad,
  type PublicBoardQuery,
  updatePublicQueryState,
} from "../query";
import { usePageRouteGuard } from "../query/route-page";
import { useAlignmentReread } from "./alignment";
import { previewHistorySwitch } from "./navigation";
import {
  PublicRefreshGenerationContext,
  refreshPublicReads,
  useAuctionOpeningRefresh,
  useBidRefresh,
} from "./refresh";

/** 公开目录读取的作用域。 */
interface CatalogReadScope {
  auctionId: string;
  world: string;
  category: string;
  equipmentGroup: string;
  maxLevel: string | null;
  includeUnleveled: string;
  page: number;
  pageSize: number;
}

/** 预览可保留的上下文；同一身份和阶段的预览修订更新允许沿用已确认目录。 */
interface CatalogPreviewScope {
  read: CatalogReadScope | null;
  historical: boolean;
  status: string;
  presentationMode: string;
  biddingOpen: boolean;
  userId: string | null;
}

/** 行、数量及分页来自同一次权威响应，后台更新不能混用不同修订的字段。 */
interface CatalogSnapshot {
  page: PublicLotEnvelopeResponse;
  rows: CatalogRow[];
}

/** 目录资源、对齐代号与已消费的竞价修订号共用看板生命周期。 */
function useCatalogLots(readScope: () => CatalogReadScope | null, refreshGeneration: () => number) {
  const lotsRefreshGeneration = createCell(0);
  const request = createValueMemo(() => {
    const scope = readScope();
    return scope === null
      ? null
      : { scope, refresh: refreshGeneration(), generation: lotsRefreshGeneration() };
  });
  const lots = createTaskResource(request, async (requested, context) => {
    if (requested === null) {
      return null;
    }
    const { generation, scope } = requested;
    // Solid 版分类可为集合；空集合不发送 category 参数，Rust 版仍只发送单个分类。
    const category = catalogCategoryKeys(scope.category).length === 0 ? null : scope.category;
    const equipmentGroup = scope.equipmentGroup !== "all" ? scope.equipmentGroup : null;
    const levels = catalogLevelFilterParse(scope.maxLevel ?? "", scope.includeUnleveled);
    if (!levels.ok) {
      return [scope, generation, err("invalid_level_filter")] as const;
    }
    const page = await fetchCurrentLots(
      scope.auctionId,
      category,
      equipmentGroup,
      levels.value,
      scope.page,
      scope.pageSize,
      context.signal,
    );
    if (!context.alive()) {
      return [scope, generation, err("network_error")] as const;
    }
    const result: Result<CatalogSnapshot, string> = page.ok
      ? ok({ rows: lotEnvelopeTakeRows(page.value), page: page.value })
      : page;
    return [scope, generation, result] as const;
  });
  useBidRefresh(
    lots,
    () => readScope() !== null,
    () => readScope()?.auctionId ?? null,
  );
  return { lots, lotsRefreshGeneration };
}

/** 由公开看板装配权威目录和对齐重读；身份子树只消费读取结果。 */
export function useAuctionCatalog(
  auction: () => CurrentAuctionItemResponse | null,
  historical: () => boolean,
  auctionReading: () => boolean,
  queryState: Cell<PublicBoardQuery>,
  queryBlocked: () => boolean,
) {
  const refreshGeneration = useContext(PublicRefreshGenerationContext);
  useAuctionOpeningRefresh(auction, historical, refreshGeneration);
  const catalogScope = createValueMemo((): CatalogReadScope | null => {
    const round = auction();
    if (round === null || queryBlocked()) {
      return null;
    }
    const query = queryState();
    return {
      auctionId: round.id,
      world: round.world,
      category: query.category ?? "all",
      equipmentGroup: query.equipmentGroup ?? "all",
      maxLevel: query.maxLevel,
      includeUnleveled: query.includeUnleveled,
      page: Math.max(query.page, 1),
      pageSize: query.pageSize,
    };
  });
  const { lots, lotsRefreshGeneration } = useCatalogLots(catalogScope, refreshGeneration);
  const current = createValueMemo((): readonly [number, Result<CatalogSnapshot, string>] | null => {
    const loaded = lots();
    if (loaded == null || !structuralEquals(loaded[0], catalogScope())) {
      return null;
    }
    return [loaded[1], loaded[2]];
  });
  const aligned = createValueMemo(() => {
    const round = auction();
    const loaded = current();
    if (round === null || loaded === null || !loaded[1].ok) {
      return true;
    }
    const page = loaded[1].value.page;
    return (
      page.auction_id === round.id &&
      page.auction_status === round.status &&
      page.presentation_mode === round.presentation_mode &&
      page.bidding_open === round.bidding_open &&
      page.preview_revision === round.preview_revision
    );
  });
  const alignmentKey = createValueMemo(() => {
    const round = auction();
    return round === null
      ? ""
      : `${round.id}:${round.status}:${round.presentation_mode}:${round.bidding_open}:${round.preview_revision}`;
  });
  const loadedGeneration = createValueMemo(() => {
    const loaded = current();
    return loaded?.[1].ok ? loaded[0] : null;
  });
  // 场次或目录仍有读取在途时等两侧返回，避免对齐补读取消正在进行的读取。
  const alignment = useAlignmentReread(
    aligned,
    alignmentKey,
    loadedGeneration,
    () => auctionReading() || lots.state() === "pending",
    lotsRefreshGeneration,
    refreshGeneration,
  );
  const readySnapshot = createValueMemo((): CatalogSnapshot | null => {
    const loaded = current();
    const scope = catalogScope();
    if (scope === null || loaded === null || !loaded[1].ok) {
      return null;
    }
    const snapshot = loaded[1].value;
    return aligned() &&
      publicPageMatches(snapshot.page.page, snapshot.page.page_size, scope.page, scope.pageSize)
      ? snapshot
      : null;
  });
  const retry = () => {
    lotsRefreshGeneration.update((generation) => Math.min(generation + 1, Number.MAX_SAFE_INTEGER));
    refreshPublicReads(refreshGeneration);
  };
  const reading = () =>
    current() === null || lots.state() === "pending" || (!aligned() && !alignment.exhausted());
  return { scope: catalogScope, current, readySnapshot, alignment, retry, reading };
}

export interface AuctionDataProps {
  readonly auction: CurrentAuctionItemResponse;
  readonly historical: boolean;
  readonly catalog: ReturnType<typeof useAuctionCatalog>;
  readonly query: Cell<PublicBoardQuery>;
  readonly bootPrerequisitesReady: boolean;
}

export function AuctionData(props: AuctionDataProps): JSX.Element {
  const queryStateSignal = props.query;
  const routeStillOnBoard = usePageRouteGuard("AuctionIndex");
  const activeCategory = createValueMemo(() => queryStateSignal().category ?? "all");
  // 手机分类面板应用新分类会让目录重新读取并卸载承载工具栏的目录表。请求必须放在始终挂载的
  // 看板层，下一次工具栏挂载后才能把焦点还给分类开关；面板自身随目录表一起不存在。
  const categoryFocusRequest = createCell(false);
  const activeEquipmentGroup = createValueMemo(() => queryStateSignal().equipmentGroup ?? "all");
  const activeMaxLevel = createValueMemo(() => queryStateSignal().maxLevel);
  const activeIncludeUnleveled = createValueMemo(() => queryStateSignal().includeUnleveled);
  const pageNumber = createValueMemo(() => Math.max(queryStateSignal().page, 1));
  const requestedPageSize = createValueMemo(() => queryStateSignal().pageSize);
  const refreshGeneration = useContext(PublicRefreshGenerationContext);
  const session = useContext(PublicSessionResourceContext);
  const sessionView = createValueMemo(() => PublicSessionView.fromSnapshot(session()));
  const viewerPlayerId = () => sessionView().viewerPlayerId;
  const sessionReady = () => sessionView().ready;
  const sessionError = () => sessionView().error;
  const authenticated = () => sessionView().authenticated;
  const previewScope = createValueMemo((): CatalogPreviewScope => {
    const snapshot = session();
    const identity = snapshot?.ok === true ? snapshot.value : null;
    return {
      read: props.catalog.scope(),
      historical: props.historical,
      status: props.auction.status,
      presentationMode: props.auction.presentation_mode,
      biddingOpen: props.auction.bidding_open,
      userId: identity?.authenticated === true ? identity.user_id : null,
    };
  });
  const historySwitchWorld = createValueMemo(() =>
    props.historical || props.auction.presentation_mode !== "live" ? props.auction.world : null,
  );
  const historySwitch = createTaskResource(
    () => [refreshGeneration(), historySwitchWorld()] as const,
    async ([, world], context): Promise<readonly [string | null, PreviewHistoryLoad]> => {
      if (world === null) {
        return [null, ok(null)] as const;
      }
      const current = await fetchCurrentAuction(world, context.signal);
      if (!context.alive()) {
        return [world, err("network_error")] as const;
      }
      if (!current.ok) {
        return [world, current] as const;
      }
      if (current.value === null || current.value.presentation_mode === "live") {
        return [world, ok([current.value, []])] as const;
      }
      const rounds = await fetchAuctionList(world, context.signal);
      if (!context.alive()) {
        return [world, err("network_error")] as const;
      }
      return [world, rounds.ok ? ok([current.value, rounds.value] as const) : rounds] as const;
    },
  );
  const current = () => props.catalog.current();
  // 首次揭示要等主要目录已有成功或失败结果；辅助视图切换继续局部异步，避免拖慢整页。
  useAppBootReady(() => props.bootPrerequisitesReady && current() !== null);
  const alignmentExhausted = () => props.catalog.alignment.exhausted();
  const isLivePresentation = () => props.auction.presentation_mode === "live";
  const quietPreview = createValueMemo(
    () => sessionReady() && props.auction.presentation_mode === "preview",
  );
  const readySnapshot = () => props.catalog.readySnapshot();
  // 只缓存当前上下文已对齐的预览；明确失败、切换上下文或离开预览立即清除。
  let retainedPreview: { scope: CatalogPreviewScope; snapshot: CatalogSnapshot } | null = null;
  const retainedPreviewSnapshot = createValueMemo((): CatalogSnapshot | null => {
    const quiet = quietPreview();
    const scopeNow = previewScope();
    const loaded = current();
    const ready = readySnapshot();
    if (
      !quiet ||
      (retainedPreview !== null && !structuralEquals(retainedPreview.scope, scopeNow)) ||
      (loaded !== null && !loaded[1].ok)
    ) {
      retainedPreview = null;
    }
    if (quiet && ready !== null) {
      retainedPreview = { scope: scopeNow, snapshot: ready };
    }
    return quiet ? (retainedPreview?.snapshot ?? null) : null;
  });
  const visibleSnapshot = createValueMemo(() => {
    const retained = retainedPreviewSnapshot();
    return readySnapshot() ?? retained;
  });
  const currentError = createValueMemo(() => {
    const loaded = current();
    return loaded !== null && !loaded[1].ok ? loaded[1].error : null;
  });
  const currentFailed = createValueMemo(() => currentError() !== null);
  // 读取失败与对齐重读用尽都是可操作的最终状态（正文给出重试），不再占用顶栏的加载提示。
  useContentLoading(() => visibleSnapshot() === null && !alignmentExhausted() && !currentFailed());
  const currentHistorySwitch = createValueMemo((): PreviewHistoryLoad | null => {
    const snapshot = historySwitch();
    return snapshot === undefined || snapshot[0] !== historySwitchWorld() ? null : snapshot[1];
  });
  // 只有会显示视图切换的预览/历史场次才等待它；竞价中的当前场次不展示切换。
  useContentLoading(
    () =>
      (props.historical || props.auction.presentation_mode !== "live") &&
      currentHistorySwitch() === null,
  );

  const levelFiltered = () => {
    const maxLevel = activeMaxLevel();
    return (maxLevel !== null && maxLevel !== "") || activeIncludeUnleveled() !== "true";
  };
  const catalog = (): JSX.Element => (
    <Switch fallback={null}>
      <Match when={currentFailed()}>
        <AsyncState
          kind="error"
          title={t("data-error")}
          detail={lotListErrorMessage(currentError() ?? "")}
          retryLabel={t("data-retry")}
          onRetry={() => props.catalog.retry()}
        />
      </Match>
      <Match when={visibleSnapshot() === null && alignmentExhausted()}>
        <AsyncState
          kind="error"
          title={t("data-error")}
          detail={lotListErrorMessage("temporarily_unavailable")}
          retryLabel={t("data-retry")}
          onRetry={() => props.catalog.alignment.attempt.set(null)}
        />
      </Match>
      <Match when={visibleSnapshot()}>
        {(snapshot) => {
          const pageSize = () => snapshot().page.page_size ?? requestedPageSize();
          // 旧 lot 外壳为详情和列表共用，列表 total 可以省略且没有 has_more 字段；满页时
          // 保留一次向后探测，避免把“当前页长度”伪装成总数并永久隐藏下一页。
          const hasMore = () =>
            (snapshot().page.total ?? null) === null && snapshot().rows.length >= pageSize();
          return (
            <CatalogTable
              rows={snapshot().rows}
              projectionKind={snapshot().page.projection_kind}
              auctionStatus={snapshot().page.auction_status}
              categoryCounts={new Map(Object.entries(snapshot().page.category_counts ?? {}))}
              activeCategory={activeCategory()}
              activeEquipmentGroup={activeEquipmentGroup()}
              categoryFocusRequest={categoryFocusRequest}
              levelFiltered={levelFiltered()}
              pageNumber={pageNumber()}
              onCategoryChange={(category) => {
                const nextCategory = catalogCategoryKeys(category).length === 0 ? null : category;
                const latest = queryStateSignal.peek();
                if (latest.category === nextCategory && latest.page === 1) {
                  // 目标值已是当前查询：不重复写历史，但对手机面板仍表示无需重试。
                  return true;
                }
                return updatePublicQueryState(
                  queryStateSignal,
                  (query) => {
                    query.category = nextCategory;
                    query.page = 1;
                  },
                  "push",
                );
              }}
              onPageChange={(page) => {
                updatePublicQueryState(
                  queryStateSignal,
                  (query) => {
                    query.page = page;
                  },
                  "push",
                );
              }}
              onPageClamp={(page) =>
                routeStillOnBoard() && updatePublicPage(queryStateSignal, page, "replace")
              }
              onPageSizeChange={(size) => updatePublicPageSize(queryStateSignal, size)}
              pageTotal={snapshot().page.total ?? null}
              pageSize={pageSize()}
              hasMore={hasMore()}
              // 可出价只看当前展示的已对齐快照：后台重读期间保留旧值，按钮不随请求往返消失再出现。
              canBid={
                readySnapshot() !== null && isLivePresentation() && props.auction.bidding_open
              }
              allowSlow={props.auction.status === "live"}
              proxyDefaultMode={props.auction.proxy_default_mode ?? null}
              sessionReady={sessionReady()}
              authenticated={authenticated()}
              viewerPlayerId={viewerPlayerId()}
            />
          );
        }}
      </Match>
    </Switch>
  );

  return (
    // 标题与截止时间已收进顶部 ticker；场次名称只作为本区域的可访问名称。
    <section
      class="public-auction-page"
      aria-label={t("auction-header-title", { number: props.auction.number })}
    >
      <div class="public-auction-live">
        <Show
          when={
            !props.historical &&
            isLivePresentation() &&
            props.auction.bidding_open &&
            sessionError()
          }
        >
          <div class="ui-async-state" role="alert">
            <p class="ui-async-state__title">{t("session-error")}</p>
            <button class="ui-async-state__retry" type="button" onClick={() => session.restart()}>
              {t("data-retry")}
            </button>
          </div>
        </Show>
        <CatalogGroupFilters
          activeEquipmentGroup={activeEquipmentGroup()}
          maxLevel={activeMaxLevel()}
          includeUnleveled={activeIncludeUnleveled()}
          onEquipmentGroupChange={(equipmentGroup) => {
            updatePublicQueryState(
              queryStateSignal,
              (query) => {
                query.equipmentGroup = equipmentGroup !== "all" ? equipmentGroup : null;
                query.page = 1;
              },
              "push",
            );
          }}
          onLevelChange={(levels) => {
            const maxLevel = levels.maxLevel === null ? null : String(levels.maxLevel);
            const includeUnleveled = String(levels.includeUnleveled);
            const query = queryStateSignal.peek();
            if (query.maxLevel === maxLevel && query.includeUnleveled === includeUnleveled) {
              return;
            }
            updatePublicQueryState(
              queryStateSignal,
              (query) => {
                query.maxLevel = maxLevel;
                query.includeUnleveled = includeUnleveled;
                query.page = 1;
              },
              "push",
            );
          }}
        >
          {previewHistorySwitch(
            currentHistorySwitch(),
            props.auction,
            props.historical,
            queryStateSignal(),
          )}
        </CatalogGroupFilters>
        <Show
          when={
            !props.historical &&
            (isLivePresentation() || props.auction.presentation_mode === "preview")
          }
          fallback={catalog()}
        >
          <section>{catalog()}</section>
        </Show>
      </div>
    </section>
  );
}

/** lot 目录读取失败的本地化文案：先处理本页特有的历史变体码，未命中走共享基础层。 */
function lotListErrorMessage(error: string): string {
  switch (error) {
    case "invalid_level_filter":
      return t("filter-level-invalid");
    case "request_cancelled":
      return t("error-request-cancelled");
    case "NOT_FOUND":
      return t("error-not-found");
    case "internal":
    case "INTERNAL":
      return t("error-internal");
    default:
      break;
  }
  return (
    baseApiErrorMessage(error, (kind: BaseErrorKind) => {
      switch (kind) {
        case "network":
          return t("error-network");
        case "timeout":
          return t("error-request-timeout");
        case "invalid_response":
          return t("error-invalid-response");
        case "not_found":
          return t("error-not-found");
        case "unauthenticated":
        case "forbidden":
        case "rate_limited":
        case "unavailable":
        case "feature_not_supported":
        case "conflict":
        case "invalid_request":
          return t("common-load-error");
        default:
          return unreachable(kind);
      }
    }) ?? t("common-load-error")
  );
}
