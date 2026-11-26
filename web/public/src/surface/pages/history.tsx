// Public 历史拍卖列表。本页只列已结算场次，没有状态筛选；页面里没有世界下拉框，世界由页面
// 顶部的世界按钮组（WorldTabs）切换并写入地址栏。Rust 版页面内有世界与状态两个下拉框。

import type {
  AuctionListResponse,
  AuctionSummaryV2Response,
} from "@auction/contracts/dto/public/catalog/auction";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import type { Result } from "@auction/web-shared/result";
import { Link } from "@auction/web-shared/router";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { TableColumns } from "@auction/web-shared/ui/components/table-columns";
import { Time } from "@auction/web-shared/ui/components/time";
import { formatCreditAmount } from "@auction/web-shared/ui/format";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { type Accessor, createEffect, For, Match, Show, Switch, useContext } from "solid-js";

import { fetchAuctionHistoryPage } from "../../api/queries/directory";
import { PublicRefreshGenerationContext } from "../board/refresh";
import { WorldTabs, worldLabel } from "../board/world";
import {
  auctionStatusClass,
  historicalAuctionStatusLabel,
  historicalBidLogHref,
  historicalFulfillmentHref,
  historicalLotsHref,
} from "../catalog/presentation";
import { useContentLoading } from "../loading";
import {
  PublicPager,
  publicPageMatches,
  updatePublicPage,
  updatePublicPageSize,
} from "../pagination";
import { publicOffsetLastPage, usePublicPageBounds } from "../pagination/bounds";
import { usePublicQueryState } from "../query";
import { updatePublicQueryPairs } from "../query/location";
import { usePageRouteGuard } from "../query/route-page";

type HistoryRequest = readonly [world: string, status: string, page: number, pageSize: number];

interface LoadedHistory {
  readonly requested: HistoryRequest;
  readonly result: Result<AuctionListResponse<AuctionSummaryV2Response>, string>;
}

/** 往期列表读取只展示当前查询的快照，页内重试与外壳刷新各自触发重读。 */
export function useAuctionHistory(requested: Accessor<HistoryRequest | null>) {
  const refreshGeneration = useContext(PublicRefreshGenerationContext);
  const retryGeneration = createCell(0);
  const auctions = createTaskResource(
    () => ({
      refreshGeneration: refreshGeneration(),
      retryGeneration: retryGeneration(),
      requested: requested(),
    }),
    async (dependencies, context): Promise<LoadedHistory | null> => {
      if (dependencies.requested === null) {
        return null;
      }
      const [world, status, page, size] = dependencies.requested;
      if (page > publicOffsetLastPage(size)) {
        return null;
      }
      const result = await fetchAuctionHistoryPage(
        world === "all" ? null : world,
        status,
        page,
        size,
        context.signal,
      );
      if (!context.alive()) {
        return null;
      }
      return { requested: dependencies.requested, result };
    },
  );
  // 查询变化时旧快照不能参与列表展示或页码纠偏，同查询后台刷新仍保留旧值。
  const currentResult = createValueMemo(
    (): Result<AuctionListResponse<AuctionSummaryV2Response>, string> | null => {
      const loaded = auctions();
      const request = requested();
      if (loaded === null || loaded === undefined || request === null) {
        return null;
      }
      const [world, status, page, size] = request;
      const requestedPage = loaded.requested;
      if (
        requestedPage[0] !== world ||
        requestedPage[1] !== status ||
        requestedPage[2] !== page ||
        requestedPage[3] !== size
      ) {
        return null;
      }
      return loaded.result;
    },
  );
  const retry = (): void => {
    const generation = retryGeneration.peek();
    retryGeneration.set(Math.min(generation + 1, Number.MAX_SAFE_INTEGER));
  };
  return { currentResult, retry };
}

/** 历史拍卖列表入口；本页只列已结算场次，状态筛选已移除。页面里没有世界下拉框，世界范围由
 * 页面顶部的世界按钮组（WorldTabs）写入地址栏；Rust 版页面内有世界与状态两个下拉框。 */
