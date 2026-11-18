// 公开竞价日志页。Solid 版与 Rust 版有意不同：世界由页面顶部的世界按钮组写入查询状态
// （Rust 版是页面内的下拉框）；本页作为跨世界页，缺省范围为全部世界。

import type { AuctionSummaryResponse } from "@auction/contracts/dto/public/catalog/auction";
import type { PublicBidLogListResponse } from "@auction/contracts/dto/public/catalog/history";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import type { Result } from "@auction/web-shared/result";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { createEffect, For, Match, Show, Switch, useContext } from "solid-js";

import { fetchBidLogPage } from "../../../api/queries/bid-log";
import {
  PublicRefreshGenerationContext,
  refreshPublicReads,
  useBidRefresh,
} from "../../board/refresh";
import { WorldTabs, worldLabel } from "../../board/world";
import {
  LotDetailLookupContext,
  LotDetailLookupHost,
  useLotDetailLookup,
} from "../../catalog/detail/lookup";
import { useContentLoading } from "../../loading";
import { publicPageMatches, updatePublicPage, updatePublicPageSize } from "../../pagination";
import { usePublicPageBounds } from "../../pagination/bounds";
import { updatePublicQueryPairs } from "../../pagination/location";
import {
  normalizedWorldQuery,
  usePublicQueryState,
  type WorldScope,
  worldScopeFromQuery,
} from "../../query";
import { usePageRouteGuard } from "../../query/route-page";
import { rustTrim } from "../../query/text";
import { useReadRoundDirectory } from "./directory";
import { applyLotFilter, resetLotDraft, useLotFilterDraft, validLotFilter } from "./lots";
import { createScopeSelection, type ScopeSelection, sameScopeSelection } from "./scope-selection";
import { BidLogRow, BidLogTable, bidLogStatusClass, bidLogStatusLabel } from "./table";

export {
  applyLotFilter,
  BidLogRow,
  BidLogTable,
  bidLogStatusClass,
  bidLogStatusLabel,
  resetLotDraft,
  validLotFilter,
};

/** lot 筛选生效的防抖窗口：输入停顿后才应用筛选并无历史记录地写回地址栏，连续输入只发一次
 * 记录请求。 */
const LOT_FILTER_WRITE_DEBOUNCE_MILLIS = 300;
/** lot 筛选格式提示的元素 ID，供输入框 `aria-describedby` 引用。 */
const LOT_FILTER_ERROR_ID = "public-auction-bid-log-filter-error";

/** 一次竞价日志读取所对应的查询；刷新完成时用它与当前查询核对，避免迟到结果覆盖新页面。 */
type BidLogRequested = readonly [
  auctionId: string,
  selection: ScopeSelection,
  worldId: string,
  lotId: string | null,
  page: number,
  pageSize: number,
];

interface BidLogSnapshot {
  readonly requested: BidLogRequested;
  readonly result: Result<PublicBidLogListResponse | null, string>;
}

interface AuctionOptionGroup {
  readonly world: string;
  readonly auctions: AuctionSummaryResponse[];
}

type BidLogBody =
  | { readonly kind: "error" }
  | { readonly kind: "empty" }
  | { readonly kind: "invalid" }
  | { readonly kind: "unselected" }
  | { readonly kind: "loading" }
  | { readonly kind: "page"; readonly page: PublicBidLogListResponse };

/** 场次下拉框中的一项；原生选中态跟随查询状态，目录刷新后不会按下标错配到另一场。 */
function BidLogRoundOption(props: {
  readonly auction: AuctionSummaryResponse;
  readonly selected: boolean;
}): JSX.Element {
  return (
    <option value={props.auction.id} selected={props.selected}>
      {/* 选项只显示期数；场次状态由下拉框旁边的状态标签给出。 */}
      {t("bidlog-round-short", { number: props.auction.sequence })}
    </option>
  );
}

