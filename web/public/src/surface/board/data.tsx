// Public 看板正文：目录读取、当前场次对齐、开场补读、拍品直达与分页筛选装配。

import type { CurrentAuctionItemResponse } from "@auction/contracts/dto/public/catalog/auction";
import type { PublicLotEnvelopeResponse } from "@auction/contracts/dto/public/catalog/lot";
import { type BaseErrorKind, baseApiErrorMessage } from "@auction/web-shared/net/errors";
import { useSecondTick } from "@auction/web-shared/platform/clock";
import type { PageVisibility } from "@auction/web-shared/platform/visibility";
import { createCell } from "@auction/web-shared/reactive/cell";
import { structuralEquals } from "@auction/web-shared/reactive/equality";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import { err, ok, type Result, unreachable } from "@auction/web-shared/result";
import { useAppBootReady } from "@auction/web-shared/ui/bootstrap";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { createEffect, Match, Show, Switch, useContext } from "solid-js";

import { estimatedNow } from "../../api/clock";
import { type CatalogRow, catalogLevelFilterParse, lotEnvelopeTakeRows } from "../../api/model";
import { fetchCurrentAuction } from "../../api/queries/current";
import { fetchAuctionList } from "../../api/queries/directory";
import { fetchCurrentLots } from "../../api/queries/lots";
import { parseStrictRfc3339 } from "../../api/session";
import { disposeLotView, LotViewRequestContext } from "../catalog/detail/deep-link";
import { LotDetailLookupContext } from "../catalog/detail/lookup";
import { CatalogGroupFilters } from "../catalog/filters";
import { CatalogTable } from "../catalog/tables";
import { PublicSessionResourceContext, PublicSessionView } from "../index";
import { useContentLoading } from "../loading";
import { publicPageMatches, updatePublicPageSize } from "../pagination";
import type { PreviewHistoryLoad } from "../query";
import { catalogCategoryKeys, updatePublicQueryState, usePublicQueryState } from "../query";
import { PageVisibilityContext } from "../shell/contexts";
import { useAlignmentReread } from "./alignment";
import { previewHistorySwitch } from "./navigation";
import { PublicRefreshGenerationContext, refreshPublicReads, useBidRefresh } from "./refresh";

/** 目录读取与展示的上下文；预览修订号不属于上下文，允许同场后台更新时保留已确认目录。 */
interface CatalogScope {
  auctionId: string;
  world: string;
  historical: boolean;
  status: string;
  presentationMode: string;
  biddingOpen: boolean;
  sessionReady: boolean;
  userId: string | null;
  category: string;
  equipmentGroup: string;
  maxLevel: string | null;
  includeUnleveled: string;
  page: number;
  pageSize: number;
}

/** 行、数量及分页来自同一次权威响应，后台更新不能混用不同修订的字段。 */
interface CatalogSnapshot {
  page: PublicLotEnvelopeResponse;
  rows: CatalogRow[];
}

export interface AuctionDataProps {
  readonly auction: CurrentAuctionItemResponse;
  readonly historical: boolean;
  /** 提供 `auction` 的当前场次（或历史场次）读取仍在进行；对齐重读据此等待本轮读完再判定用尽。 */
  readonly auctionReading: boolean;
  readonly bootPrerequisitesReady: boolean;
}