export function AuctionList(): JSX.Element {
  // 世界范围与页码仍来自地址栏；状态不再读取查询参数。
  const queryState = usePublicQueryState();
  const queryStateSignal = queryState.state;
  const worldFilter = createValueMemo(() => queryStateSignal().world ?? "all");
  const pageNumber = createValueMemo(() => Math.max(queryStateSignal().page, 1));
  const pageSize = createValueMemo(() => queryStateSignal().pageSize);
  const routeStillOnPage = usePageRouteGuard("AuctionList");
  const bounds = usePublicPageBounds(queryStateSignal, "AuctionList", (query) => [
    ["page", query.pageSize],
  ]);
  // 旧链接里的 status 不再生效；含解析器未接受的未知取值在内，读入后无历史地移除一次。
  createEffect(
    () => {
      const query = queryStateSignal();
      return query.status ?? query.passthrough.find(([key]) => key === "status")?.[1] ?? null;
    },
    (status) => {
      if (status === null) {
        return;
      }
      // 前进/后退换页时旧页面可能在新地址下被唤醒；纠偏只能写回仍属于本页的地址。
      if (!routeStillOnPage()) {
        return;
      }
      updatePublicQueryPairs(queryStateSignal, ["status"], [], "replace");
    },
  );
  const requested = createValueMemo(() =>
    bounds.blocked("page") ? null : ([worldFilter(), "settled", pageNumber(), pageSize()] as const),
  );
  const { currentResult, retry } = useAuctionHistory(requested);
  const currentPage = createValueMemo(() => {
    const result = currentResult();
    if (result === null || !result.ok) {
      return null;
    }
    const page = result.value;
    return publicPageMatches(page.page, page.page_size, pageNumber(), pageSize()) ? page : null;
  });
  // 没有当前查询的结果（含等待页码纠偏后的重读）时由顶栏提示加载，正文留空。
  const contentLoading = createValueMemo(() => {
    if (bounds.failed("page")) {
      return false;
    }
    const result = currentResult();
    if (result === null) {
      return true;
    }
    return result.ok ? currentPage() === null : false;
  });
  useContentLoading(contentLoading);

  return (
    <section class="public-auction-rounds" aria-label={t("page-auctions-title")}>
      {/* 世界切换：页面顶部的按钮组（WorldTabs，Solid 版专属）；Rust 版用页面内的下拉框。 */}
      <div class="public-auction-group-toolbar public-world-toolbar">
        <WorldTabs />
      </div>
      <Switch>
        <Match when={bounds.failed("page")}>
          <AsyncState
            kind="error"
            title={t("page-auctions-title")}
            detail={t("data-error")}
            retryLabel={t("data-retry")}
            onRetry={bounds.retry}
          />
        </Match>
        <Match when={currentResult() === null}>{null}</Match>
        <Match when={currentResult()?.ok === false}>
          <AsyncState
            kind="error"
            title={t("page-auctions-title")}
            detail={t("data-error")}
            retryLabel={t("data-retry")}
            onRetry={retry}
          />
        </Match>
        <Match when={currentPage()}>
          {(page) => (
            <AuctionHistoryPage
              page={page()}
              pageNumber={pageNumber()}
              world={queryStateSignal().world}
              onPageSizeChange={(size) => updatePublicPageSize(queryStateSignal, size)}
              onPageChange={(nextPage) => updatePublicPage(queryStateSignal, nextPage, "push")}
              onPageClamp={(nextPage) =>
                routeStillOnPage() && updatePublicPage(queryStateSignal, nextPage, "replace")
              }
            />
          )}
        </Match>
      </Switch>
    </section>
  );
}

/** 当前历史查询的一页；空列表仍渲染分页器，以便越界页码纠偏。 */
export function AuctionHistoryPage(props: {
  readonly page: AuctionListResponse<AuctionSummaryV2Response>;
  readonly pageNumber: number;
  readonly world: string | null;
  readonly onPageChange: (page: number) => boolean;
  readonly onPageClamp: (page: number) => boolean;
  readonly onPageSizeChange: (size: number) => boolean;
}): JSX.Element {
  const pageSize = createValueMemo(() => Math.max(props.page.page_size, 1));
  return (
    <div>
      <Show
        when={props.page.auctions.length === 0}
        fallback={<AuctionHistoryTable rows={props.page.auctions} world={props.world} />}
      >
        <div class="public-auction-empty" role="status">
          <p class="public-auction-empty__title">{t("auction-history-empty-title")}</p>
        </div>
      </Show>
      <PublicPager
        pageLevel
        page={props.pageNumber}
        pageSize={pageSize()}
        total={props.page.total}
        hasMore={props.page.has_more ?? false}
        label={t("page-auctions-title")}
        onPageChange={props.onPageChange}
        onPageClamp={props.onPageClamp}
        onPageSizeChange={props.onPageSizeChange}
      />
    </div>
  );
}

