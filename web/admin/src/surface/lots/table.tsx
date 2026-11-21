// Admin lot 表格和页面级分页编排。

import type { Cell } from "@auction/web-shared/reactive/cell";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { Link } from "@auction/web-shared/router";
import { compareText } from "@auction/web-shared/text";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { TableColumns } from "@auction/web-shared/ui/components/table-columns";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Show } from "solid-js";

import type { AdminLot, AdminLotPage } from "../../api/lots";
import { AdminIntakeForm } from "../intake";
import { AdminPager, type AdminPagination } from "../pagination";
import { MobileSortControl, SortableTh, type SortDirection, sortRows } from "../sort";
import type { LotEdit } from "./edit";
import { AdminProxyHistory } from "./proxy";
import { AdminLotRow } from "./row";
import type { LotUpdateNotice } from "./state";

/** 当前页 lot 投影、分页 owner、排序状态与页面共享的草稿/通知。 */
export interface AdminLotTableProps {
  readonly page: AdminLotPage;
  readonly pagination: AdminPagination;
  readonly editsReady: boolean;
  /** 当前排序列（空串 = 服务端原始顺序）与方向；变更经 `onSortChange` 由页面写回 URL。 */
  readonly sort: string;
  readonly direction: SortDirection;
  /** 翻页回调：由页面把新页码 push 到地址栏并更新查询状态。 */
  readonly onPageChange: (page: number) => void;
  readonly onSortChange: (value: readonly [string, SortDirection]) => void;
  readonly reloadGeneration: Cell<bigint>;
  readonly draft: Cell<LotEdit | null>;
  readonly notice: Cell<LotUpdateNotice | null>;
}

/** 拍品列表：手工录入入口、桌面表头、排序、翻页与代理历史弹窗。 */
export function AdminLotTable(props: AdminLotTableProps): JSX.Element {
  const selectedLot = createCell<readonly [string, string] | null>(null);
  const sortedLots = createValueMemo(() => {
    const lots = [...props.page.lots];
    sortLots(lots, props.sort, props.direction);
    return lots;
  });
  const isEmpty = createValueMemo(() => props.page.lots.length === 0);
  const selectedPairs = createValueMemo(() => {
    const selected = selectedLot();
    return selected === null || !props.page.lots.some((lot) => lot.id === selected[0])
      ? []
      : [selected];
  });

  return (
    <section class="admin-auction-page" aria-labelledby="auction-title">
      <h1 id="auction-title">
        {t("page-auction-lots-title", { auction: props.page.auction.sequence })}
      </h1>
      <p class="admin-auction-intro">
        <Link href="/auctions">{t("admin-lots-back")}</Link>
      </p>
      <section>
        <Show
          when={
            (props.page.auction.status === "draft" || props.page.auction.status === "preview") &&
            props.page.capabilities.manualIntake
          }
        >
          <For each={[props.page.auction.world_id]} keyed={(worldId) => worldId}>
            {(worldId) => <AdminIntakeForm worldId={worldId()} />}
          </For>
        </Show>
        <AdminLotMobileSort
          sort={props.sort}
          direction={props.direction}
          onSortChange={props.onSortChange}
        />
        <div class="admin-auction-table-wrap admin-auction-lot-table-wrap">
          <table
            class="admin-auction-table admin-auction-lot-table"
            aria-label={t("page-auction-lots-title", { auction: props.page.auction.sequence })}
          >
            <TableColumns layout="admin-lots" widths={[7, 28, 10, 7, 9, 9, 12, 18]} />
            <thead>
              <tr>
                <SortableTh
                  column="code"
                  label={t("admin-lots-code")}
                  active={props.sort}
                  direction={props.direction}
                  onSort={props.onSortChange}
                  omitInactiveAriaSort={true}
                />
                <th scope="col">{t("admin-lots-equipment")}</th>
                <th scope="col">{t("admin-lots-seller")}</th>
                <th scope="col">{t("admin-lots-submission")}</th>
                <SortableTh
                  column="status"
                  label={t("admin-lots-status")}
                  active={props.sort}
                  direction={props.direction}
                  onSort={props.onSortChange}
                  omitInactiveAriaSort={true}
                />
                <SortableTh
                  column="start_price"
                  label={t("admin-lots-start-price")}
                  active={props.sort}
                  direction={props.direction}
                  onSort={props.onSortChange}
                  omitInactiveAriaSort={true}
                  class="admin-auction-lot-start"
                />
                <SortableTh
                  column="current_price"
                  label={t("admin-lots-price")}
                  active={props.sort}
                  direction={props.direction}
                  onSort={props.onSortChange}
                  omitInactiveAriaSort={true}
                  class="admin-auction-lot-current"
                />
                <th scope="col" class="admin-auction-lot-actions">
                  {t("admin-lots-actions")}
                </th>
              </tr>
            </thead>
            <tbody>
              <For each={sortedLots()} keyed={(lot) => lot.id}>
                {(lot) => (
                  // 按业务 ID 固定 key，避免刷新/重排后行内本地状态错位绑定到另一行。
                  <AdminLotRow
                    lot={lot()}
                    selectedLot={selectedLot}
                    notice={props.notice}
                    allowLotUpdate={props.page.auction.status === "draft"}
                    editsReady={props.editsReady}
                    draft={props.draft}
                  />
                )}
              </For>
            </tbody>
          </table>
        </div>
        <Show when={isEmpty()}>
          <AsyncState kind="empty" title={t("admin-lots-empty")} detail="" />
        </Show>
        <AdminPager
          pagination={props.pagination}
          page={props.page.pagination.page}
          pageSize={props.page.pagination.page_size}
          total={props.page.pagination.total}
          label={t("admin-lots-pagination")}
          onPageChange={(next) => props.onPageChange(next)}
        />
      </section>
      <For each={selectedPairs()} keyed={(pair) => pair[0]}>
        {(pair) => (
          <AdminProxyHistory
            lotId={pair()[0]}
            lotCode={pair()[1]}
            selectedLot={selectedLot}
            reloadGeneration={props.reloadGeneration}
          />
        )}
      </For>
    </section>
  );
}

/** 对当前页 lot 行排序；`column` 为空或不在允许集合时保持服务端原始顺序。 */
function sortLots(lots: AdminLot[], column: string, direction: SortDirection): void {
  sortRows(lots, column, direction, (left, right, sortColumn) => {
    switch (sortColumn) {
      case "code":
        return compareInteger(left.lot_number, right.lot_number);
      case "status":
        return compareText(left.status, right.status);
      case "start_price":
        return compareInteger(left.start_price, right.start_price);
      case "current_price":
        return compareInteger(left.current_price, right.current_price);
      default:
        return 0;
    }
  });
}

function compareInteger(left: number, right: number): number {
  if (left < right) {
    return -1;
  }
  if (left > right) {
    return 1;
  }
  return 0;
}

function AdminLotMobileSort(props: {
  readonly sort: string;
  readonly direction: SortDirection;
  readonly onSortChange: (value: readonly [string, SortDirection]) => void;
}): JSX.Element {
  const columns = createValueMemo<readonly (readonly [string, string])[]>(() => [
    ["code", t("admin-lots-code")],
    ["status", t("admin-lots-status")],
    ["start_price", t("admin-lots-start-price")],
    ["current_price", t("admin-lots-price")],
  ]);
  return (
    <MobileSortControl
      columns={columns()}
      sort={props.sort}
      direction={props.direction}
      onSortChange={props.onSortChange}
      defaultOrderLabel={t("admin-sort-default-order")}
    />
  );
}
