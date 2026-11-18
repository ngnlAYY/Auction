// Public 价格历史页面及其筛选、值规范化、结果表格和详情查找宿主。
// 世界是本页“筛选条件”面板里的一项，点「应用」才写入地址栏，缺省为全部世界；其他跨世界页仍用
// 页面顶部的世界按钮组。Rust 版把世界下拉框放在表单的主筛选行。

import type { PublicPriceHistoryListResponse } from "@auction/contracts/dto/public/catalog/history";
import { createCell } from "@auction/web-shared/reactive/cell";
import { structuralEquals } from "@auction/web-shared/reactive/equality";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import type { Result } from "@auction/web-shared/result";
import { compareText } from "@auction/web-shared/text";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { createEffect, For, Match, Show, Switch, useContext } from "solid-js";

import type { PriceHistoryQuery } from "../../../api/model";
import { fetchPriceHistory, type PriceHistoryFetchError } from "../../../api/queries/price-history";
import { PublicRefreshGenerationContext } from "../../board/refresh";
import {
  LotDetailLookupContext,
  LotDetailLookupHost,
  useLotDetailLookup,
} from "../../catalog/detail/lookup";
import { useContentLoading } from "../../loading";
import { publicPageMatches, updatePublicPageSize } from "../../pagination";
import {
  type PublicBoardQuery,
  readPublicQuery,
  updatePublicQueryState,
  usePublicQueryState,
} from "../../query";
import { WorldSwitchContext } from "../../shell/contexts";
import { PriceHistoryFilters } from "./filters/components";
import type { HistoryFilterError, HistoryPriceUnit } from "./filters/model";
import { historyDateValue, historyNameValue, historyPriceValue } from "./filters/validation";
import { HistoryResultControls } from "./presentation";
import { PriceHistoryTable } from "./table";
import {
  historyActiveFilterCount,
  historyApplyChangesQuery,
  historyDateParts,
  historyFormFilters,
  historyHasRequestCondition,
  historyPriceParts,
  historyPriceRangeReversed,
  historyQueryError,
  historyResetChangesQuery,
  historyWorldDraftForApply,
  normalizeHistoryQuery,
  optionalFilterValue,
} from "./values";

interface HistorySnapshot {
  readonly query: PriceHistoryQuery;
  readonly pageSize: number;
  readonly result: Result<PublicPriceHistoryListResponse, PriceHistoryFetchError>;
}

/** 已取回的成交页；只有查询身份与当前页码都吻合时才装配到页面。 */
interface HistoryTableEntry {
  readonly key: string;
  readonly page: PublicPriceHistoryListResponse;
}

