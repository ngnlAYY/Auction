// Public 价格历史页面及其筛选、值规范化、结果表格和详情查找宿主。
// 世界是本页“筛选条件”面板里的一项，点「应用」才写入地址栏，缺省为全部世界；其他跨世界页仍用
// 页面顶部的世界按钮组。Rust 版把世界下拉框放在表单的主筛选行。

import type { PublicPriceHistoryListResponse } from "@auction/contracts/dto/public/catalog/history";
import { structuralEquals } from "@auction/web-shared/reactive/equality";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import type { Result } from "@auction/web-shared/result";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Match, Show, Switch, useContext } from "solid-js";

import type { PriceHistoryQuery } from "../../../api/model";
import { fetchPriceHistory, type PriceHistoryFetchError } from "../../../api/queries/price-history";
import { PublicRefreshGenerationContext } from "../../board/refresh";
import {
  LotDetailLookupContext,
  LotDetailLookupHost,
  useLotDetailLookup,
} from "../../catalog/detail/lookup";
import { useContentLoading } from "../../loading";
import { publicPageMatches, updatePublicPage, updatePublicPageSize } from "../../pagination";
import { usePublicPageBounds } from "../../pagination/bounds";
import { updatePublicQueryPairs } from "../../pagination/location";
import { usePublicQueryState } from "../../query";
import { usePageRouteGuard } from "../../query/route-page";
import { WorldSwitchContext } from "../../shell/contexts";
import { PriceHistoryFilters } from "./filters/components";
import { useHistoryFilters } from "./filters/state";
import { HistoryResultControls } from "./presentation";
import { PriceHistoryTable } from "./table";
import { historyActiveFilterCount, historyHasRequestCondition } from "./values";

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
  const worldOptions = createValueMemo(() => worldSwitch.options());
  const filters = useHistoryFilters(queryStateSignal, worldOptions);
  const appliedQuery = createValueMemo(() => filters.applied().query);
  const activeFilterCount = createValueMemo(() => historyActiveFilterCount(appliedQuery()));
  const hasSearch = createValueMemo(
    () =>
      historyHasRequestCondition(appliedQuery()) &&
      Object.keys(filters.applied().errors).length === 0,
  );
  const bounds = usePublicPageBounds(queryStateSignal, "History", (query) => [
    ["page", query.pageSize],
  ]);
  const routeStillOnPage = usePageRouteGuard("History");
  const historyDependencies = createValueMemo(() => {
    return {
      query: appliedQuery(),
      readable: hasSearch() && !bounds.blocked("page"),
      pageSize: pageSize(),
      // 刷新代数参与依赖身份：同一查询也必须因权威重读请求重新发起读取。
      refreshGeneration: refreshGeneration(),
    };
  });
  const history = createTaskResource(historyDependencies, async (dependencies, context) => {
    // 没有已应用的筛选条件时只展示搜索入口，不发起查询。
    if (!dependencies.readable) {
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
    if (!hasSearch() || bounds.failed("page")) {
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

  return (
    <LotDetailLookupContext value={lookup}>
      <section class="public-history-page" aria-label={t("page-history-title")}>
        <div class="public-history-search-region">
          <PriceHistoryFilters
            filters={filters}
            worldOptions={worldOptions()}
            activeFilterCount={activeFilterCount()}
          />
        </div>
        <Show when={hasSearch()}>
          <div class="public-history-output">
            <HistoryResultControls
              sort={appliedQuery().sort}
              grouping={queryStateSignal().historyGroup}
              onSortChange={(sort) =>
                updatePublicQueryPairs(
                  queryStateSignal,
                  ["history_sort"],
                  sort === "closed_desc" ? [] : [["history_sort", sort]],
                  "push",
                )
              }
              onGroupChange={(group) =>
                // 分组只重排已取回的本页，地址写入也不能改变其他请求条件。
                updatePublicQueryPairs(
                  queryStateSignal,
                  ["history_group"],
                  group === "equipment" ? [] : [["history_group", group]],
                  "push",
                )
              }
            />
            <Switch>
              <Match when={bounds.failed("page")}>
                <AsyncState
                  kind="error"
                  title={t("page-history-title")}
                  detail={t("data-error")}
                  retryLabel={t("data-retry")}
                  onRetry={bounds.retry}
                />
              </Match>
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
                      onBuyerFilter={(name) =>
                        updatePublicQueryPairs(
                          queryStateSignal,
                          ["buyer", "page"],
                          [["buyer", name]],
                          "push",
                        )
                      }
                      onSellerFilter={(name) =>
                        updatePublicQueryPairs(
                          queryStateSignal,
                          ["seller", "page"],
                          [["seller", name]],
                          "push",
                        )
                      }
                      onPageSizeChange={(size) => updatePublicPageSize(queryStateSignal, size)}
                      onPageChange={(nextPage) =>
                        updatePublicPage(queryStateSignal, nextPage, "push")
                      }
                      onPageClamp={(nextPage) =>
                        routeStillOnPage() &&
                        updatePublicPage(queryStateSignal, nextPage, "replace")
                      }
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
