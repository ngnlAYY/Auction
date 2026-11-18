// Public 历史拍卖列表。

import type { WorldsResponse } from "@auction/contracts/dto/public/account";
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
import { For, Match, Show, Switch } from "solid-js";

import { fetchWorlds } from "../../api/queries/current";
import { fetchAuctionHistoryPage } from "../../api/queries/directory";
import { supportedWorlds, worldLabel } from "../board/world";
import {
  auctionStatusClass,
  historicalAuctionStatusLabel,
  historicalBidLogHref,
  historicalFulfillmentHref,
  historicalLotsHref,
} from "../catalog/presentation";
import { useContentLoading } from "../loading";
import { PublicPager, publicPageMatches, updatePublicPageSize } from "../pagination";
import { updatePublicQueryState, usePublicQueryState } from "../query";

type HistoryRequest = readonly [world: string, status: string, page: number, pageSize: number];

interface LoadedHistory {
  readonly requested: HistoryRequest;
  readonly result: Result<AuctionListResponse<AuctionSummaryV2Response>, string>;
}

/** 历史拍卖列表入口；筛选器保持挂载，切换查询、失败或重试时不会丢失键盘焦点。 */
export function AuctionList(): JSX.Element {
  // 世界/状态/页码与地址栏双向同步。
  const queryState = usePublicQueryState();
  const queryStateSignal = queryState.state;
  const worldFilter = createValueMemo(() => queryStateSignal().world ?? "all");
  const statusFilter = createValueMemo(() => {
    const status = queryStateSignal().status;
    return status === "live" || status === "final" || status === "settled" ? status : "settled";
  });
  const pageNumber = createValueMemo(() => Math.max(queryStateSignal().page, 1));
  const pageSize = createValueMemo(() => queryStateSignal().pageSize);
  const refreshGeneration = createCell(0);
  const worlds = createTaskResource<null, Result<WorldsResponse, string> | null>(
    () => null,
    async (_dependencies, context) => {
      const result = await fetchWorlds(context.signal);
      if (!context.alive()) {
        return null;
      }
      return result;
    },
  );
  const worldOptions = createValueMemo(() => {
    const loaded = worlds();
    const options = loaded?.ok === true ? supportedWorlds(loaded.value) : ["main", "isekai"];
    const selected = worldFilter();
    // 历史深链的世界即使已从当前目录移除，也必须在筛选器中保留真实值。
    if (selected !== "all" && !options.includes(selected)) {
      options.push(selected);
    }
    return options;
  });
  const requested = createValueMemo(
    () => [worldFilter(), statusFilter(), pageNumber(), pageSize()] as const,
  );
  const auctions = createTaskResource(
    () => ({ generation: refreshGeneration(), requested: requested() }),
    async (dependencies, context): Promise<LoadedHistory | null> => {
      const [world, status, page, size] = dependencies.requested;
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
      if (loaded === null || loaded === undefined) {
        return null;
      }
      const [world, status, page, size] = requested();
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
    const result = currentResult();
    if (result === null) {
      return true;
    }
    return result.ok ? currentPage() === null : false;
  });
  useContentLoading(contentLoading);
  const retry = (): void => {
    const generation = refreshGeneration.peek();
    refreshGeneration.set(Math.min(generation + 1, Number.MAX_SAFE_INTEGER));
  };

  return (
    <section class="public-auction-rounds" aria-label={t("page-auctions-title")}>
      <div class="public-auction-rounds__controls">
        <label>
          {t("auction-history-world")}
          <select
            name="world"
            value={worldFilter()}
            onChange={(event) => {
              const world = event.currentTarget.value;
              updatePublicQueryState(
                queryStateSignal,
                (query) => {
                  query.world = world === "all" ? null : world;
                  query.auctionId = null;
                  query.category = null;
                  query.page = 1;
                },
                "push",
              );
            }}
          >
            <option value="all">{t("auction-history-world-all")}</option>
            <For each={worldOptions()} keyed={false}>
              {(world) => <option value={world()}>{worldLabel(world())}</option>}
            </For>
          </select>
        </label>
        <label>
          {t("auction-history-status")}
          <select
            name="status"
            value={statusFilter()}
            onChange={(event) => {
              const status = event.currentTarget.value;
              updatePublicQueryState(
                queryStateSignal,
                (query) => {
                  query.status = status;
                  query.page = 1;
                },
                "push",
              );
            }}
          >
            <option value="settled">{t("auction-history-status-settled")}</option>
            <option value="final">{t("auction-history-status-final")}</option>
            <option value="live">{t("auction-history-status-live")}</option>
          </select>
        </label>
      </div>
      <Switch>
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
              onPageSizeChange={(size) => updatePublicPageSize(queryStateSignal, size)}
              onPageChange={(nextPage) => {
                updatePublicQueryState(
                  queryStateSignal,
                  (query) => {
                    query.page = nextPage;
                  },
                  "push",
                );
              }}
              onPageClamp={(nextPage) => {
                updatePublicQueryState(
                  queryStateSignal,
                  (query) => {
                    query.page = nextPage;
                  },
                  "replace",
                );
              }}
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
  readonly onPageChange: (page: number) => void;
  readonly onPageClamp: (page: number) => void;
  readonly onPageSizeChange: (size: number) => void;
}): JSX.Element {
  const pageSize = createValueMemo(() => Math.max(props.page.page_size, 1));
  return (
    <div>
      <Show
        when={props.page.auctions.length === 0}
        fallback={<AuctionHistoryTable rows={props.page.auctions} />}
      >
        <div class="public-auction-empty" role="status">
          <p class="public-auction-empty__title">{t("auction-history-empty-title")}</p>
          <p class="public-auction-empty__hint">{t("auction-history-empty-hint")}</p>
        </div>
      </Show>
      <PublicPager
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
}): JSX.Element {
  return (
    <div class="public-auction-rounds__table">
      <table aria-label={t("page-auctions-title")}>
        {/* Solid 版的“查看”一列由 styles/auction-rounds-links.css 改成刚好容纳四个入口的固定宽度，
            这里最后一项只是没有该样式时的回退；其余列按各自的百分比分配剩余宽度。
            Rust 版仍是 [12, 8, 22, 7, 7, 7, 21, 16]。 */}
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
                  bidLogHref: historicalBidLogHref(auction.id, auction.world_id),
                  // 只有已结算场次有履约进度；进行中的场次不给入口。
                  fulfillmentHref:
                    auction.status === "settled"
                      ? historicalFulfillmentHref(auction.id, auction.world_id)
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