/** 价格历史页面。 */
export function History(): JSX.Element {
  const refreshGeneration = useContext(PublicRefreshGenerationContext);
  // 世界选项来自外壳控制器；本页只把选项交给筛选表单，按钮改变的是可提交的草稿。
  const worldSwitch = useContext(WorldSwitchContext);
  // 成交行的查看按钮在页内打开只读的拍品详情抽屉；读取任务归页面宿主，不随翻页卸载。
  const lookup = useLotDetailLookup();
  // 筛选与页码与地址栏双向同步。
  const queryState = usePublicQueryState();
  const queryStateSignal = queryState.state;
  const pageSize = createValueMemo(() => queryStateSignal().pageSize);
  // 日期沿用既有恢复规则；非法名称/金额保留为字段错误并阻止查询，不改写地址栏。
  const appliedQuery = createValueMemo(() => historyQueryFromBoard(queryStateSignal()));
  // 草稿只随实际筛选条件恢复；世界也是表单字段，翻页与排序不应覆盖尚未提交的输入或字段错误。
  const appliedFilters = createValueMemo(() => historyFormFilters(appliedQuery()));
  const worldOptions = createValueMemo(() => worldSwitch.options());
  // 筛选草稿以已应用查询为初值，筛选条件变化（含前进/后退恢复）时经 effect 同步。
  const initial = historyQueryFromBoard(queryStateSignal.peek());
  const worldFilter = createCell(initial.world ?? "all");
  const nameFilter = createCell(initial.name ?? "");
  const [fromMonth, fromYear] = historyDateParts(initial.from);
  const [toMonth, toYear] = historyDateParts(initial.to);
  const [minimumPrice, minimumPriceUnit] = historyPriceParts(initial.minPrice);
  const [maximumPrice, maximumPriceUnit] = historyPriceParts(initial.maxPrice);
  const fromMonthFilter = createCell(fromMonth);
  const fromYearFilter = createCell(fromYear);
  const toMonthFilter = createCell(toMonth);
  const toYearFilter = createCell(toYear);
  const minimumPriceFilter = createCell(minimumPrice);
  const minimumPriceUnitFilter = createCell(minimumPriceUnit);
  const maximumPriceFilter = createCell(maximumPrice);
  const maximumPriceUnitFilter = createCell(maximumPriceUnit);
  const buyerFilter = createCell(initial.buyer ?? "");
  const sellerFilter = createCell(initial.seller ?? "");
  const initialError = historyQueryError(initial);
  const settingsOpen = createCell(
    initialError === "minimum_price_c" ||
      initialError === "maximum_price_c" ||
      initialError === "price_range",
  );
  const filterError = createCell(initialError);

  createEffect(appliedFilters, (applied) => {
    worldFilter.set(applied.world ?? "all");
    nameFilter.set(applied.name ?? "");
    const [nextFromMonth, nextFromYear] = historyDateParts(applied.from);
    fromMonthFilter.set(nextFromMonth);
    fromYearFilter.set(nextFromYear);
    const [nextToMonth, nextToYear] = historyDateParts(applied.to);
    toMonthFilter.set(nextToMonth);
    toYearFilter.set(nextToYear);
    const [nextMinimumPrice, nextMinimumPriceUnit] = historyPriceParts(applied.minPrice);
    minimumPriceFilter.set(nextMinimumPrice);
    minimumPriceUnitFilter.set(nextMinimumPriceUnit);
    const [nextMaximumPrice, nextMaximumPriceUnit] = historyPriceParts(applied.maxPrice);
    maximumPriceFilter.set(nextMaximumPrice);
    maximumPriceUnitFilter.set(nextMaximumPriceUnit);
    buyerFilter.set(applied.buyer ?? "");
    sellerFilter.set(applied.seller ?? "");
    const error = historyQueryError(applied);
    if (error === "minimum_price_c" || error === "maximum_price_c" || error === "price_range") {
      settingsOpen.set(true);
    }
    filterError.set(error);
  });

  // 目录可能在用户选过世界后收窄；已应用的关闭世界仍可作为深链值，其余失效草稿回到已应用值。
  createEffect(worldOptions, (options) => {
    const appliedWorld = historyQueryFromBoard(queryStateSignal.peek()).world;
    const currentWorld = worldFilter.peek();
    const nextWorld = historyWorldDraftForApply(currentWorld, options, appliedWorld);
    if (nextWorld !== currentWorld) {
      worldFilter.set(nextWorld);
    }
  });

  const activeFilterCount = createValueMemo(() => historyActiveFilterCount(appliedQuery()));
  const hasSearch = createValueMemo(
    () => historyHasRequestCondition(appliedQuery()) && historyQueryError(appliedQuery()) === null,
  );
  const historyDependencies = createValueMemo(() => {
    return {
      query: appliedQuery(),
      pageSize: pageSize(),
      // 刷新代数参与依赖身份：同一查询也必须因权威重读请求重新发起读取。
      refreshGeneration: refreshGeneration(),
    };
  });
  const history = createTaskResource(historyDependencies, async (dependencies, context) => {
    // 没有已应用的筛选条件时只展示搜索入口，不发起查询。
    if (
      !historyHasRequestCondition(dependencies.query) ||
      historyQueryError(dependencies.query) !== null
    ) {
      return null;
    }
    const result = await fetchPriceHistory(
      dependencies.query,
      dependencies.pageSize,
      context.signal,
    );
    if (!context.alive()) {
      return null;
    }
    return {
      query: dependencies.query,
      pageSize: dependencies.pageSize,
      result,
    } satisfies HistorySnapshot;
  });
  // Resource 会保留旧结果；重置或切换查询后不能把前一次装备列表装配到当前搜索条件。
  const historyResult = createValueMemo(() => {
    const snapshot = history();
    if (snapshot === null || snapshot === undefined) {
      return undefined;
    }
    return structuralEquals(snapshot.query, appliedQuery()) && snapshot.pageSize === pageSize()
      ? snapshot.result
      : undefined;
  });
  const failed = createValueMemo(() => {
    const result = historyResult();
    return result !== undefined && !result.ok;
  });
  const loadedPage = createValueMemo(() =>
    historyPageOf(historyResult(), appliedQuery().page, pageSize()),
  );
  // 查询身份同时隔离子分组的折叠状态；同查询后台重读保留当前展开选择。
  const tableScopeKey = createValueMemo(
    () => `table:${historyQueryKey(appliedQuery())}:${pageSize()}`,
  );
  const tableEntries = createValueMemo((): readonly HistoryTableEntry[] => {
    const page = loadedPage();
    return page === null ? [] : [{ key: tableScopeKey(), page }];
  });
  // 已提交的搜索还没有结果（含等待页码纠偏后的重读）时由顶栏提示加载，正文留空。
  const contentLoading = createValueMemo(() => {
    if (!hasSearch()) {
      return false;
    }
    const result = historyResult();
    if (result === undefined) {
      return true;
    }
    if (!result.ok) {
      return false;
    }
    return loadedPage() === null;
  });
  useContentLoading(contentLoading);

  const applyFilters = () => {
    const name = historyNameValue(nameFilter());
    if (!name.ok) {
      filterError.set(name.error);
      return;
    }
    // 每次提交都以地址栏最新的已应用查询为锚点，路由恢复和前一次提交不会留下旧日期边界。
    const applied = historyQueryFromBoard(readPublicQuery());
    let validationError: HistoryFilterError | null = null;
    let from: string | null;
    const fromValue = historyDateValue(fromMonthFilter(), fromYearFilter(), false, applied.from);
    if (fromValue.ok) {
      from = fromValue.value;
    } else {
      validationError =
        fromValue.error === "incomplete" ? "from_date_incomplete" : "from_date_invalid";
      from = null;
    }
    let to: string | null;
    const toValue = historyDateValue(toMonthFilter(), toYearFilter(), true, applied.to);
    if (toValue.ok) {
      to = toValue.value;
    } else {
      if (validationError === null) {
        validationError = toValue.error === "incomplete" ? "to_date_incomplete" : "to_date_invalid";
      }
      to = null;
    }
    if (validationError === null && from !== null && to !== null && compareText(from, to) > 0) {
      validationError = "date_range";
    }
    let minimum: string | null;
    const minimumValue = historyPriceValue(minimumPriceFilter(), minimumPriceUnitFilter());
    if (minimumValue.ok) {
      minimum = minimumValue.value;
    } else {
      if (validationError === null) {
        validationError = priceFilterError(true, minimumPriceUnitFilter());
      }
      minimum = null;
    }
    let maximum: string | null;
    const maximumValue = historyPriceValue(maximumPriceFilter(), maximumPriceUnitFilter());
    if (maximumValue.ok) {
      maximum = maximumValue.value;
    } else {
      if (validationError === null) {
        validationError = priceFilterError(false, maximumPriceUnitFilter());
      }
      maximum = null;
    }
    if (validationError === null && historyPriceRangeReversed(minimum, maximum)) {
      validationError = "price_range";
    }
    if (validationError !== null) {
      filterError.set(validationError);
      settingsOpen.set(true);
      return;
    }
    // 目录收窄后，未应用的消失世界不能继续提交；回退值仍以地址栏已经应用的世界为准。
    const draftWorld = worldFilter.peek();
    const world = historyWorldDraftForApply(draftWorld, worldOptions(), applied.world);
    if (world !== draftWorld) {
      worldFilter.set(world);
    }
    const candidate: PriceHistoryQuery = {
      world: world === "all" ? null : world,
      name: name.value,
      from,
      to,
      minPrice: minimum,
      maxPrice: maximum,
      buyer: optionalFilterValue(buyerFilter()),
      seller: optionalFilterValue(sellerFilter()),
      sort: applied.sort,
      page: 1,
    };
    filterError.set(null);
    // 第 1 页上筛选与已应用值等价时不新增历史条目；在其他页仍要回到第 1 页。
    if (!historyApplyChangesQuery(applied, candidate)) {
      return;
    }
    updatePublicQueryState(
      queryStateSignal,
      (query: PublicBoardQuery) => {
        query.world = candidate.world;
        query.historyName = candidate.name;
        query.historyFrom = candidate.from;
        query.historyTo = candidate.to;
        query.historyMinPrice = candidate.minPrice;
        query.historyMaxPrice = candidate.maxPrice;
        query.historyBuyer = candidate.buyer;
        query.historySeller = candidate.seller;
        query.page = candidate.page;
      },
      "push",
    );
  };

  const resetFilters = () => {
    worldFilter.set("all");
    nameFilter.set("");
    fromMonthFilter.set("");
    fromYearFilter.set("");
    toMonthFilter.set("");
    toYearFilter.set("");
    minimumPriceFilter.set("");
    minimumPriceUnitFilter.set("K");
    maximumPriceFilter.set("");
    maximumPriceUnitFilter.set("K");
    buyerFilter.set("");
    sellerFilter.set("");
    settingsOpen.set(false);
    filterError.set(null);
    // 已经处于清空后的状态时不新增一条相同的历史记录；页长不参与判断。
    if (!historyResetChangesQuery(readPublicQuery())) {
      return;
    }
    updatePublicQueryState(
      queryStateSignal,
      (query: PublicBoardQuery) => {
        query.world = null;
        query.historyName = null;
        query.historyFrom = null;
        query.historyTo = null;
        query.historyMinPrice = null;
        query.historyMaxPrice = null;
        query.historyBuyer = null;
        query.historySeller = null;
        query.historySort = "closed_desc";
        query.historyGroup = "equipment";
        query.page = 1;
      },
      "push",
    );
  };

  return (
    <LotDetailLookupContext value={lookup}>
      <section class="public-history-page" aria-label={t("page-history-title")}>
        <div class="public-history-search-region">
          <PriceHistoryFilters
            world={worldFilter()}
            worldOptions={worldOptions()}
            name={nameFilter()}
            fromMonth={fromMonthFilter()}
            fromYear={fromYearFilter()}
            toMonth={toMonthFilter()}
            toYear={toYearFilter()}
            minimumPrice={minimumPriceFilter()}
            minimumPriceUnit={minimumPriceUnitFilter()}
            maximumPrice={maximumPriceFilter()}
            maximumPriceUnit={maximumPriceUnitFilter()}
            buyer={buyerFilter()}
            seller={sellerFilter()}
            activeFilterCount={activeFilterCount()}
            settingsOpen={settingsOpen()}
            filterError={filterError()}
            setWorld={worldFilter.set}
            setName={nameFilter.set}
            setFromMonth={fromMonthFilter.set}
            setFromYear={fromYearFilter.set}
            setToMonth={toMonthFilter.set}
            setToYear={toYearFilter.set}
            setMinimumPrice={minimumPriceFilter.set}
            setMinimumPriceUnit={minimumPriceUnitFilter.set}
            setMaximumPrice={maximumPriceFilter.set}
            setMaximumPriceUnit={maximumPriceUnitFilter.set}
            setBuyer={buyerFilter.set}
            setSeller={sellerFilter.set}
            setSettingsOpen={settingsOpen.set}
            setFilterError={filterError.set}
            onApply={applyFilters}
            onReset={resetFilters}
          />
        </div>
        <Show when={hasSearch()}>
          <div class="public-history-output">
            <HistoryResultControls
              sort={appliedQuery().sort}
              grouping={queryStateSignal().historyGroup}
              onSortChange={(sort) => {
                updatePublicQueryState(
                  queryStateSignal,
                  (query: PublicBoardQuery) => {
                    query.historySort = sort;
                    query.page = 1;
                  },
                  "push",
                );
              }}
              onGroupChange={(group) => {
                // 分组只重排已取回的本页，不能改变 API 查询或重新读取相同成交集合。
                updatePublicQueryState(
                  queryStateSignal,
                  (query: PublicBoardQuery) => {
                    query.historyGroup = group;
                  },
                  "push",
                );
              }}
            />
            <Switch>
              <Match when={historyResult() === undefined}>{null}</Match>
              <Match when={failed()}>
                <AsyncState
                  kind="error"
                  title={t("page-history-title")}
                  detail={t("data-error")}
                  retryLabel={t("data-retry")}
                  onRetry={() => {
                    refreshGeneration.set(
                      Math.min(refreshGeneration.peek() + 1, Number.MAX_SAFE_INTEGER),
                    );
                  }}
                />
              </Match>
              <Match when={loadedPage()}>
                <For each={tableEntries()} keyed={(entry) => entry.key}>
                  {(entry) => (
                    <PriceHistoryTable
                      page={entry().page}
                      grouping={queryStateSignal().historyGroup}
                      currentPage={appliedQuery().page}
                      onBuyerFilter={(name) => {
                        updatePublicQueryState(
                          queryStateSignal,
                          (query: PublicBoardQuery) => {
                            query.historyBuyer = name;
                            query.page = 1;
                          },
                          "push",
                        );
                      }}
                      onSellerFilter={(name) => {
                        updatePublicQueryState(
                          queryStateSignal,
                          (query: PublicBoardQuery) => {
                            query.historySeller = name;
                            query.page = 1;
                          },
                          "push",
                        );
                      }}
                      onPageSizeChange={(size) => updatePublicPageSize(queryStateSignal, size)}
                      onPageChange={(nextPage) => {
                        const requestedPage = Math.max(nextPage, 1);
                        updatePublicQueryState(
                          queryStateSignal,
                          (query: PublicBoardQuery) => {
                            query.page = requestedPage;
                          },
                          "push",
                        );
                      }}
                      onPageClamp={(nextPage) => {
                        updatePublicQueryState(
                          queryStateSignal,
                          (query: PublicBoardQuery) => {
                            query.page = Math.max(nextPage, 1);
                          },
                          "replace",
                        );
                      }}
                    />
                  )}
                </For>
              </Match>
            </Switch>
          </div>
        </Show>
        <Show when={lookup.failed()}>
          <p class="action-feedback action-feedback--error" role="alert">
            {t("data-error")}
          </p>
        </Show>
        {/* 抽屉宿主在结果读取分支之外：翻页、筛选或后台重读替换表格时，已打开的详情不被卸载。 */}
        <LotDetailLookupHost lookup={lookup} />
      </section>
    </LotDetailLookupContext>
  );
}

