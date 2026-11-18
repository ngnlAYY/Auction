// 详情抽屉里的竞价历史：最近几条紧凑记录，以及按页浏览全部记录的弹窗。
// 两者都只读公开竞价记录；出价成功与实时竞价通知触发同一刷新节拍。

import type {
  PublicBidLogEntryResponse,
  PublicBidLogListResponse,
} from "@auction/contracts/dto/public/catalog/history";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import { Dialog } from "@auction/web-shared/ui/components/dialog";
import { TableColumns } from "@auction/web-shared/ui/components/table-columns";
import { Time } from "@auction/web-shared/ui/components/time";
import { formatCreditAmount } from "@auction/web-shared/ui/format";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Match, Show, Switch, useContext } from "solid-js";

import { fetchLotBidLog } from "../../../api/queries/bid-log";
import { PublicRefreshGenerationContext, useBidRefresh } from "../../board/refresh";
import { PublicPager } from "../../pagination";
import { BidHistoryParticipant, bidHistoryRows } from "../rows";

/** 抽屉内直接展示的最近出价条数。 */
const LATEST_BIDS = 4;
/** 完整历史弹窗的固定页长。弹窗不提供页长选择，也不读写地址栏分页参数。 */
const HISTORY_PAGE_SIZE = 10;

/** 展开后的落败代理已有独立 id，与胜出记录各自保留行。 */
export function bidHistoryRowKey(bid: PublicBidLogEntryResponse): string {
  return bid.id;
}

/** 最近出价。`returnFocus` 为真时“查看全部”带 autofocus：完整历史弹窗关闭后焦点回到它。 */
export function DetailLatestBids(props: {
  readonly auctionId: string;
  readonly lotId: string;
  readonly returnFocus: boolean;
  readonly onViewAll: () => void;
}): JSX.Element {
  const refreshGeneration = useContext(PublicRefreshGenerationContext);
  const latestDependencies = createValueMemo(() => ({
    auctionId: props.auctionId,
    lotId: props.lotId,
    // 两种历史读取都跟随权威刷新代号，普通出价只补读所属场次。
    refreshGeneration: refreshGeneration(),
  }));
  const bids = createTaskResource(latestDependencies, async (dependencies, context) =>
    fetchLotBidLog(dependencies.auctionId, dependencies.lotId, 1, LATEST_BIDS, context.signal),
  );
  useBidRefresh(
    bids,
    () => true,
    () => props.auctionId,
  );
  // 分页模式的响应带总数；缺失时不提供“查看全部”，不猜测条数。
  const total = () => {
    const snapshot = bids();
    return snapshot?.ok ? (snapshot.value.total ?? 0) : 0;
  };
  const page = () => {
    const snapshot = bids();
    return snapshot?.ok ? snapshot.value : null;
  };
  const emptyPage = () => {
    const current = page();
    return current !== null && current.bids.length === 0 ? current : null;
  };
  const nonEmptyPage = () => {
    const current = page();
    return current !== null && current.bids.length > 0 ? current : null;
  };
  const failed = () => {
    const snapshot = bids();
    return snapshot !== undefined && !snapshot.ok;
  };

  return (
    <section class="public-catalog-detail__section">
      <div class="public-catalog-detail__section-head">
        <h3 class="public-catalog-detail__section-title">{t("lot-bid-history")}</h3>
        <Show when={total() > LATEST_BIDS}>
          <button
            class="public-catalog-detail__more"
            type="button"
            aria-haspopup="dialog"
            autofocus={props.returnFocus}
            onClick={() => props.onViewAll()}
          >
            {t("catalog-detail-view-all", { count: String(total()) })}
          </button>
        </Show>
      </div>
      {/* 读取中不放“加载中”文字，避免内容到达时把区块撑开又收回。 */}
      <Switch>
        <Match when={bids() === undefined}>{null}</Match>
        <Match when={failed()}>
          <p class="public-catalog-detail__status" role="alert">
            <span>{t("data-error")}</span>
            <button
              class="public-catalog-detail__more"
              type="button"
              onClick={() => bids.restart()}
            >
              {t("data-retry")}
            </button>
          </p>
        </Match>
        <Match when={emptyPage()}>
          <p class="public-catalog-detail__empty">{t("lot-no-bids")}</p>
        </Match>
        <Match when={nonEmptyPage()}>
          {(current) => (
            <ol class="public-catalog-detail__bids">
              <For each={bidHistoryRows(current().bids)} keyed={bidHistoryRowKey}>
                {(bid) => (
                  <li
                    class={
                      bid().valid
                        ? "public-catalog-detail__bid-row public-catalog-detail__bid-row--leading"
                        : "public-catalog-detail__bid-row"
                    }
                    aria-description={bid().valid ? t("bidlog-valid") : t("bidlog-invalid")}
                  >
                    <span class="public-catalog-detail__bidder">
                      <BidHistoryParticipant
                        name={bid().bidder_name}
                        source={bid().source}
                        sourcePostNumber={bid().source_post_number}
                        sourcePostUrl={bid().source_post_url}
                      />
                    </span>
                    <b class="public-catalog-detail__bid-amount">
                      {formatCreditAmount(bid().amount)}
                    </b>
                    <span class="public-catalog-detail__bid-time">
                      <Time value={bid().created_at} />
                    </span>
                  </li>
                )}
              </For>
            </ol>
          )}
        </Match>
      </Switch>
    </section>
  );
}

