// 价格历史的分组表格。

import type {
  PublicPriceHistoryEntryResponse,
  PublicPriceHistoryListResponse,
  PublicPriceHistoryStatsResponse,
} from "@auction/contracts/dto/public/catalog/history";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { Link } from "@auction/web-shared/router";
import { TableColumns } from "@auction/web-shared/ui/components/table-columns";
import { Time } from "@auction/web-shared/ui/components/time";
import { TruncatedText } from "@auction/web-shared/ui/components/truncated-text";
import { formatCreditAmount } from "@auction/web-shared/ui/format";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Show } from "solid-js";

import { lotCodeFor } from "../../../api/model";
import { worldLabel } from "../../board/world";
import { LotDetailLookupButton } from "../../catalog/detail/lookup";
import { catalogCategoryKey, catalogCategoryLabel } from "../../catalog/presentation";
import { PublicPager } from "../../pagination";
import { historyGroups, historyMedianAmount } from "./presentation";

/** 价格历史表格；分组只消费当前页快照，不补查其他分页。 */
export function PriceHistoryTable(props: {
  readonly page: PublicPriceHistoryListResponse;
  readonly grouping: string;
  readonly currentPage: number;
  readonly onBuyerFilter: (buyer: string) => void;
  readonly onSellerFilter: (seller: string) => void;
  readonly onPageChange: (page: number) => boolean;
  readonly onPageClamp: (page: number) => boolean;
  readonly onPageSizeChange: (size: number) => boolean;
}): JSX.Element {
  const pageSize = createValueMemo(() => Math.max(props.page.page_size, 1));
  const empty = createValueMemo(() => props.page.rows.length === 0);
  // 当前组件拥有本页快照，分组直接接管行，不为派生展示复制整页 DTO。
  const groups = createValueMemo(() => historyGroups(props.page.rows, props.grouping));
  // 分组面板的 key 包含分组方式、页码与页大小；先在列表项里冻结完整 key，
  // 避免 keyed 在比较新旧项时读取到切换后的分组方式而复用错误面板。
  const groupEntries = createValueMemo(() =>
    groups().map((group) => ({
      key: `${props.currentPage}:${pageSize()}:${group.key}`,
      group,
    })),
  );
  const stats = createValueMemo(() => {
    const current = props.page.stats;
    return current === null || current === undefined || current.count <= 0 ? null : current;
  });
  return (
    <div class="public-history-results">
      <Show when={stats()}>{(current) => <PriceHistoryStatsView stats={current()} />}</Show>
      <Show
        when={empty()}
        fallback={
          <div class="public-history-result-groups">
            <Show
              when={props.grouping === "flat"}
              fallback={
                <For each={groupEntries()} keyed={(entry) => entry.key}>
                  {(entry) => (
                    <PriceHistoryGroupPanel
                      label={entry().group.label}
                      rows={entry().group.rows}
                      onBuyerFilter={props.onBuyerFilter}
                      onSellerFilter={props.onSellerFilter}
                    />
                  )}
                </For>
              }
            >
              <For each={groups()} keyed={false}>
                {(group) => (
                  <PriceHistoryRowsTable
                    rows={group().rows}
                    title={t("page-history-title")}
                    onBuyerFilter={props.onBuyerFilter}
                    onSellerFilter={props.onSellerFilter}
                  />
                )}
              </For>
            </Show>
          </div>
        }
      >
        <div class="public-auction-empty" role="status">
          <p class="public-auction-empty__title">{t("history-empty-title")}</p>
          <p class="public-auction-empty__hint">{t("history-empty-hint")}</p>
        </div>
      </Show>
      <PublicPager
        page={props.currentPage}
        pageSize={pageSize()}
        total={props.page.total}
        hasMore={props.page.has_more ?? false}
        label={t("page-history-title")}
        onPageChange={props.onPageChange}
        onPageClamp={props.onPageClamp}
        onPageSizeChange={props.onPageSizeChange}
      />
    </div>
  );
}