/** 从查询结果中取出与当前页码吻合的成交页；其余状态由调用方分别判定。 */
function historyPageOf(
  result: Result<PublicPriceHistoryListResponse, PriceHistoryFetchError> | undefined,
  requestedPage: number,
  requestedPageSize: number,
): PublicPriceHistoryListResponse | null {
  if (result === undefined || !result.ok) {
    return null;
  }
  return publicPageMatches(
    result.value.page,
    result.value.page_size,
    requestedPage,
    requestedPageSize,
  )
    ? result.value
    : null;
}

function priceFilterError(minimum: boolean, unit: HistoryPriceUnit): HistoryFilterError {
  if (minimum) {
    return unit === "C" ? "minimum_price_c" : unit === "K" ? "minimum_price_k" : "minimum_price_m";
  }
  return unit === "C" ? "maximum_price_c" : unit === "K" ? "maximum_price_k" : "maximum_price_m";
}

function historyQueryKey(query: PriceHistoryQuery): string {
  return JSON.stringify([
    query.world,
    query.name,
    query.from,
    query.to,
    query.minPrice,
    query.maxPrice,
    query.buyer,
    query.seller,
    query.sort,
    query.page,
  ]);
}

function historyQueryFromBoard(query: PublicBoardQuery): PriceHistoryQuery {
  const normalized = normalizeHistoryQuery(query);
  return {
    world: normalized.world,
    name: normalized.historyName,
    from: normalized.historyFrom,
    to: normalized.historyTo,
    minPrice: normalized.historyMinPrice,
    maxPrice: normalized.historyMaxPrice,
    buyer: normalized.historyBuyer,
    seller: normalized.historySeller,
    sort: normalized.historySort,
    page: Math.max(normalized.page, 1),
  };
}
