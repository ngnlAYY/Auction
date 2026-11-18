// 竞价日志表格、记录行与场次状态展示；行顺序与列归属由竞价日志页决定。

import type {
  PublicBidLogEntryResponse,
  PublicBidLogListResponse,
} from "@auction/contracts/dto/public/catalog/history";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { TableColumns } from "@auction/web-shared/ui/components/table-columns";
import { Time } from "@auction/web-shared/ui/components/time";
import { TruncatedText } from "@auction/web-shared/ui/components/truncated-text";
import { formatCreditAmount } from "@auction/web-shared/ui/format";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Show } from "solid-js";

import { lotCodeFor } from "../../../api/model";
import { LotDetailLookupButton } from "../../catalog/detail/lookup";
import { BidHistoryParticipant, bidHistoryRows } from "../../catalog/rows";
import { PublicPager } from "../../pagination";

/** 竞价记录表格；空记录提示与分页器在有无表格时都保留。 */
export function BidLogTable(props: {
  readonly page: PublicBidLogListResponse;
  readonly currentPage: number;
  readonly world: string | null;
  readonly lotFilter: string;
  readonly onPageChange: (page: number) => boolean;
  readonly onPageClamp: (page: number) => boolean;
  readonly onPageSizeChange: (size: number) => unknown;
}): JSX.Element {
  const hasMore = createValueMemo(() => props.page.has_more ?? false);
  const bids = createValueMemo(() => bidHistoryRows(props.page.bids));
  return (
    <div>
      <Show
        when={bids().length > 0}
        fallback={
          <p class="public-auction-bid-log-empty">
            {props.lotFilter === "" ? t("bidlog-empty") : t("bidlog-filter-empty")}
          </p>
        }
      >
        <div class="public-auction-bid-log-table-wrap">
          <table class="public-auction-bid-log-table" aria-label={t("page-bidlog-title")}>
            <TableColumns layout="public-bid-log" widths={[20, 9, 31, 22, 12, 6]} />
            <thead>
              <tr>
                <th scope="col">{t("bidlog-time")}</th>
                <th scope="col">{t("bidlog-lot")}</th>
                <th scope="col">{t("catalog-item")}</th>
                <th scope="col">{t("bidlog-bidder")}</th>
                <th scope="col" class="public-auction-bid-log-number">
                  {t("bidlog-amount")}
                </th>
                <th scope="col" class="public-lot-view-cell">
                  {t("catalog-actions")}
                </th>
              </tr>
            </thead>
            <tbody>
              <For each={bids()} keyed={(bid) => bid.id}>
                {(bid, index) => (
                  // 以服务端 bid id 作为稳定 key，避免翻页/筛选刷新后行状态错位。
                  <BidLogRow bid={bid()} world={props.world} anchorOrder={index()} />
                )}
              </For>
            </tbody>
          </table>
        </div>
      </Show>
      <PublicPager
        page={props.currentPage}
        pageSize={props.page.page_size}
        total={props.page.total}
        hasMore={hasMore()}
        label={t("page-bidlog-title")}
        onPageChange={props.onPageChange}
        onPageClamp={props.onPageClamp}
        onPageSizeChange={props.onPageSizeChange}
      />
    </div>
  );
}

/** 竞价记录行；编号只作标识，详情经操作栏的查看按钮在页内抽屉打开。 */
export function BidLogRow(props: {
  readonly bid: PublicBidLogEntryResponse;
  readonly world: string | null;
  readonly anchorOrder?: number;
}): JSX.Element {
  const lotCode = createValueMemo(() =>
    lotCodeFor(props.bid.equipment_category, props.bid.lot_number),
  );
  const validity = createValueMemo(() =>
    props.bid.valid ? t("bidlog-valid") : t("bidlog-invalid"),
  );
  return (
    <tr
      class={props.bid.valid ? "public-auction-bid-log-leading" : ""}
      aria-description={validity()}
    >
      <td data-label={t("bidlog-time")}>
        <Time value={props.bid.created_at} />
      </td>
      {/* 编号只作标识；详情经操作栏的查看按钮在页内抽屉打开。 */}
      <td class="public-auction-bid-log-code" data-label={t("bidlog-lot")}>
        {lotCode()}
      </td>
      {/* 名称外没有链接承接焦点，截断时由文本自身进入 Tab 顺序以查看完整名称。 */}
      <td data-label={t("catalog-item")}>
        <TruncatedText
          value={props.bid.equipment_name}
          size="wide"
          portal={true}
          anchorOrder={props.anchorOrder ?? null}
        />
      </td>
      <td class="public-auction-bid-log-participant" data-label={t("bidlog-bidder")}>
        <BidHistoryParticipant
          name={props.bid.bidder_name}
          source={props.bid.source}
          sourcePostNumber={props.bid.source_post_number}
          sourcePostUrl={props.bid.source_post_url}
          anchorOrder={props.anchorOrder ?? null}
        />
      </td>
      <td
        class="public-auction-bid-log-amount public-auction-bid-log-number"
        data-label={t("bidlog-amount")}
      >
        {formatCreditAmount(props.bid.amount)}
      </td>
      <td class="public-lot-view-cell" data-label={t("catalog-actions")}>
        <LotDetailLookupButton
          auctionId={props.bid.auction_id}
          lotId={props.bid.lot_id}
          lotCode={lotCode()}
          world={props.world}
        />
      </td>
    </tr>
  );
}

/** 竞价日志中的场次状态文案；未知状态也按未知显示。 */
export function bidLogStatusLabel(status: string): string {
  switch (status) {
    case "live":
      return t("auction-history-status-live");
    case "final":
      return t("auction-history-status-final");
    case "settled":
      return t("auction-history-status-settled");
    default:
      return t("bidlog-status-unknown");
  }
}

/** 场次状态样式类；只有 live/final 有专属修饰类，其余沿用通用状态类。 */
export function bidLogStatusClass(status: string): string {
  switch (status) {
    case "live":
      return "public-auction-bid-log-round-status public-auction-bid-log-round-status-live";
    case "final":
      return "public-auction-bid-log-round-status public-auction-bid-log-round-status-final";
    default:
      return "public-auction-bid-log-round-status";
  }
}
