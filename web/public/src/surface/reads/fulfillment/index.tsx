// 公开履约进度页：按已结算场次逐件显示结算后的流程状态（流拍寄回、COD 付款与退回、异世界
// 交付、卖家货款与成交撤销）。
//
// 状态由服务端闭集给出，本页只把状态映射为文案与语气样式，不重建判定；付款期限与 COD 退回
// 次数只在服务端给出时显示。目录门禁与请求身份绑定沿用竞价日志页。重读失败时缓存层回退的
// 旧快照会连同其确认时刻一起提示，不把旧状态当作当前结果静默显示。
//
// 与 Rust 版有意不同：页面里没有世界下拉框，世界由页面顶部的世界按钮组（WorldTabs）切换并写入
// 地址栏，`world` 缺省表示全部世界；期次下拉框与场次标题合并在标题区，Rust 版是页面内的下拉框。

import type { AuctionSummaryResponse } from "@auction/contracts/dto/public/catalog/auction";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import type { Result } from "@auction/web-shared/result";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Match, Show, Switch, useContext } from "solid-js";

import type { FulfillmentRead } from "../../../api/fulfillment";
import { fetchFulfillmentPage, MAX_FULFILLMENT_PAGE_SIZE } from "../../../api/fulfillment";
import { PublicRefreshGenerationContext, refreshPublicReads } from "../../board/refresh";
import { WorldTabs, worldLabel } from "../../board/world";
import {
  LotDetailLookupContext,
  LotDetailLookupHost,
  useLotDetailLookup,
} from "../../catalog/detail/lookup";
import { formatAuctionTime } from "../../catalog/presentation";
import { useContentLoading } from "../../loading";
import { publicPageMatches, updatePublicPage, updatePublicPageSize } from "../../pagination";
import { usePublicPageBounds } from "../../pagination/bounds";
import {
  normalizedWorldQuery,
  usePublicQueryState,
  type WorldScope,
  worldScopeFromQuery,
} from "../../query";
import { usePageRouteGuard } from "../../query/route-page";
import { rustTrim } from "../../query/text";
import { useReadRoundDirectory } from "../bid-log/directory";
import { useLotFilterDraft } from "../bid-log/lots";
import { createScopeSelection, sameScopeSelection } from "../bid-log/scope-selection";
import { INVALID_READ_AUCTION_ERROR_ID, InvalidReadAuction } from "../invalid-auction";
import type {
  FulfillmentBodyState,
  FulfillmentProgressSnapshot,
  FulfillmentRequest,
} from "./model";
import {
  LOT_FILTER_ERROR_ID,
  LOT_FILTER_WRITE_DEBOUNCE_MILLIS,
  orderFulfillmentAuctionDirectory,
} from "./model";
import { FulfillmentTable } from "./table";

/** 全部世界范围先主世界、后异世界；目录读取与下拉分组共用这一顺序。 */
const WORLD_SCOPE_ORDER = ["main", "isekai"] as const;

interface FulfillmentAuctionGroup {
  readonly world: "main" | "isekai";
  readonly auctions: readonly AuctionSummaryResponse[];
}