/** 历史场次表；行内容全部来自 v2 摘要，论坛地址为可选列。 */
export function AuctionHistoryTable(props: {
  readonly rows: readonly AuctionSummaryV2Response[];
  readonly world: string | null;
}): JSX.Element {
  return (
    <div class="public-auction-rounds__table">
      <table aria-label={t("page-auctions-title")}>
        {/* “查看”一列由共用样式 public.css 改成刚好容纳四个入口的固定宽度，最后一项只是没有该样式时的
            回退；其余列按各自的百分比分配剩余宽度。两套实现的列比例相同。 */}
        <TableColumns layout="public-auction-history" widths={[12, 7, 24, 6, 6, 6, 18, 21]} />
        <thead>
          <tr>
            <th scope="col">{t("auction-history-number")}</th>
            <th scope="col">{t("auction-history-world")}</th>
            <th scope="col">{t("auction-history-ended")}</th>
            <th scope="col" class="public-auction-rounds__number">
              {t("auction-history-lots")}
            </th>
            <th scope="col" class="public-auction-rounds__number">
              {t("auction-history-sold")}
            </th>
            <th scope="col" class="public-auction-rounds__number">
              {t("auction-history-unsold")}
            </th>
            <th scope="col" class="public-auction-rounds__number">
              {t("auction-history-total")}
            </th>
            <th scope="col">{t("auction-history-links")}</th>
          </tr>
        </thead>
        <tbody>
          <For each={props.rows} keyed={false}>
            {(row) => {
              const display = createValueMemo(() => {
                const auction = row();
                return {
                  auction,
                  forumThreadUrl: auction.forum_thread_url,
                  ended:
                    auction.settled_at ??
                    auction.effective_end_at ??
                    auction.opened_at ??
                    auction.scheduled_open_at,
                  lotsHref: historicalLotsHref(auction.id, auction.world_id),
                  // 出价记录与履约进度都是跨世界页；行内入口继承当前地址栏的世界范围，不替用户改范围。
                  bidLogHref: historicalBidLogHref(auction.id, props.world),
                  // 只有已结算场次有履约进度；进行中的场次不给入口。
                  fulfillmentHref:
                    auction.status === "settled"
                      ? historicalFulfillmentHref(auction.id, props.world)
                      : null,
                  statusClass: auctionStatusClass(auction.status),
                  statusLabel: historicalAuctionStatusLabel(auction.status),
                };
              });
              return (
                <tr>
                  <td class="public-auction-rounds__round" data-label={t("auction-history-number")}>
                    {`#${display().auction.sequence}`}
                    <span
                      class={[
                        "public-auction-rounds__status",
                        `public-auction-rounds__status--${display().statusClass}`,
                      ]}
                    >
                      {display().statusLabel}
                    </span>
                  </td>
                  <td data-label={t("auction-history-world")}>
                    {worldLabel(display().auction.world_id)}
                  </td>
                  <td data-label={t("auction-history-ended")}>
                    <Time value={display().ended} />
                  </td>
                  <td class="public-auction-rounds__number" data-label={t("auction-history-lots")}>
                    {display().auction.lot_count}
                  </td>
                  <td class="public-auction-rounds__number" data-label={t("auction-history-sold")}>
                    {display().auction.sold_count}
                  </td>
                  <td
                    class="public-auction-rounds__number"
                    data-label={t("auction-history-unsold")}
                  >
                    {display().auction.unsold_count}
                  </td>
                  <td class="public-auction-rounds__total" data-label={t("auction-history-total")}>
                    {formatCreditAmount(display().auction.sold_amount)}
                  </td>
                  <td
                    class="public-auction-rounds__links-cell"
                    data-label={t("auction-history-links")}
                  >
                    <div class="public-auction-rounds__links">
                      <Link href={display().lotsHref}>{t("auction-history-link-lots")}</Link>
                      <Link href={display().bidLogHref}>{t("auction-history-link-bidlog")}</Link>
                      <Show when={display().fulfillmentHref}>
                        {(href) => (
                          <Link href={href()}>{t("auction-history-link-fulfillment")}</Link>
                        )}
                      </Show>
                      <Show when={display().forumThreadUrl}>
                        {(url) => (
                          <a href={url()} target="_blank" rel="noopener noreferrer">
                            {t("auction-history-link-forum")}
                          </a>
                        )}
                      </Show>
                    </div>
                  </td>
                </tr>
              );
            }}
          </For>
        </tbody>
      </table>
    </div>
  );
}
