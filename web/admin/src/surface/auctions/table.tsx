// Admin 拍卖列表表格与筛选器。

import type { Cell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import type { Result } from "@auction/web-shared/result";
import { compareText } from "@auction/web-shared/text";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { TableColumns } from "@auction/web-shared/ui/components/table-columns";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Match, Show, Switch } from "solid-js";

import type { AdminAuction, AdminAuctionPage } from "../../api/auctions";
import { AdminIntakeForm } from "../intake";
import { AdminPager, type AdminPagination } from "../pagination";
import { requirePendingCommands } from "../pending";
import { adminWorldLabel } from "../presentation";
import type { AuctionsQuery } from "../query/auctions";
import { MobileSortControl, SortableTh, type SortDirection, sortRows } from "../sort";
import { type AdminAuctionNotice, auctionNoticeLabel } from "./actions/model";
import { AdminAuctionRow } from "./actions/row";
import { AdminAuctionCreateForm } from "./create";
import { AdminAuctionForumStatus } from "./forum";

export interface AdminAuctionTableProps {
  readonly state: Result<AdminAuctionPage, string> | null;
  readonly pagination: AdminPagination;
  readonly editsReady: boolean;
  /** 当前世界筛选（地址栏恢复后的值）；变更经 `onQueryChange` 由页面写回 URL。 */
  readonly world: string;
  /** 当前排序列（空串 = 服务端原始顺序）与方向；变更经 `onSortChange` 由页面写回 URL。 */
  readonly sort: string;
  readonly direction: SortDirection;
  readonly onQueryChange: (query: AuctionsQuery) => void;
  readonly onRetry: () => void;
  readonly onSortChange: (value: readonly [string, SortDirection]) => void;
  readonly notice: Cell<AdminAuctionNotice | null>;
}

type AuctionsView =
  | { readonly kind: "loading" }
  | { readonly kind: "error"; readonly code: string }
  | { readonly kind: "ready"; readonly page: AdminAuctionPage };

export function AdminAuctionTable(props: AdminAuctionTableProps): JSX.Element {
  const owner = requirePendingCommands();
  // 秒级时钟只在行内就绪与动作叶子单元格订阅（见 row.tsx/dialog.tsx），整表不随秒针重渲染。
  const intakeBusy = () => owner.isBusy(`intake:${props.world}`);
  const view = createValueMemo((): AuctionsView => {
    const state = props.state;
    if (state === null) {
      return { kind: "loading" };
    }
    if (!state.ok) {
      return { kind: "error", code: state.error };
    }
    return { kind: "ready", page: state.value };
  });
  const readyPage = createValueMemo(() => {
    const current = view();
    return current.kind === "ready" ? current.page : null;
  });
  const loadError = createValueMemo(() => {
    const current = view();
    return current.kind === "error" ? current : null;
  });
  return (
    <section class="admin-auction-page" aria-labelledby="auction-title">
      <h1 id="auction-title">{t("page-auctions-title")}</h1>
      <div class="admin-auction-filter">
        <form onSubmit={(event) => event.preventDefault()}>
          <label for="auction-world">
            {t("admin-auctions-world")}
            <select
              class="admin-auction-world"
              id="auction-world"
              disabled={intakeBusy()}
              value={props.world}
              onChange={(event) => {
                if (intakeBusy()) {
                  return;
                }
                // 切换世界筛选回到第一页并保留排序，由页面 push 到地址栏。
                props.onQueryChange({
                  world: event.currentTarget.value,
                  page: 1n,
                  sort: props.sort,
                  direction: props.direction,
                });
              }}
            >
              <option value="">{t("admin-auctions-world-all")}</option>
              <option value="main">{adminWorldLabel("main")}</option>
              <option value="isekai">{adminWorldLabel("isekai")}</option>
            </select>
          </label>
        </form>
        <AdminAuctionCreateForm notice={props.notice} />
      </div>
      <Show when={props.notice()}>
        {(notice) => (
          <p class="action-feedback action-feedback--success" role="status">
            {auctionNoticeLabel(notice())}
          </p>
        )}
      </Show>
      <Switch
        fallback={
          <Show when={readyPage()}>
            {(page) => (
              <AdminAuctionTableContent
                page={page()}
                pagination={props.pagination}
                editsReady={props.editsReady}
                world={props.world}
                sort={props.sort}
                direction={props.direction}
                onQueryChange={props.onQueryChange}
                onSortChange={props.onSortChange}
                notice={props.notice}
              />
            )}
          </Show>
        }
      >
        <Match when={view().kind === "loading"}>
          <AsyncState kind="loading" title={t("admin-auctions-loading")} detail="" />
        </Match>
        <Match when={loadError()}>
          {(error) => (
            <AsyncState
              kind="error"
              title={error().code}
              detail=""
              retryLabel={t("admin-retry")}
              onRetry={() => props.onRetry()}
            />
          )}
        </Match>
      </Switch>
    </section>
  );
}