/** 每组独立呈现结果框；组头始终保留，折叠只卸载本组表格，不读取其他分页。 */
function PriceHistoryGroupPanel(props: {
  readonly label: string;
  readonly rows: readonly PublicPriceHistoryEntryResponse[];
  readonly onBuyerFilter: (buyer: string) => void;
  readonly onSellerFilter: (seller: string) => void;
}): JSX.Element {
  const expanded = createCell(true);
  const count = createValueMemo(() => props.rows.length);
  return (
    <section class="public-history-group-panel" aria-label={props.label}>
      <h2 class="public-history-group-heading">
        <button
          type="button"
          class="public-history-group-toggle"
          aria-expanded={expanded() ? "true" : "false"}
          onClick={() => expanded.set(!expanded())}
        >
          <span class="public-chevron" aria-hidden="true" />
          <TruncatedText value={props.label} size="wide" focusable={false} portal={true} />
          <span class="public-history-group-count">
            {t("history-group-count", { count: count() })}
          </span>
        </button>
      </h2>
      <Show when={expanded()}>
        <PriceHistoryRowsTable
          rows={props.rows}
          title={props.label}
          onBuyerFilter={props.onBuyerFilter}
          onSellerFilter={props.onSellerFilter}
        />
      </Show>
    </section>
  );
}

/** 各结果框复用相同十列表头与列宽，标题关联当前组；窄屏仍保留原生表格结构。 */
function PriceHistoryRowsTable(props: {
  readonly rows: readonly PublicPriceHistoryEntryResponse[];
  readonly title: string;
  readonly onBuyerFilter: (buyer: string) => void;
  readonly onSellerFilter: (seller: string) => void;
}): JSX.Element {
  return (
    <div class="public-history-table-wrap">
      <table class="public-history-table" aria-label={props.title}>
        <TableColumns layout="public-price-history" widths={[22, 9, 5, 7, 6, 11, 9, 9, 16, 6]} />
        <thead>
          <tr>
            <th scope="col">{t("history-item")}</th>
            <th scope="col">{t("history-category")}</th>
            <th scope="col" class="public-history-number">
              {t("history-level")}
            </th>
            <th scope="col">{t("history-world")}</th>
            <th scope="col">{t("history-round")}</th>
            <th scope="col" class="public-history-number">
              {t("history-price")}
            </th>
            <th scope="col">{t("history-buyer")}</th>
            <th scope="col">{t("history-seller")}</th>
            <th scope="col">{t("history-closed")}</th>
            <th scope="col" class="public-lot-view-cell">
              {t("catalog-actions")}
            </th>
          </tr>
        </thead>
        <tbody>
          <For each={props.rows} keyed={(row) => row.lot_id}>
            {(row) => (
              <PriceHistoryRow
                row={row()}
                onBuyerFilter={props.onBuyerFilter}
                onSellerFilter={props.onSellerFilter}
              />
            )}
          </For>
        </tbody>
      </table>
    </div>
  );
}

/** 过滤集统计；精确中位数缺失时沿用整数中位数。 */
export function PriceHistoryStatsView(props: {
  readonly stats: PublicPriceHistoryStatsResponse;
}): JSX.Element {
  const median = createValueMemo(() => historyMedianAmount(props.stats));
  return (
    <dl class="public-history-stats" aria-label={t("history-statistics")}>
      <div class="public-history-stat">
        <dt>{t("history-count")}</dt>
        <dd>{props.stats.count}</dd>
      </div>
      <div class="public-history-stat">
        <dt>{t("history-min")}</dt>
        <dd>{formatCreditAmount(props.stats.min)}</dd>
      </div>
      <div class="public-history-stat">
        <dt>{t("history-max")}</dt>
        <dd>{formatCreditAmount(props.stats.max)}</dd>
      </div>
      <div class="public-history-stat">
        <dt>{t("history-median")}</dt>
        <dd>{median()}</dd>
      </div>
    </dl>
  );
}

