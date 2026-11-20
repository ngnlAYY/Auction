// Admin lot 的代理竞价历史弹窗与分页。

import type { Cell } from "@auction/web-shared/reactive/cell";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { Dialog } from "@auction/web-shared/ui/components/dialog";
import { TableColumns } from "@auction/web-shared/ui/components/table-columns";
import { Time } from "@auction/web-shared/ui/components/time";
import { TruncatedText } from "@auction/web-shared/ui/components/truncated-text";
import { formatCreditAmount } from "@auction/web-shared/ui/format";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Match, Show, Switch } from "solid-js";

import {
  type AdminProxyEvent,
  type AdminProxyEventPage,
  fetchAdminProxyEvents,
} from "../../api/proxy-events";
import { useAdminSessionHost } from "../auth/shell";
import { AdminPager, type AdminPagination, useAdminPage, useAdminPagination } from "../pagination";
import { advanceCommandReload } from "../pending/sources";
import { adminApiErrorMessage } from "../presentation";
import { proxyModeLabel, proxyStatusLabel } from "./presentation";

/** 代理历史弹窗的拍品身份、页面共享的选择状态与重读世代。 */
export interface AdminProxyHistoryProps {
  readonly lotId: string;
  readonly lotCode: string;
  readonly selectedLot: Cell<readonly [string, string] | null>;
  readonly reloadGeneration: Cell<bigint>;
}

type ProxyView =
  | { readonly kind: "loading" }
  | { readonly kind: "error"; readonly code: string }
  | { readonly kind: "ready"; readonly page: AdminProxyEventPage };

/** 拍品的代理竞价历史；弹窗打开期间按页读取并允许重试。 */
export function AdminProxyHistory(props: AdminProxyHistoryProps): JSX.Element {
  const host = useAdminSessionHost();
  const pageNumber = createCell(1);
  const pagination = useAdminPagination("proxy-events", ["proxy_page_size"], 100, 100_000);
  const events = useAdminPage(
    pagination,
    () => {
      props.reloadGeneration();
      return [props.lotId, pageNumber()] as const;
    },
    (request, size, signal) => fetchAdminProxyEvents(host, request[0], request[1], size, signal),
  );
  const view = createValueMemo((): ProxyView => {
    const current = events();
    if (current === null) {
      return { kind: "loading" };
    }
    if (!current.ok) {
      return { kind: "error", code: current.error };
    }
    return { kind: "ready", page: current.value };
  });
  const loadError = createValueMemo(() => {
    const current = view();
    return current.kind === "error" ? current : null;
  });
  const readyPage = createValueMemo(() => {
    const current = view();
    return current.kind === "ready" ? current.page : null;
  });

  return (
    // 外壳（遮罩/标题栏/关闭守卫）由共享 Dialog 接管；.admin-auction-proxy 内容网格
    // 样式挂在面板内的包装 div 上。
    <Dialog
      title={t("admin-lots-proxy-title", { lot: props.lotCode })}
      closeLabel={t("admin-lots-proxy-close")}
      onClose={() => props.selectedLot.set(null)}
    >
      <div class="admin-auction-proxy">
        <Switch>
          <Match when={view().kind === "loading"}>
            <AsyncState kind="loading" title={t("admin-lots-proxy-loading")} detail="" />
          </Match>
          <Match when={loadError()}>
            {(error) => (
              <AsyncState
                kind="error"
                title={adminApiErrorMessage(error().code)}
                detail=""
                retryLabel={t("admin-lots-retry")}
                onRetry={() => advanceCommandReload(props.reloadGeneration)}
              />
            )}
          </Match>
          <Match when={readyPage()}>
            {(page) => (
              <AdminProxyHistoryTable
                page={page()}
                pageNumber={pageNumber}
                pagination={pagination}
              />
            )}
          </Match>
        </Switch>
        <div class="admin-shell__dialog-actions">
          <button type="button" autofocus onClick={() => props.selectedLot.set(null)}>
            {t("admin-lots-proxy-close")}
          </button>
        </div>
      </div>
    </Dialog>
  );
}

interface AdminProxyHistoryTableProps {
  readonly page: AdminProxyEventPage;
  readonly pageNumber: Cell<number>;
  readonly pagination: AdminPagination;
}

function AdminProxyHistoryTable(props: AdminProxyHistoryTableProps): JSX.Element {
  const currentPage = createValueMemo(() =>
    Math.max(props.page.pagination.page ?? props.pageNumber(), 1),
  );
  const pageSize = createValueMemo(() => Math.max(props.page.pagination.page_size, 1));
  const total = createValueMemo(() => props.page.pagination.total ?? null);
  const hasMore = createValueMemo(
    () => props.page.pagination.has_more ?? props.page.pagination.next_cursor != null,
  );
  const empty = createValueMemo(() => props.page.proxy_events.length === 0);

  return (
    <>
      <Show
        when={!empty()}
        fallback={<AsyncState kind="empty" title={t("admin-lots-proxy-empty")} detail="" />}
      >
        <div class="admin-auction-table-wrap">
          <table class="admin-auction-table" aria-label={t("admin-lots-proxy-pagination")}>
            <TableColumns layout="admin-proxy-history" widths={[22, 18, 10, 12, 10, 28]} />
            <thead>
              <tr>
                <th scope="col">{t("admin-lots-proxy-user")}</th>
                <th scope="col">{t("admin-lots-proxy-maximum")}</th>
                <th scope="col">{t("admin-lots-proxy-mode")}</th>
                <th scope="col">{t("admin-lots-proxy-status")}</th>
                <th scope="col">{t("admin-lots-proxy-usage")}</th>
                <th scope="col">{t("admin-lots-proxy-created")}</th>
              </tr>
            </thead>
            <tbody>
              <For each={props.page.proxy_events} keyed={false}>
                {(event) => <AdminProxyHistoryRow event={event()} />}
              </For>
            </tbody>
          </table>
        </div>
      </Show>
      <AdminPager
        pagination={props.pagination}
        local={true}
        page={currentPage()}
        pageSize={pageSize()}
        total={total()}
        hasMore={hasMore()}
        label={t("admin-lots-proxy-pagination")}
        onPageChange={(next) => props.pageNumber.set(next)}
      />
    </>
  );
}

function AdminProxyHistoryRow(props: { readonly event: AdminProxyEvent }): JSX.Element {
  return (
    <tr>
      <td data-label={t("admin-lots-proxy-user")}>
        <strong>
          <TruncatedText value={props.event.username} />
        </strong>
      </td>
      <td data-label={t("admin-lots-proxy-maximum")}>
        {t("admin-lots-proxy-amount", {
          amount: formatCreditAmount(props.event.maximum_amount),
        })}
      </td>
      <td data-label={t("admin-lots-proxy-mode")}>{proxyModeLabel(props.event.mode)}</td>
      <td data-label={t("admin-lots-proxy-status")}>{proxyStatusLabel(props.event.status)}</td>
      <td data-label={t("admin-lots-proxy-usage")} title={props.event.authorization_id}>
        {t("admin-lots-proxy-identity", {
          authorization_sequence: props.event.authorization_sequence,
          usage_number: props.event.usage_number,
        })}
      </td>
      <td data-label={t("admin-lots-proxy-created")}>
        <Time value={props.event.created_at} />
      </td>
    </tr>
  );
}