interface AdminAuctionTableContentProps {
  readonly page: AdminAuctionPage;
  readonly pagination: AdminPagination;
  readonly editsReady: boolean;
  readonly world: string;
  readonly sort: string;
  readonly direction: SortDirection;
  readonly onQueryChange: (query: AuctionsQuery) => void;
  readonly onSortChange: (value: readonly [string, SortDirection]) => void;
  readonly notice: Cell<AdminAuctionNotice | null>;
}

function AdminAuctionTableContent(props: AdminAuctionTableContentProps): JSX.Element {
  const isEmpty = () => props.page.auctions.length === 0;
  const auctions = createValueMemo(() => {
    const rows = [...props.page.auctions];
    sortAuctions(rows, props.sort, props.direction);
    return rows;
  });
  const columns = createValueMemo((): readonly (readonly [string, string])[] => [
    ["sequence", t("admin-auctions-sequence")],
    ["status", t("admin-auctions-status")],
    ["open_at", t("admin-auctions-open-at")],
    ["end_at", t("admin-auctions-end-at")],
  ]);
  return (
    <>
      <Show
        when={
          props.page.capabilities.manual_intake &&
          (props.world === "main" || props.world === "isekai")
        }
      >
        <For each={[props.world]} keyed={(worldId) => worldId}>
          {(worldId) => <AdminIntakeForm worldId={worldId()} />}
        </For>
      </Show>
      <Show when={!isEmpty()}>
        <AdminAuctionForumStatus auctions={props.page.auctions} />
      </Show>
      <MobileSortControl
        columns={columns()}
        sort={props.sort}
        direction={props.direction}
        onSortChange={(value) => props.onSortChange(value)}
      />
      <div class="admin-auction-table-wrap">
        <table class="admin-auction-table" aria-label={t("page-auctions-title")}>
          <TableColumns layout="admin-auctions" widths={[6, 6, 8, 18, 18, 15, 29]} />
          <thead>
            <tr>
              <SortableTh
                column="sequence"
                label={t("admin-auctions-sequence")}
                active={props.sort}
                direction={props.direction}
                onSort={(value) => props.onSortChange(value)}
              />
              <th scope="col">{t("admin-auctions-world")}</th>
              <SortableTh
                column="status"
                label={t("admin-auctions-status")}
                active={props.sort}
                direction={props.direction}
                onSort={(value) => props.onSortChange(value)}
              />
              <SortableTh
                column="open_at"
                label={t("admin-auctions-open-at")}
                active={props.sort}
                direction={props.direction}
                onSort={(value) => props.onSortChange(value)}
              />
              <SortableTh
                column="end_at"
                label={t("admin-auctions-end-at")}
                active={props.sort}
                direction={props.direction}
                onSort={(value) => props.onSortChange(value)}
              />
              <th scope="col">{t("admin-auctions-readiness")}</th>
              <th scope="col">{t("admin-auctions-actions")}</th>
            </tr>
          </thead>
          <tbody>
            <For each={auctions()} keyed={(auction) => auction.id}>
              {(auction) => (
                <AdminAuctionRow
                  auction={auction()}
                  capabilities={props.page.capabilities}
                  editsReady={props.editsReady}
                  notice={props.notice}
                />
              )}
            </For>
          </tbody>
        </table>
      </div>
      <Show when={isEmpty()}>
        <AsyncState kind="empty" title={t("admin-auctions-empty")} detail="" />
      </Show>
      <AdminPager
        pagination={props.pagination}
        page={props.page.pagination.page}
        pageSize={props.page.pagination.page_size}
        total={props.page.pagination.total}
        label={t("admin-auctions-pagination")}
        onPageChange={(nextPage) => {
          // 翻页保留当前世界筛选与排序。
          props.onQueryChange({
            world: props.world,
            page: BigInt(nextPage),
            sort: props.sort,
            direction: props.direction,
          });
        }}
      />
    </>
  );
}

/** 对当前页拍卖行排序；`column` 为空或不在允许集合时保持服务端原始顺序。 */
function sortAuctions(auctions: AdminAuction[], column: string, direction: SortDirection): void {
  sortRows(auctions, column, direction, (left, right, current) => {
    switch (current) {
      case "sequence":
        return compareNumbers(left.sequence, right.sequence);
      case "status":
        return compareText(left.status, right.status);
      case "open_at":
        return compareText(left.scheduled_open_at, right.scheduled_open_at);
      case "end_at":
        return compareOptionalTimestamps(
          left.scheduled_ends_at ?? null,
          right.scheduled_ends_at ?? null,
        );
      default:
        return 0;
    }
  });
}

function compareNumbers(left: number, right: number): number {
  return left < right ? -1 : left > right ? 1 : 0;
}

function compareOptionalTimestamps(left: string | null, right: string | null): number {
  if (left === null && right === null) {
    return 0;
  }
  if (left === null) {
    return -1;
  }
  if (right === null) {
    return 1;
  }
  return compareText(left, right);
}