/** 公开竞价日志页。 */
export function BidLog(): JSX.Element {
  const refreshGeneration = useContext(PublicRefreshGenerationContext);
  const routeStillOnPage = usePageRouteGuard("BidLog");
  // 世界/拍卖/页码/lot 筛选与地址栏双向同步。
  const queryState = usePublicQueryState();
  const queryStateSignal = queryState.state;
  // 跨世界页：缺省或非法 `world` 表示全部世界，显式值限定单个世界。
  const worldScope = createValueMemo(
    (): WorldScope => worldScopeFromQuery("multi", normalizedWorldQuery(queryStateSignal().world)),
  );
  // 世界范围与本次选择的序号一起变化；目录资源只依赖这一个值，避免“新范围 + 旧代际”
  // 先启动一轮读取再被下一轮取消。目录纠偏、记录读取门禁与实时重读都只接受这次选择确认的目录。
  const scopeSelection = createScopeSelection(worldScope);
  const pageNumber = createValueMemo(() => queryStateSignal().page);
  const pageSize = createValueMemo(() => queryStateSignal().pageSize);
  const lot = useLotFilterDraft(queryStateSignal, "/bidlog", LOT_FILTER_WRITE_DEBOUNCE_MILLIS);
  // 缺省场次会和第一页一起写址，先纠偏页码会生成另一次无意义的 replace。
  const bounds = usePublicPageBounds(queryStateSignal, "BidLog", (query) =>
    query.auctionId === null ? [] : [["page", query.pageSize]],
  );
  // 已有场次时只替换迁移期的分页别名，保留仍需修正的原始筛选。
  createEffect(
    () =>
      [
        queryStateSignal().auctionId !== null && queryStateSignal().bidLogPageAlias,
        pageNumber(),
        bounds.blocked("page"),
      ] as const,
    ([alias, page, blocked]) => {
      if (alias && !blocked && routeStillOnPage()) {
        updatePublicQueryPairs(
          queryStateSignal,
          ["page", "bidlog_page"],
          page === 1 ? [] : [["page", String(page)]],
          "replace",
        );
      }
    },
  );
  const rounds = useReadRoundDirectory(
    "bidlog",
    scopeSelection,
    refreshGeneration,
    queryStateSignal,
    routeStillOnPage,
  );
  const auctionOptions = rounds.options;
  const selectedAuction = rounds.selected;
  const selectedAuctionId = () => selectedAuction()?.id ?? "";
  // 全部世界时按世界分组；目录已按主世界、异世界排序，因此相同世界始终相邻。
  const auctionGroups = createValueMemo((): readonly AuctionOptionGroup[] => {
    const groups: AuctionOptionGroup[] = [];
    for (const auction of auctionOptions()) {
      const previous = groups[groups.length - 1];
      if (previous !== undefined && previous.world === auction.world_id) {
        previous.auctions.push(auction);
      } else {
        groups.push({ world: auction.world_id, auctions: [auction] });
      }
    }
    return groups;
  });

  const readRequest = createValueMemo(() => {
    const auction = rounds.readable();
    if (auction === null || bounds.blocked("page") || !lot.appliedValid()) {
      return null;
    }
    const requested: BidLogRequested = [
      auction.id,
      scopeSelection(),
      auction.world,
      rustTrim(lot.appliedLot()) || null,
      pageNumber(),
      pageSize(),
    ];
    return { requested, generation: auction.generation };
  });
  const bidLog = createTaskResource(
    readRequest,
    async (dependencies, context): Promise<BidLogSnapshot | null> => {
      if (dependencies === null || !routeStillOnPage()) {
        return bidLog.peek() ?? null;
      }
      const [auctionId, , world, lotId, page, size] = dependencies.requested;
      const result = await fetchBidLogPage(auctionId, page, world, lotId, size, context.signal);
      return context.alive() ? { requested: dependencies.requested, result } : null;
    },
  );
  useBidRefresh(bidLog, () => readRequest() !== null);
  const lookup = useLotDetailLookup();
  const currentPage = pageNumber;
  const currentLotFilter = lot.appliedLot;
  // 同查询后台刷新保留完成快照；目录确认场次已不可见后，迟到记录也不能恢复旧行和分页。
  const currentResult = createValueMemo(() => {
    const snapshot = bidLog();
    const selected = selectedAuction();
    if (
      snapshot === null ||
      snapshot === undefined ||
      selected === null ||
      !lot.appliedValid() ||
      bounds.blocked("page")
    ) {
      return null;
    }
    const [auctionId, selection, worldId, lotId, page, size] = snapshot.requested;
    if (
      auctionId === selected.id &&
      sameScopeSelection(selection, scopeSelection()) &&
      worldId === selected.world_id &&
      (lotId ?? "") === rustTrim(currentLotFilter()) &&
      page === currentPage() &&
      size === pageSize()
    ) {
      return snapshot.result;
    }
    return null;
  });
  // 目录或当前查询的记录还没有结果时由顶栏提示加载，正文留空。
  const bodyState = createValueMemo((): BidLogBody => {
    const resultNow = currentResult();
    if (bounds.failed("page") || rounds.failed() || resultNow?.ok === false) {
      return { kind: "error" };
    }
    if (rounds.correctionFailed()) {
      return { kind: "unselected" };
    }
    if (rounds.empty()) {
      return { kind: "empty" };
    }
    if (!lot.appliedValid()) {
      return { kind: "invalid" };
    }
    if (
      resultNow?.ok === true &&
      resultNow.value !== null &&
      publicPageMatches(resultNow.value.page, resultNow.value.page_size, currentPage(), pageSize())
    ) {
      return { kind: "page", page: resultNow.value };
    }
    return { kind: "loading" };
  });
  const loadedPage = createValueMemo(() => {
    const state = bodyState();
    return state.kind === "page" ? state.page : null;
  });
  useContentLoading(() => bodyState().kind === "loading");

  return (
    <LotDetailLookupContext value={lookup}>
      <section class="public-auction-bid-log-page" aria-label={t("page-bidlog-title")}>
        {/* 世界切换：页面顶部的按钮组（WorldTabs，Solid 版专属）；Rust 版用页面内的下拉框。 */}
        <div class="public-auction-group-toolbar public-world-toolbar">
          <WorldTabs />
        </div>
        <div class="public-auction-bid-log-heading">
          <div class="public-auction-bid-log-round-header">
            <select
              id="public-auction-bid-log-round"
              class="public-auction-bid-log-round-select"
              name="auction"
              autocomplete="off"
              aria-label={t("bidlog-round")}
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
                      <BidLogRoundOption
                        auction={auction()}
                        selected={selectedAuctionId() === auction().id}
                      />
                    )}
                  </For>
                }
              >
                <For each={auctionGroups()} keyed={(group) => group.world}>
                  {(group) => (
                    <optgroup label={worldLabel(group().world)}>
                      <For each={group().auctions} keyed={(auction) => auction.id}>
                        {(auction) => (
                          <BidLogRoundOption
                            auction={auction()}
                            selected={selectedAuctionId() === auction().id}
                          />
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
                  <span class="public-auction-bid-log-round-world">
                    {t("bidlog-world", { world: worldLabel(auction().world_id) })}
                  </span>
                  <span class={bidLogStatusClass(auction().status)}>
                    {bidLogStatusLabel(auction().status)}
                  </span>
                </>
              )}
            </Show>
          </div>
          <div class="public-auction-bid-log-toolbar">
            <label for="public-auction-bid-log-filter">{t("bidlog-filter-label")}</label>
            <input
              id="public-auction-bid-log-filter"
              name="lot"
              autocomplete="off"
              type="search"
              enterkeyhint="search"
              autocapitalize="none"
              spellcheck="false"
              value={lot.draft()}
              placeholder={t("bidlog-filter-placeholder")}
              aria-invalid={lot.interacted() && lot.draftError() !== null ? "true" : "false"}
              aria-describedby={lot.draftError() === null ? undefined : LOT_FILTER_ERROR_ID}
              onInput={(event) => lot.input(event.currentTarget.value)}
              onBlur={lot.blur}
            />
            <Show when={lot.draft() !== "" || !lot.appliedValid()}>
              <button type="button" onClick={lot.clear}>
                {t("bidlog-filter-clear")}
              </button>
            </Show>
            <Show when={lot.draftError() !== null}>
              <p
                id={LOT_FILTER_ERROR_ID}
                class="public-auction-bid-log-toolbar__error"
                role="status"
                aria-live="polite"
              >
                {lot.draftError() === "duplicate"
                  ? t("filter-error-duplicate")
                  : t("bidlog-filter-invalid")}
                <Show when={lot.errorOriginal() !== null}>
                  <bdi> {lot.errorOriginal()}</bdi>
                </Show>
              </p>
            </Show>
          </div>
        </div>
        <Switch>
          <Match when={bodyState().kind === "error"}>
            <div class="public-auction-empty" role="alert">
              <p class="public-auction-empty__title">{t("data-error")}</p>
              <button
                class="public-auction-empty__retry"
                type="button"
                onClick={() => {
                  if (bounds.failed("page")) {
                    bounds.retry();
                  } else {
                    refreshGeneration.set(
                      Math.min(refreshGeneration.peek() + 1, Number.MAX_SAFE_INTEGER),
                    );
                  }
                }}
              >
                {t("data-retry")}
              </button>
            </div>
          </Match>
          <Match when={bodyState().kind === "empty"}>
            <div class="public-auction-empty" role="status">
              <p class="public-auction-empty__title">{t("data-empty")}</p>
            </div>
          </Match>
          <Match when={loadedPage()}>
            {(page) => (
              <BidLogTable
                page={page()}
                currentPage={currentPage()}
                world={selectedAuction()?.world_id ?? null}
                lotFilter={currentLotFilter()}
                onPageChange={(nextPage) => updatePublicPage(queryStateSignal, nextPage, "push")}
                onPageClamp={(nextPage) =>
                  routeStillOnPage() && updatePublicPage(queryStateSignal, nextPage, "replace")
                }
                onPageSizeChange={(size) => updatePublicPageSize(queryStateSignal, size)}
              />
            )}
          </Match>
        </Switch>
        <Show when={lookup.failed()}>
          <p class="action-feedback action-feedback--error" role="alert">
            {t("data-error")}
          </p>
        </Show>
        {/* 抽屉宿主在记录读取分支之外：翻页、筛选或后台重读替换表格时，已打开的详情不被卸载。 */}
        <LotDetailLookupHost lookup={lookup} />
      </section>
    </LotDetailLookupContext>
  );
}
