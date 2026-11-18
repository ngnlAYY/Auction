// 履约进度表格、逐件状态行与分页器。

import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { TableColumns } from "@auction/web-shared/ui/components/table-columns";
import { TruncatedText } from "@auction/web-shared/ui/components/truncated-text";
import { formatCreditAmount } from "@auction/web-shared/ui/format";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Show } from "solid-js";

import type { FulfillmentLot, FulfillmentPage } from "../../../api/fulfillment";
import { MAX_FULFILLMENT_PAGE_SIZE } from "../../../api/fulfillment";
import { lotCodeFor } from "../../../api/model";
import { LotDetailLookupButton } from "../../catalog/detail/lookup";
import { formatAuctionTime } from "../../catalog/presentation";
import { PublicPager } from "../../pagination";
import { statusKey, statusTone } from "./status";

/** 已结算场次的履约进度表格与分页器。空表文案区分场次无拍品和筛选无结果；状态色只作辅助，
 * 完整状态始终由文字给出。 */
export function FulfillmentTable(props: {
  readonly page: FulfillmentPage;
  readonly currentPage: number;
  readonly lotFilter: string;
  readonly onPageChange: (page: number) => void;
  readonly onPageClamp: (page: number) => void;
  readonly onPageSizeChange: (size: number) => unknown;
}): JSX.Element {
  const auctionId = createValueMemo(() => props.page.auction.id);
  const world = createValueMemo(() => props.page.auction.world);
  return (
    <div>
      <Show
        when={props.page.lots.length > 0}
        fallback={
          <p class="public-fulfillment__empty">
            {props.lotFilter === "" ? t("fulfillment-empty") : t("fulfillment-filter-empty")}
          </p>
        }
      >
        <div class="public-fulfillment__table-wrap">
          <table class="public-fulfillment__table" aria-label={t("page-fulfillment-title")}>
            <TableColumns layout="public-fulfillment" widths={[9, 24, 13, 13, 11, 24, 6]} />
            <thead>
              <tr>
                <th scope="col">{t("fulfillment-lot")}</th>
                <th scope="col">{t("catalog-item")}</th>
                <th scope="col">{t("fulfillment-seller")}</th>
                <th scope="col">{t("fulfillment-buyer")}</th>
                <th scope="col" class="public-fulfillment__number">
                  {t("fulfillment-final-price")}
                </th>
                <th scope="col">{t("fulfillment-status")}</th>
                <th scope="col" class="public-lot-view-cell">
                  {t("catalog-actions")}
                </th>
              </tr>
            </thead>
            <tbody>
              <For each={props.page.lots} keyed={(lot) => lot.lot_id}>
                {(lot) => <FulfillmentRow auctionId={auctionId()} lot={lot()} world={world()} />}
              </For>
            </tbody>
          </table>
        </div>
      </Show>
      <PublicPager
        page={props.currentPage}
        pageSize={props.page.page_size}
        total={props.page.total}
        maxPageSize={MAX_FULFILLMENT_PAGE_SIZE}
        label={t("page-fulfillment-title")}
        onPageChange={props.onPageChange}
        onPageClamp={props.onPageClamp}
        onPageSizeChange={props.onPageSizeChange}
      />
    </div>
  );
}

function FulfillmentRow(props: {
  readonly auctionId: string;
  readonly lot: FulfillmentLot;
  readonly world: string;
}): JSX.Element {
  const lotCode = createValueMemo(() =>
    lotCodeFor(props.lot.equipment_category, props.lot.lot_number),
  );
  // 成交后展示当前买家；流拍不伪造买家，退回买方等例外状态也保留明确的状态行。
  const buyer = createValueMemo(() => {
    const name = props.lot.buyer_name;
    if (name !== null && name !== undefined) {
      return name;
    }
    return props.lot.status.startsWith("unsold_") ? t("fulfillment-unsold") : "—";
  });
  const finalPrice = createValueMemo(() => {
    const price = props.lot.final_price;
    return price === null || price === undefined ? "—" : formatCreditAmount(price);
  });
  const deadline = createValueMemo(() => {
    const value = props.lot.payment_deadline_at;
    if (value === null || value === undefined) {
      return null;
    }
    return t("fulfillment-payment-deadline", { deadline: formatAuctionTime(value) });
  });
  const returns = createValueMemo(() => {
    const count = props.lot.cod_return_count;
    const limit = props.lot.cod_return_limit;
    if (count === null || count === undefined || limit === null || limit === undefined) {
      return null;
    }
    return t("fulfillment-cod-returns", { count: String(count), limit: String(limit) });
  });
  return (
    <tr>
      {/* 编号只作标识；详情经操作栏的查看按钮在页内抽屉打开。 */}
      <td class="public-fulfillment__code" data-label={t("fulfillment-lot")}>
        {lotCode()}
      </td>
      <td data-label={t("catalog-item")}>
        <TruncatedText value={props.lot.equipment_name} size="wide" portal={true} />
      </td>
      <td data-label={t("fulfillment-seller")}>
        <TruncatedText value={props.lot.seller_name} portal={true} />
      </td>
      <td data-label={t("fulfillment-buyer")}>
        <TruncatedText value={buyer()} portal={true} />
      </td>
      <td class="public-fulfillment__number" data-label={t("fulfillment-final-price")}>
        {finalPrice()}
      </td>
      <td class="public-fulfillment__state" data-label={t("fulfillment-status")}>
        {/* 状态色只作辅助，文字始终给出完整状态。 */}
        <span class={statusTone(props.lot.status)}>{t(statusKey(props.lot.status))}</span>
        <Show when={deadline() !== null}>
          <span class="public-fulfillment__detail">{deadline()}</span>
        </Show>
        <Show when={returns() !== null}>
          <span class="public-fulfillment__detail">{returns()}</span>
        </Show>
      </td>
      <td class="public-lot-view-cell" data-label={t("catalog-actions")}>
        <LotDetailLookupButton
          auctionId={props.auctionId}
          lotId={props.lot.lot_id}
          lotCode={lotCode()}
          world={props.world}
        />
      </td>
    </tr>
  );
}