/** 一条成交记录；名称不是链接，详情统一经操作栏的查看按钮在页内抽屉打开。 */
export function PriceHistoryRow(props: {
  readonly row: PublicPriceHistoryEntryResponse;
  readonly onBuyerFilter: (buyer: string) => void;
  readonly onSellerFilter: (seller: string) => void;
}): JSX.Element {
  const auctionHref = createValueMemo(() =>
    historyAuctionHref(props.row.auction_id, props.row.world_id),
  );
  const lotCode = createValueMemo(() =>
    lotCodeFor(props.row.equipment_category, props.row.lot_number),
  );
  const equipmentLabel = createValueMemo(() => `${lotCode()} · ${props.row.equipment_name}`);
  const level = createValueMemo(() =>
    props.row.equipment_level == null ? t("value-empty") : props.row.equipment_level.toString(),
  );
  const world = createValueMemo(() => worldLabel(props.row.world_id));
  const category = createValueMemo(() =>
    catalogCategoryLabel(catalogCategoryKey(props.row.equipment_category)),
  );
  const price = createValueMemo(() => formatCreditAmount(props.row.price));
  const buyer = createValueMemo(() => props.row.buyer_name);
  const seller = createValueMemo(() => props.row.seller_name);
  return (
    <tr>
      <td class="public-history-equipment" data-label={t("history-item")}>
        {/* 名称外没有链接承接焦点，截断时由文本自身进入 Tab 顺序查看完整名称。 */}
        <TruncatedText value={equipmentLabel()} size="wide" portal={true} />
      </td>
      <td data-label={t("history-category")}>{category()}</td>
      <td class="public-history-number" data-label={t("history-level")}>
        {level()}
      </td>
      <td data-label={t("history-world")}>{world()}</td>
      <td class="public-history-auction" data-label={t("history-round")}>
        <Link href={auctionHref()}>{`#${props.row.auction_sequence}`}</Link>
      </td>
      <td class="public-history-price public-history-number" data-label={t("history-price")}>
        {price()}
      </td>
      <td data-label={t("history-buyer")}>
        <button
          type="button"
          class="public-history-player-filter"
          aria-label={t("history-filter-by-buyer", { name: buyer() })}
          title={t("history-filter-by-buyer", { name: buyer() })}
          onClick={() => props.onBuyerFilter(buyer())}
        >
          <TruncatedText
            value={props.row.buyer_name}
            size="compact"
            focusable={false}
            portal={true}
          />
        </button>
      </td>
      <td data-label={t("history-seller")}>
        <Show
          when={seller() === "—"}
          fallback={
            <button
              type="button"
              class="public-history-player-filter"
              aria-label={t("history-filter-by-seller", { name: seller() })}
              title={t("history-filter-by-seller", { name: seller() })}
              onClick={() => props.onSellerFilter(seller())}
            >
              <TruncatedText
                value={props.row.seller_name}
                size="compact"
                focusable={false}
                portal={true}
              />
            </button>
          }
        >
          <TruncatedText value={props.row.seller_name} size="compact" portal={true} />
        </Show>
      </td>
      <td data-label={t("history-closed")}>
        <Time value={props.row.closed_at} />
      </td>
      <td class="public-lot-view-cell" data-label={t("catalog-actions")}>
        <LotDetailLookupButton
          auctionId={props.row.auction_id}
          lotId={props.row.lot_id}
          lotCode={lotCode()}
          world={props.row.world_id}
        />
      </td>
    </tr>
  );
}

/** 构造成交轮次链接；非 main 世界附带 world 参数。 */
export function historyAuctionHref(auctionId: string, worldId: string): string {
  const query = new URLSearchParams();
  query.append("auction", auctionId);
  if (worldId !== "main") {
    query.append("world", worldId);
  }
  return `/?${query.toString()}`;
}