export function AuctionData(props: AuctionDataProps): JSX.Element {
  // 开场检查只在 effect 中读取共享秒针；倒计时由叶子组件订阅，目录不随每秒重渲染。
  const tick = useSecondTick();
  const queryState = usePublicQueryState();
  const queryStateSignal = queryState.state;
  const activeCategory = createValueMemo(() => queryStateSignal().category ?? "all");
  // 手机分类面板应用新分类会让目录重新读取并卸载承载工具栏的目录表。请求必须放在始终挂载的
  // 看板层，下一次工具栏挂载后才能把焦点还给分类开关；面板自身随目录表一起不存在。
  const categoryFocusRequest = createCell(false);
  const activeEquipmentGroup = createValueMemo(() => queryStateSignal().equipmentGroup ?? "all");
  const activeMaxLevel = createValueMemo(() => queryStateSignal().maxLevel);
  const activeIncludeUnleveled = createValueMemo(() => queryStateSignal().includeUnleveled);
  const pageNumber = createValueMemo(() => Math.max(queryStateSignal().page, 1));
  const requestedPageSize = createValueMemo(() => queryStateSignal().pageSize);
  const visibility = useContext<PageVisibility>(PageVisibilityContext);
  const refreshGeneration = useContext(PublicRefreshGenerationContext);
  const shouldRefreshAtOpen = () => !props.historical && props.auction.presentation_mode !== "live";
  const openingAt = createValueMemo(() => parseStrictRfc3339(props.auction.opens_at));
  const openingKey = createValueMemo(() => `${props.auction.id}:${props.auction.opens_at}`);
  const openingRefreshKey = createCell<string | null>(null);
  const openingRefreshed = createCell(false);
  createEffect(
    () => ({
      tick: tick(),
      key: openingKey(),
      opensAt: openingAt(),
      should: shouldRefreshAtOpen(),
    }),
    (current) => {
      if (openingRefreshKey.peek() !== current.key) {
        openingRefreshKey.set(current.key);
        openingRefreshed.set(false);
      }
      // 预览不建立 WebSocket，不能用 Live 连接态作为开场补读前提；这里只失效权威读取。
      if (
        current.should &&
        !openingRefreshed.peek() &&
        visibility.isVisible() &&
        current.opensAt !== null &&
        BigInt(estimatedNow()) * 1_000_000n >= current.opensAt
      ) {
        openingRefreshed.set(true);
        // 开场是一次已确认的生命周期迁移：TTL 内的预览快照可能仍标着 preview，
        // 必须先失效公开读缓存，否则开场补读会命中旧值并拖到下一次轮询才切换 live。
        refreshPublicReads(refreshGeneration);
      }
    },
  );
  const session = useContext(PublicSessionResourceContext);
  const sessionView = createValueMemo(() => PublicSessionView.fromSnapshot(session()));
  const viewerPlayerId = () => sessionView().viewerPlayerId;
  const sessionReady = () => sessionView().ready;
  const sessionError = () => sessionView().error;
  const authenticated = () => sessionView().authenticated;
  const catalogScope = createValueMemo((): CatalogScope => {
    const snapshot = session();
    const identity = snapshot?.ok === true ? snapshot.value : null;
    return {
      auctionId: props.auction.id,
      world: props.auction.world,
      historical: props.historical,
      status: props.auction.status,
      presentationMode: props.auction.presentation_mode,
      biddingOpen: props.auction.bidding_open,
      sessionReady: snapshot?.ok === true,
      userId: identity?.authenticated === true ? identity.user_id : null,
      category: activeCategory(),
      equipmentGroup: activeEquipmentGroup(),
      maxLevel: activeMaxLevel(),
      includeUnleveled: activeIncludeUnleveled(),
      page: pageNumber(),
      pageSize: requestedPageSize(),
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
      const rounds = await fetchAuctionList(world, context.signal);
      if (!context.alive()) {
        return [world, err("network_error")] as const;
      }
      return [world, rounds.ok ? ok([current.value, rounds.value] as const) : rounds] as const;
    },
  );
  const lotsRefreshGeneration = createCell(0);
  const lots = createTaskResource(
    () => [refreshGeneration(), lotsRefreshGeneration(), catalogScope()] as const,
    async ([, generation, scope], context) => {
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
    },
  );
  useBidRefresh(lots, () => true);
  const current = createValueMemo((): readonly [number, Result<CatalogSnapshot, string>] | null => {
    const loaded = lots();
    if (loaded === undefined || !structuralEquals(loaded[0], catalogScope())) {
      return null;
    }
    return [loaded[1], loaded[2]];
  });
  const aligned = createValueMemo(() => {
    const loaded = current();
    if (loaded === null || !loaded[1].ok) {
      return true;
    }
    const page = loaded[1].value.page;
    return (
      page.auction_id === props.auction.id &&
      page.auction_status === props.auction.status &&
      page.presentation_mode === props.auction.presentation_mode &&
      page.bidding_open === props.auction.bidding_open &&
      page.preview_revision === props.auction.preview_revision
    );
  });
  // 首次揭示要等主要目录已有成功或失败结果；辅助视图切换继续局部异步，避免拖慢整页。
  useAppBootReady(() => props.bootPrerequisitesReady && current() !== null);
  const alignmentKey = createValueMemo(
    () =>
      `${props.auction.id}:${props.auction.status}:${props.auction.presentation_mode}:${props.auction.bidding_open}:${props.auction.preview_revision}`,
  );
  const loadedGeneration = createValueMemo(() => {
    const loaded = current();
    return loaded?.[1].ok ? loaded[0] : null;
  });
  // 场次或目录仍有读取在途（轮询、实时刷新或上一轮对齐重读）：返回后可能就对齐。
  const reading = createValueMemo(() => props.auctionReading || lots.state() === "pending");
  const alignment = useAlignmentReread(
    () => aligned(),
    () => alignmentKey(),
    () => loadedGeneration(),
    () => reading(),
    lotsRefreshGeneration,
    refreshGeneration,
  );
  const alignmentExhausted = alignment.exhausted;
  const alignmentAttempt = alignment.attempt;
  const isLivePresentation = () => props.auction.presentation_mode === "live";
  const quietPreview = createValueMemo(
    () => catalogScope().sessionReady && catalogScope().presentationMode === "preview",
  );
  const readySnapshot = createValueMemo((): CatalogSnapshot | null => {
    const loaded = current();
    if (loaded === null || !loaded[1].ok) {
      return null;
    }
    const snapshot = loaded[1].value;
    return aligned() &&
      publicPageMatches(
        snapshot.page.page,
        snapshot.page.page_size,
        catalogScope().page,
        catalogScope().pageSize,
      )
      ? snapshot
      : null;
  });
  // 只缓存当前上下文已对齐的预览；明确失败、切换上下文或离开预览立即清除。
  let retainedPreview: { scope: CatalogScope; snapshot: CatalogSnapshot } | null = null;
  const retainedPreviewSnapshot = createValueMemo((): CatalogSnapshot | null => {
    const quiet = quietPreview();
    const scopeNow = catalogScope();
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
  // 拍品直达链接：目录当前页有这一行时由行自己打开抽屉；看板展示的不是它所在的场次，或
  // 这一页没有它时，在这里退回只读抽屉。请求只处置一次，不写 URL。
  const viewRequest = useContext(LotViewRequestContext);
  const lotLookup = useContext(LotDetailLookupContext);
  if (lotLookup === null) {
    throw new Error("AuctionData 必须在 LotDetailLookupContext 内渲染");
  }
  const readOnlyView = createValueMemo(() => {
    const target = viewRequest();
    if (target === null) {
      return null;
    }
    const ready = readySnapshot();
    const pageLots = ready === null ? null : ready.rows.map((row) => row.lotId);
    return disposeLotView(
      target,
      queryStateSignal().world ?? "main",
      props.auction.world,
      props.auction.id,
      pageLots,
    ) === "read_only"
      ? target
      : null;
  });
  createEffect(
    () => readOnlyView(),
    (target) => {
      if (target !== null) {
        viewRequest.set(null);
        lotLookup.view(target.auctionId, target.lotId);
      }
    },
  );
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
    const scope = catalogScope();
    return (scope.maxLevel !== null && scope.maxLevel !== "") || scope.includeUnleveled !== "true";
  };
  const retryCatalog = (): void => {
    lotsRefreshGeneration.update((generation) => Math.min(generation + 1, Number.MAX_SAFE_INTEGER));
    refreshPublicReads(refreshGeneration);
  };
  const catalog = (): JSX.Element => (
    <Switch fallback={null}>
      <Match when={currentFailed()}>
        <AsyncState
          kind="error"
          title={t("data-error")}
          detail={lotListErrorMessage(currentError() ?? "")}
          retryLabel={t("data-retry")}
          onRetry={retryCatalog}
        />
      </Match>
      <Match when={visibleSnapshot() === null && alignmentExhausted()}>
        <AsyncState
          kind="error"
          title={t("data-error")}
          detail={lotListErrorMessage("temporarily_unavailable")}
          retryLabel={t("data-retry")}
          onRetry={() => alignmentAttempt.set(null)}
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
              onPageClamp={(page) => {
                updatePublicQueryState(
                  queryStateSignal,
                  (query) => {
                    query.page = page;
                  },
                  "replace",
                );
              }}
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