/** 完整竞价历史弹窗。作为抽屉的兄弟节点渲染（不嵌套在抽屉内），焦点陷阱把它视为最顶层；
 * 关闭后焦点回到抽屉里打开它的按钮。分页状态只存在于弹窗内。 */
export function DetailBidHistoryDialog(props: {
  readonly auctionId: string;
  readonly lotId: string;
  readonly lotCode: string;
  readonly lotName: string;
  readonly onClose: () => void;
}): JSX.Element {
  const refreshGeneration = useContext(PublicRefreshGenerationContext);
  const page = createCell(1);
  const bidLogDependencies = createValueMemo(() => ({
    auctionId: props.auctionId,
    lotId: props.lotId,
    refreshGeneration: refreshGeneration(),
    requested: page(),
  }));
  const bidLog = createTaskResource(bidLogDependencies, async (dependencies, context) => {
    const result = await fetchLotBidLog(
      dependencies.auctionId,
      dependencies.lotId,
      dependencies.requested,
      HISTORY_PAGE_SIZE,
      context.signal,
    );
    return { requested: dependencies.requested, result };
  });
  useBidRefresh(
    bidLog,
    () => true,
    () => props.auctionId,
  );
  // 翻页期间保留上一页的表格，新页到达后整体替换；只有首次读取时正文为空。
  const loaded = () => bidLog()?.result;
  const loadedPage = () => {
    const result = loaded();
    return result?.ok ? result.value : null;
  };
  const emptyPage = () => {
    const current = loadedPage();
    return current !== null && current.bids.length === 0 ? current : null;
  };
  const tablePage = () => {
    const current = loadedPage();
    return current !== null && current.bids.length > 0 ? current : null;
  };
  const failed = () => {
    const result = loaded();
    return result !== undefined && !result.ok;
  };

  return (
    <Dialog
      title={t("lot-bid-history")}
      closeLabel={t("catalog-close")}
      onClose={() => props.onClose()}
    >
      <div class="public-bid-dialog__lot public-catalog-history__lot">
        <div class="public-bid-dialog__lot-name">
          <span class="public-bid-dialog__lot-code">{props.lotCode}</span>
          <span class="public-bid-dialog__lot-title">{props.lotName}</span>
        </div>
      </div>
      <div class="public-catalog-history">
        <Switch>
          <Match when={loaded() === undefined}>{null}</Match>
          <Match when={failed()}>
            <p class="public-catalog-detail__status" role="alert">
              {t("data-error")}
            </p>
          </Match>
          <Match when={emptyPage()}>
            <p class="public-catalog-detail__empty">{t("lot-no-bids")}</p>
          </Match>
          <Match when={tablePage()}>
            {(current) => (
              <>
                <BidHistoryTable page={current()} />
                <PublicPager
                  page={current().page ?? 1}
                  pageSize={HISTORY_PAGE_SIZE}
                  total={current().total}
                  hasMore={current().has_more ?? false}
                  hideWhenSingle={true}
                  showSizePicker={false}
                  label={t("lot-bid-history")}
                  onPageChange={(next: number) => page.set(next)}
                  onPageClamp={(next: number) => page.set(next)}
                  onPageSizeChange={() => {}}
                />
              </>
            )}
          </Match>
        </Switch>
      </div>
    </Dialog>
  );
}

/** 完整历史弹窗里的一页竞价记录：竞价者、金额与时间；有效出价整行标出。 */
function BidHistoryTable(props: { readonly page: PublicBidLogListResponse }): JSX.Element {
  return (
    <div class="public-auction-lot__table">
      <table aria-label={t("page-bidlog-title")}>
        <TableColumns layout="public-lot-bid-history" widths={[38, 22, 40]} />
        <thead>
          <tr>
            <th>{t("bidlog-bidder")}</th>
            <th class="public-auction-lot__number">{t("bidlog-amount")}</th>
            <th>{t("bidlog-time")}</th>
          </tr>
        </thead>
        <tbody>
          <For each={bidHistoryRows(props.page.bids)} keyed={bidHistoryRowKey}>
            {(bid) => (
              <tr
                class={bid().valid ? "public-auction-lot__leading" : ""}
                aria-description={bid().valid ? t("bidlog-valid") : t("bidlog-invalid")}
              >
                <td class="public-auction-lot__participant-cell" data-label={t("bidlog-bidder")}>
                  <BidHistoryParticipant
                    name={bid().bidder_name}
                    source={bid().source}
                    sourcePostNumber={bid().source_post_number}
                    sourcePostUrl={bid().source_post_url}
                  />
                </td>
                <td
                  class="public-auction-lot__amount public-auction-lot__number"
                  data-label={t("bidlog-amount")}
                >
                  {formatCreditAmount(bid().amount)}
                </td>
                <td data-label={t("bidlog-time")}>
                  <Time value={bid().created_at} />
                </td>
              </tr>
            )}
          </For>
        </tbody>
      </table>
    </div>
  );
}