/** 路由入口。 */
export function Fulfillment(): JSX.Element {
  // 世界/场次/页码/lot 筛选与地址栏双向同步，键与竞价日志共用。
  const queryState = usePublicQueryState();
  const queryStateSignal = queryState.state;
  const refreshGeneration = useContext(PublicRefreshGenerationContext);
  const routeStillOnPage = usePageRouteGuard("Fulfillment");
  // 跨世界页：缺省 `world` 表示全部世界，显式 `main`/`isekai` 限定单个世界。
  const worldScope = createValueMemo(
    (): WorldScope => worldScopeFromQuery("multi", normalizedWorldQuery(queryStateSignal().world)),
  );
  // 世界范围与本次选择的序号一起变化；目录资源只依赖这一个值，避免“新范围 + 旧代际”
  // 先启动一轮读取再被下一轮取消。
  const scopeSelection = createScopeSelection(worldScope);
  const pageNumber = createValueMemo(() => queryStateSignal().page);
  // 接口每页最多 100 件；纠偏、请求与分页器共用实际页长。
  const pageSize = createValueMemo(() =>
    Math.min(queryStateSignal().pageSize, MAX_FULFILLMENT_PAGE_SIZE),
  );
  const bounds = usePublicPageBounds(queryStateSignal, "Fulfillment", (query) =>
    query.auctionId === null ? [] : [["page", Math.min(query.pageSize, MAX_FULFILLMENT_PAGE_SIZE)]],
  );
  const lot = useLotFilterDraft(queryStateSignal, "/fulfillment", LOT_FILTER_WRITE_DEBOUNCE_MILLIS);
  const lotFilter = lot.appliedLot;
  const rounds = useReadRoundDirectory(
    "fulfillment",
    scopeSelection,
    refreshGeneration,
    queryStateSignal,
    routeStillOnPage,
    orderFulfillmentAuctionDirectory,
  );
  const auctionOptions = rounds.options;
  const selectedAuction = rounds.selected;
  const selectedAuctionId = () => selectedAuction()?.id ?? "";
  const auctionGroups = createValueMemo((): readonly FulfillmentAuctionGroup[] => {
    if (worldScope() !== "all") {
      return [];
    }
    const options = auctionOptions();
    const groups: FulfillmentAuctionGroup[] = [];
    for (const world of WORLD_SCOPE_ORDER) {
      const auctionsForWorld = options.filter((auction) => auction.world_id === world);
      if (auctionsForWorld.length > 0) {
        groups.push({ world, auctions: auctionsForWorld });
      }
    }
    return groups;
  });

  const readRequest = createValueMemo(() => {
    const auction = rounds.readable();
    if (auction === null || bounds.blocked("page") || !lot.appliedValid()) {
      return null;
    }
    const request: FulfillmentRequest = [
      auction.id,
      scopeSelection(),
      auction.world,
      rustTrim(lotFilter()) || null,
      pageNumber(),
      pageSize(),
    ];
    return { request, generation: auction.generation };
  });
  const progress = createTaskResource(
    readRequest,
    async (dependencies, context): Promise<FulfillmentProgressSnapshot | null> => {
      if (dependencies === null || !routeStillOnPage()) {
        return progress.peek() ?? null;
      }
      const [auctionId, , world, lotId, page, size] = dependencies.request;
      const result = await fetchFulfillmentPage(
        auctionId,
        world,
        lotId,
        page,
        size,
        context.signal,
      );
      return context.alive() ? { request: dependencies.request, result } : null;
    },
  );

  const currentResult = createValueMemo((): Result<FulfillmentRead | null, string> | null => {
    const snapshot = progress();
    if (
      snapshot === null ||
      snapshot === undefined ||
      !lot.appliedValid() ||
      bounds.blocked("page")
    ) {
      return null;
    }
    const [auctionId, selection, world, requestLot, page, size] = snapshot.request;
    const selected = selectedAuction();
    if (
      selected === null ||
      auctionId !== selectedAuctionId() ||
      !sameScopeSelection(selection, scopeSelection()) ||
      world !== selected.world_id ||
      (requestLot ?? "") !== rustTrim(lotFilter()) ||
      page !== pageNumber() ||
      size !== pageSize()
    ) {
      return null;
    }
    return snapshot.result;
  });

  // 目录或当前查询还没有结果时由顶栏提示加载，正文留空。
  const body = createValueMemo((): FulfillmentBodyState => {
    if (rounds.invalidAuction()) return { kind: "invalid-auction" };
    const progressResult = currentResult();
    if (bounds.failed("page") || rounds.failed() || progressResult?.ok === false) {
      return { kind: "error" };
    }
    if (rounds.correctionFailed()) {
      return { kind: "unselected" };
    }
    if (rounds.empty()) {
      return { kind: "empty-auctions" };
    }
    if (!lot.appliedValid()) {
      return { kind: "invalid-filter" };
    }
    if (
      progressResult?.ok === true &&
      progressResult.value !== null &&
      publicPageMatches(
        progressResult.value.page.page,
        progressResult.value.page.page_size,
        pageNumber(),
        pageSize(),
      )
    ) {
      return { kind: "table", read: progressResult.value };
    }
    return { kind: "loading" };
  });
  const tableRead = createValueMemo((): FulfillmentRead | null => {
    const current = body();
    return current.kind === "table" ? current.read : null;
  });
  const loading = createValueMemo(() => body().kind === "loading");
  useContentLoading(loading);

  const lookup = useLotDetailLookup();

  return (
    <LotDetailLookupContext value={lookup}>
      <section class="public-fulfillment" aria-label={t("page-fulfillment-title")}>
        {/* 世界切换：页面顶部的按钮组（WorldTabs，Solid 版专属）；Rust 版用页面内的下拉框。 */}
        <div class="public-auction-group-toolbar public-world-toolbar">
          <WorldTabs />
        </div>
        <div class="public-fulfillment__heading">
          <div class="public-fulfillment__round">
            <select
              id="public-fulfillment-round"
              class="public-fulfillment__round-select"
              name="auction"
              autocomplete="off"
              aria-label={t("fulfillment-round")}
              aria-invalid={rounds.invalidAuction() ? "true" : undefined}
              aria-describedby={rounds.invalidAuction() ? INVALID_READ_AUCTION_ERROR_ID : undefined}
              value={selectedAuctionId()}
              disabled={auctionOptions().length === 0}
              onFocus={rounds.loadOptions}
              onPointerDown={rounds.loadOptions}
              onChange={(event) => rounds.choose(event.currentTarget)}
            >
              <Show when={selectedAuctionId() === "" && auctionOptions().length > 0}>
                <option value="" disabled selected>
                  {t("round-select-placeholder")}
                </option>
              </Show>
              <Show
                when={worldScope() === "all"}
                fallback={
                  <For each={auctionOptions()} keyed={(auction) => auction.id}>
                    {(auction) => (
                      <option value={auction().id} selected={selectedAuctionId() === auction().id}>
                        {t("fulfillment-round-option", { number: auction().sequence })}
                      </option>
                    )}
                  </For>
                }
              >
                <For each={auctionGroups()} keyed={(group) => group.world}>
                  {(group) => (
                    <optgroup label={worldLabel(group().world)}>
                      <For each={group().auctions} keyed={(auction) => auction.id}>
                        {(auction) => (
                          <option
                            value={auction().id}
                            selected={selectedAuctionId() === auction().id}
                          >
                            {t("fulfillment-round-option", { number: auction().sequence })}
                          </option>
                        )}
                      </For>
                    </optgroup>
                  )}
                </For>
              </Show>
            </select>
            <Show when={rounds.optionsFailed()}>
              <button type="button" onClick={() => refreshPublicReads(refreshGeneration)}>
                {t("data-retry")}
              </button>
            </Show>
            <Show when={selectedAuction()}>
              {(auction) => (
                <>
                  <span class="public-fulfillment__round-detail">
                    {t("fulfillment-world", { world: worldLabel(auction().world_id) })}
                  </span>
                  <Show when={auction().settled_at !== null && auction().settled_at !== undefined}>
                    <span class="public-fulfillment__round-detail">
                      {t("fulfillment-settled-at", {
                        date: formatAuctionTime(auction().settled_at ?? ""),
                      })}
                    </span>
                  </Show>
                </>
              )}
            </Show>
          </div>
          <div class="public-fulfillment__toolbar">
            <label for="public-fulfillment-filter">{t("fulfillment-filter-label")}</label>
            <input
              id="public-fulfillment-filter"
              name="lot"
              autocomplete="off"
              type="search"
              enterkeyhint="search"
              autocapitalize="none"
              spellcheck="false"
              value={lot.draft()}
              placeholder={t("fulfillment-filter-placeholder")}
              aria-invalid={lot.interacted() && lot.draftError() !== null ? "true" : "false"}
              aria-describedby={lot.draftError() === null ? undefined : LOT_FILTER_ERROR_ID}
              onInput={(event) => lot.input(event.currentTarget.value)}
              onBlur={lot.blur}
            />
            <Show when={lot.draft() !== "" || !lot.appliedValid()}>
              <button type="button" onClick={lot.clear}>
                {t("fulfillment-filter-clear")}
              </button>
            </Show>
            <Show when={lot.draftError() !== null}>
              <p
                id={LOT_FILTER_ERROR_ID}
                class="public-fulfillment__toolbar-error"
                role="status"
                aria-live="polite"
              >
                {lot.draftError() === "duplicate"
                  ? t("filter-error-duplicate")
                  : t("fulfillment-filter-invalid")}
                <Show when={lot.errorOriginal() !== null}>
                  <bdi> {lot.errorOriginal()}</bdi>
                </Show>
              </p>
            </Show>
          </div>
        </div>
        <Switch fallback={null}>
          <Match when={body().kind === "invalid-auction"}>
            <InvalidReadAuction onRecover={rounds.clearSelection} />
          </Match>
          <Match when={body().kind === "error"}>
            <div class="public-auction-empty" role="alert">
              <p class="public-auction-empty__title">{t("data-error")}</p>
              {/* 与看板错误重试同一入口：先使读缓存过期再推进代号，TTL 内的条目也会回到服务端。 */}
              <button
                class="public-auction-empty__retry"
                type="button"
                onClick={() =>
                  bounds.failed("page") ? bounds.retry() : refreshPublicReads(refreshGeneration)
                }
              >
                {t("data-retry")}
              </button>
            </div>
          </Match>
          <Match when={body().kind === "empty-auctions"}>
            <div class="public-auction-empty" role="status">
              <p class="public-auction-empty__title">{t("fulfillment-empty-auctions")}</p>
            </div>
          </Match>
          <Match when={tableRead()}>
            {(read) => (
              <>
                {/* 重读遇到网络失败或服务暂时繁忙时，缓存层回退旧快照：标明它的确认时刻。
                            播报区域随表格常驻、只更新内容，读屏才会可靠朗读首次出现的提示。 */}
                <div
                  class="public-fulfillment__stale-region"
                  role="status"
                  aria-live="polite"
                  aria-atomic="true"
                >
                  <Show when={read().staleSince !== null}>
                    <p class="public-fulfillment__stale">
                      {t("fulfillment-stale", {
                        time: formatAuctionTime(read().staleSince ?? ""),
                      })}
                    </p>
                  </Show>
                </div>
                <FulfillmentTable
                  page={read().page}
                  currentPage={pageNumber()}
                  lotFilter={lotFilter()}
                  onPageChange={(nextPage) => updatePublicPage(queryStateSignal, nextPage, "push")}
                  onPageClamp={(nextPage) =>
                    routeStillOnPage() && updatePublicPage(queryStateSignal, nextPage, "replace")
                  }
                  onPageSizeChange={(size) => updatePublicPageSize(queryStateSignal, size)}
                />
              </>
            )}
          </Match>
        </Switch>
        <Show when={lookup.failed()}>
          <p class="action-feedback action-feedback--error" role="alert">
            {t("data-error")}
          </p>
        </Show>
        {/* 抽屉宿主在进度读取分支之外：翻页、筛选或后台重读替换表格时，已打开的详情不被卸载。 */}
        <LotDetailLookupHost lookup={lookup} />
      </section>
    </LotDetailLookupContext>
  );
}
