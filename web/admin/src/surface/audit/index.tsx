// Admin 审计历史页面。
// 审计事实只读且追加式；筛选和分页请求通过 HTTP 安全 DTO 完成，浏览器不修改记录、不接触
// 数据库模型，也不把 metadata 当作可执行内容。

import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { err } from "@auction/web-shared/result";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { Match, Show, Switch } from "solid-js";

import type { AdminAuditFilter, AdminAuditPage } from "../../api/audit";
import { fetchAdminAudit } from "../../api/audit";
import { useAdminSessionHost } from "../auth/shell";
import { AdminPager, useAdminPage, useAdminPagination } from "../pagination";
import { advanceCommandReload, createCommandReload } from "../pending/sources";
import { adminApiErrorMessage } from "../presentation";
import { auditQueryPageRequest, auditQueryWithFilter, useAuditQueryState } from "../query/audit";
import { fetchFiltered } from "../realtime/filter-field";
import type { SortDirection } from "../sort";
import { AuditFilters } from "./filters";
import { AuditTable } from "./table";

export type { AuditFilterState } from "./filter-state";

type AuditView =
  | { readonly kind: "loading" }
  | { readonly kind: "error"; readonly error: string }
  | { readonly kind: "ready"; readonly page: AdminAuditPage };

/** 加载并展示审计历史安全投影；筛选、排序与分页跟地址栏双向同步。 */
export function page(): JSX.Element {
  const host = useAdminSessionHost();
  // 已应用的筛选与分页跟地址栏双向同步；表单草稿是提交前的瞬态，只在提交时入 URL。
  const query = useAuditQueryState();
  const queryState = query.state;
  const requestedQuery = createValueMemo(() => auditQueryPageRequest(queryState()));
  const appliedFilter = createValueMemo(() => queryState().filter);
  const reload = createCommandReload(0n);
  const pagination = useAdminPagination("audit", ["page_size"], 200, 2_147_483_647);
  const audit = useAdminPage(
    pagination,
    () => {
      reload();
      return requestedQuery();
    },
    (request: readonly [bigint, AdminAuditFilter] | null, size: number, signal: AbortSignal) => {
      return fetchFiltered(request, ([pageNumber, filter]) => {
        if (pageNumber > BigInt(Number.MAX_SAFE_INTEGER)) {
          return Promise.resolve(err("admin_pagination_bounds"));
        }
        return fetchAdminAudit(host, Number(pageNumber), filter, size, signal);
      });
    },
  );

  const view = createValueMemo((): AuditView => {
    const current = audit();
    if (current === null) {
      return { kind: "loading" };
    }
    if (!current.ok) {
      return { kind: "error", error: current.error };
    }
    return { kind: "ready", page: current.value };
  });
  const failure = createValueMemo(() => {
    const current = view();
    return current.kind === "error" ? current : null;
  });
  const ready = createValueMemo(() => {
    const current = view();
    return current.kind === "ready" ? current.page : null;
  });

  const applyFilter = (nextFilter: AdminAuditFilter): void => {
    // 提交筛选回到第一页并保留排序；查询资源响应状态变化自动重新加载。
    query.push(auditQueryWithFilter(queryState(), nextFilter));
  };

  return (
    <section class="admin-audit-page" aria-labelledby="admin-audit-title">
      <h1 id="admin-audit-title">{t("page-audit-title")}</h1>
      <AuditFilters applied={appliedFilter()} onSubmit={applyFilter} />
      <Show when={requestedQuery() !== null}>
        <Switch fallback={null}>
          <Match when={view().kind === "loading"}>
            <AsyncState kind="loading" title={t("admin-audit-loading")} detail="" />
          </Match>
          <Match when={failure()}>
            {(current) => (
              <AsyncState
                kind="error"
                title={adminApiErrorMessage(current().error)}
                detail=""
                retryLabel={t("admin-audit-retry")}
                onRetry={() => advanceCommandReload(reload)}
              />
            )}
          </Match>
          <Match when={ready()}>
            {(current) => (
              <>
                <AuditTable
                  page={current()}
                  sort={queryState().sort}
                  direction={queryState().direction}
                  onSortChange={(value: [string, SortDirection]) => {
                    const [sort, direction] = value;
                    query.push({ ...queryState(), sort, direction });
                  }}
                />
                <AdminPager
                  pagination={pagination}
                  page={Math.max(1, current().page ?? Number(queryState().page))}
                  pageSize={current().page_size}
                  total={current().total}
                  hasMore={current().has_more}
                  label={t("page-audit-title")}
                  onPageChange={(nextPage) => {
                    query.push({ ...queryState(), page: BigInt(nextPage) });
                  }}
                />
              </>
            )}
          </Match>
        </Switch>
      </Show>
    </section>
  );
}
