// Admin 用户管理页面。
// 页面维护筛选、表单与安全用户投影；会话命令宿主冻结完整负载和 command_id。

import { usePopstateRevision } from "@auction/web-shared/net/query";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { err } from "@auction/web-shared/result";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { createEffect, Match, Show, Switch, untrack } from "solid-js";

import { type AdminUserStatusFilter, fetchAdminUsers } from "../../api/users";
import { useAdminSessionHost } from "../auth/shell";
import { useAdminPagination, useVersionedAdminPage } from "../pagination";
import { PendingCommandsContext, usePendingCommandGate } from "../pending";
import { PendingCommandPanel } from "../pending/panel";
import {
  advanceCommandReload,
  createCommandReload,
  usePendingCommandsWithGate,
} from "../pending/sources";
import { type AdminActionFeedback, adminApiErrorMessage } from "../presentation";
import {
  usersQueryApply,
  usersQueryCorrectPage,
  usersQueryPageCorrection,
  useUsersQueryState,
} from "../query/users";
import type { SortDirection } from "../sort";
import { AdminUserCreateForm, AdminUserGeneratedKeyDialog } from "./create";
import { AdminUserFilters, AdminUserTable } from "./table";

/** 改名回滚和用户页的动作共用用户的实体围栏。 */
export function userCommandEntity(userId: string): string {
  return `user:${userId}`;
}

/** 加载并展示 Admin 用户管理页面。 */
export function page(): JSX.Element {
  const host = useAdminSessionHost();
  // 已应用的名称筛选、状态筛选与分页跟地址栏双向同步；名称输入框草稿只在提交时入 URL。
  const query = useUsersQueryState();
  const queryState = query.state;
  const historyRevision = createCell(0);
  usePopstateRevision(historyRevision);
  const requestedQuery = createValueMemo(() => {
    const current = queryState();
    return [current.name, current.status, current.page] as const;
  });
  // 主动排序与翻页保留草稿；历史导航用独立版本恢复地址里的文本，即使筛选内容相同。
  const appliedFilter = createValueMemo(() => {
    const current = queryState();
    return [current.name, current.status] as const;
  });
  const reloadGeneration = createCommandReload(0n);
  const createOpen = createCell(false);
  const actionBusy = createCell(false);
  const commandGate = usePendingCommandGate();
  const pending = usePendingCommandsWithGate(reloadGeneration, commandGate, (entity) => {
    return entity === "create-user" || entity.startsWith("user:");
  });
  // effect 仅用于既有控件的忙态展示；首次提交/重试在执行边界同步占用同一门禁。
  createEffect(
    () => pending.anyBusy() || commandGate.blocked(),
    (busy) => actionBusy.set(busy),
  );
  const actionFeedback = createCell<AdminActionFeedback | null>(null);
  const createNotice = createCell<string | null>(null);
  const pagination = useAdminPagination("users", ["page_size"], 200, 2_147_483_647);
  const users = useVersionedAdminPage(
    pagination,
    // 分页 helper 的本地世代仍是安全整数；页面的 u64 世代由 bigint 持有并在这里精确转换。
    () => Number(reloadGeneration()),
    () => requestedQuery(),
    async (request, size, signal) => {
      const requestedPage = request[2];
      if (requestedPage > BigInt(Number.MAX_SAFE_INTEGER)) {
        return err("admin_pagination_bounds");
      }
      try {
        return await fetchAdminUsers(
          host,
          request[0],
          request[1],
          Number(requestedPage),
          size,
          signal,
        );
      } catch {
        // 读取异常按稳定传输失败收尾，现有错误视图与重试入口继续生效。
        return err("network_error");
      }
    },
  );

  const correctionFailed = createCell(false);
  const correctPage = (): boolean => {
    const current = untrack(() => users());
    const failed =
      current?.ok === true
        ? !usersQueryCorrectPage(queryState.peek(), query, current.value[1].pagination)
        : false;
    correctionFailed.set(failed);
    return failed;
  };
  createEffect(
    () => users(),
    () => {
      correctPage();
    },
  );
  // 用户页先按权威页数纠偏，避免旧页交给仍把纠偏当作翻页的 AdminPager 包装。
  const needsCorrection = createValueMemo(() => {
    const current = users();
    if (current === null || !current.ok) {
      return false;
    }
    return usersQueryPageCorrection(queryState(), current.value[1].pagination) !== null;
  });
  const visible = createValueMemo(() => (needsCorrection() ? null : users()));
  const ready = createValueMemo(() => {
    const current = visible();
    if (current === null || !current.ok) {
      return null;
    }
    // 读取 helper 以 number 标记本次装载；表格与行统一按 bigint 比较世代。
    return { generation: BigInt(current.value[0]), page: current.value[1] };
  });
  const failure = createValueMemo(() => {
    const current = visible();
    return current !== null && !current.ok ? { code: current.error } : null;
  });
  const feedbackError = createValueMemo(() => {
    const current = actionFeedback();
    return current !== null && current.kind === "error" ? current : null;
  });

  const applyName = (name: string): void => {
    const next = { ...queryState() };
    if (next.name === name && next.page === 1n) {
      advanceCommandReload(reloadGeneration);
    } else {
      next.name = name;
      next.page = 1n;
      usersQueryApply(next, query);
    }
  };
  const applyStatus = (status: AdminUserStatusFilter): void => {
    usersQueryApply({ ...queryState(), status, page: 1n }, query);
  };
  const changePage = (nextPage: number): void => {
    usersQueryApply({ ...queryState(), page: BigInt(nextPage) }, query);
  };
  const changeSort = (value: readonly [string, SortDirection]): void => {
    usersQueryApply({ ...queryState(), sort: value[0], direction: value[1] }, query);
  };

  return (
    <PendingCommandsContext value={pending}>
      <section class="admin-users-page" aria-labelledby="admin-users-title">
        <header class="admin-users-heading">
          <h1 id="admin-users-title">{t("page-users-title")}</h1>
          <button
            class="admin-btn-primary"
            type="button"
            disabled={commandGate.blocked() || pending.contains("create-user")}
            onClick={() => {
              if (commandGate.blocked() || pending.contains("create-user")) {
                return;
              }
              actionFeedback.set(null);
              createNotice.set(null);
              createOpen.set(true);
            }}
          >
            {t("admin-users-create-open")}
          </button>
        </header>
        <Show when={commandGate.blocked()}>
          <p role="status">{t("admin-users-key-gate-help")}</p>
        </Show>
        <Show when={createOpen()}>
          <AdminUserCreateForm gate={commandGate} open={createOpen} notice={createNotice} />
        </Show>
        <PendingCommandPanel owner={pending} />
        {/* 用户列表可以读失败，已交付的写入结果跟随页面。 */}
        <div role="status">
          <Show when={actionFeedback()?.kind === "success"}>
            <p class="action-feedback action-feedback--success">{t("admin-users-updated")}</p>
          </Show>
          <Show when={createNotice()}>
            {(username) => (
              <p class="action-feedback action-feedback--success">
                {t("admin-users-created", { username: username() })}
              </p>
            )}
          </Show>
        </div>
        <Show when={feedbackError()}>
          {(error) => (
            <p class="action-feedback action-feedback--error" role="alert">
              {adminApiErrorMessage(error().code)}
            </p>
          )}
        </Show>
        <AdminUserFilters
          applied={appliedFilter}
          historyRevision={historyRevision}
          onNameApply={applyName}
          onStatusChange={applyStatus}
        />
        <Switch fallback={<AsyncState kind="loading" title={t("admin-users-loading")} detail="" />}>
          <Match when={needsCorrection() && correctionFailed()}>
            <AsyncState
              kind="error"
              title={t("admin-users-page-correction-failed")}
              detail=""
              retryLabel={t("admin-retry")}
              onRetry={correctPage}
            />
          </Match>
          <Match when={failure()}>
            {(current) => (
              <AsyncState
                kind="error"
                title={adminApiErrorMessage(current().code)}
                detail=""
                retryLabel={t("admin-retry")}
                onRetry={() => advanceCommandReload(reloadGeneration)}
              />
            )}
          </Match>
          <Match when={ready()}>
            {(current) => (
              <AdminUserTable
                page={current().page}
                pagination={pagination}
                sort={queryState().sort}
                direction={queryState().direction}
                onPageChange={changePage}
                onSortChange={changeSort}
                reloadGeneration={reloadGeneration}
                loadedGeneration={current().generation}
                actionBusy={actionBusy}
                actionFeedback={actionFeedback}
              />
            )}
          </Match>
        </Switch>
      </section>
      <Show when={pending.generatedKey()}>
        {(generated) => (
          <AdminUserGeneratedKeyDialog
            generated={generated().key}
            onClose={() => pending.acknowledgeKey(generated().commandId)}
          />
        )}
      </Show>
    </PendingCommandsContext>
  );
}
