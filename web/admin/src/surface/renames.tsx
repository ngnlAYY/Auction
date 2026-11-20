// Admin 改名历史页面。
// 页面沿用旧版的名称即时筛选、列表和页级确认对话框；回滚仍通过新版命令 API、当前用户
// 版本和 CSRF 边界完成，浏览器不自行推导身份状态迁移。

import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { err, type Result } from "@auction/web-shared/result";
import { compareText } from "@auction/web-shared/text";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { Dialog } from "@auction/web-shared/ui/components/dialog";
import { TableColumns } from "@auction/web-shared/ui/components/table-columns";
import { Time } from "@auction/web-shared/ui/components/time";
import { TruncatedText } from "@auction/web-shared/ui/components/truncated-text";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Match, onCleanup, Show, Switch } from "solid-js";

import type { AdminRename, AdminRenamePage, AdminRenameRevertCommand } from "../api/renames";
import { fetchAdminRenames, prepareAdminRenameRevert, revertAdminRename } from "../api/renames";
import { useAdminSessionHost } from "./auth/shell";
import { AdminPager, useAdminPage, useAdminPagination } from "./pagination";
import {
  PendingCommandsContext,
  requirePendingCommands,
  setIfAlive,
  usePendingCommandGate,
} from "./pending";
import { PendingCommandPanel } from "./pending/panel";
import {
  advanceCommandReload,
  createCommandReload,
  usePendingCommandsWithGate,
} from "./pending/sources";
import {
  type AdminActionFeedback,
  adminActionFeedbackMessage,
  adminApiErrorMessage,
} from "./presentation";
import { useRenamesQueryState } from "./query/renames";
import { SortableTh, type SortDirection, sortRows } from "./sort";
import { userCommandEntity } from "./users";

type RenamesView =
  | { readonly kind: "loading" }
  | { readonly kind: "error"; readonly code: string }
  | { readonly kind: "ready"; readonly page: AdminRenamePage };

/** 加载并展示改名历史安全投影；名称筛选与排序跟地址栏双向同步。 */
export function page(): JSX.Element {
  // 名称即时筛选与分页跟地址栏双向同步；回滚确认对话框和命令上下文是瞬态，不入 URL。
  const host = useAdminSessionHost();
  const query = useRenamesQueryState();
  const queryState = query.state;
  const requestedQuery = createValueMemo(() => {
    const current = queryState();
    return [current.name, current.page] as const;
  });
  const reload = createCommandReload(0n);
  const notice = createCell<AdminActionFeedback | null>(null);
  const lifetime = createLifetimeToken();
  const commandGate = usePendingCommandGate();
  const pending = usePendingCommandsWithGate(reload, commandGate, (entity) =>
    entity.startsWith("user:"),
  );
  const selected = createCell<AdminRename | null>(null);
  const pagination = useAdminPagination("renames", ["page_size"], 200, 2_147_483_647);
  const renames = useAdminPage(
    pagination,
    () => {
      reload();
      return requestedQuery();
    },
    (request: readonly [string, bigint], size: number, signal: AbortSignal) => {
      const pageNumber = request[1];
      if (pageNumber > BigInt(Number.MAX_SAFE_INTEGER)) {
        return Promise.resolve(err("admin_pagination_bounds"));
      }
      return fetchAdminRenames(host, "all", request[0], Number(pageNumber), size, signal);
    },
  );

  const confirmRevert = (): void => {
    const current = selected();
    if (current === null) {
      return;
    }
    if (
      current.reverted_at !== null ||
      pending.anyBusy() ||
      pending.contains(userCommandEntity(current.user_id))
    ) {
      return;
    }
    // 提交时复制并冻结改名事实；命令重试只使用这份快照，不读之后可能被替换的列表项。
    const rename = Object.freeze({ ...current });
    // 首次准备成功后冻结 fence；重放只使用同一请求，不能重新读取用户版本。
    let frozen: AdminRenameRevertCommand | null = null;
    const submitted = pending.submit(
      userCommandEntity(rename.user_id),
      t("admin-renames-revert"),
      async (commandId, signal): Promise<Result<unknown, string>> => {
        if (frozen === null) {
          const prepared = await prepareAdminRenameRevert(host, commandId, rename, signal);
          // 任务取消后准备结果随 future 一起丢弃，不能写回闭包里的 fence。
          if (signal.aborted) {
            return err("network_error");
          }
          if (!prepared.ok) {
            return prepared;
          }
          frozen = prepared.value;
        }
        const result = await revertAdminRename(host, frozen, signal);
        return result.ok ? result : err(result.error);
      },
      () => setIfAlive(notice, { kind: "success" }, lifetime.active),
      (code) => {
        if (code === "stale_user_state" || code === "rename_not_revertable") {
          advanceCommandReload(reload);
        }
        setIfAlive(notice, { kind: "error", code }, lifetime.active);
      },
    );
    if (submitted) {
      notice.set(null);
      selected.set(null);
    }
  };

  const view = createValueMemo((): RenamesView => {
    const state = renames();
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
    <PendingCommandsContext value={pending}>
      <section class="admin-users-page" aria-labelledby="admin-renames-title">
        <h1 id="admin-renames-title">{t("page-renames-title")}</h1>
        <PendingCommandPanel owner={pending} />
        <div class="admin-users-renames-filter">
          <label for="rename-name">
            {t("admin-renames-name-filter")}
            <input
              id="rename-name"
              name="name"
              autocomplete="off"
              autocapitalize="none"
              spellcheck="false"
              type="search"
              maxlength="256"
              value={queryState().name}
              onInput={(event) => {
                // 即时筛选每次击键都同步地址栏；用 replace 避免击键淹没历史栈。
                const current = queryState();
                const next = { ...current, name: event.currentTarget.value, page: 1n };
                query.replace(next);
              }}
            />
          </label>
        </div>
        <Switch fallback={null}>
          <Match when={view().kind === "loading"}>
            <AsyncState kind="loading" title={t("admin-renames-loading")} detail="" />
          </Match>
          <Match when={loadError()}>
            {(error) => (
              <AsyncState
                kind="error"
                title={adminApiErrorMessage(error().code)}
                detail=""
                retryLabel={t("admin-retry")}
                onRetry={() => advanceCommandReload(reload)}
              />
            )}
          </Match>
        </Switch>
        <Show when={readyPage()}>
          {(current) => (
            <>
              <RenameTable
                page={current()}
                sort={queryState().sort}
                direction={queryState().direction}
                onSelect={(rename) => {
                  if (!pending.anyBusy() && !pending.contains(userCommandEntity(rename.user_id))) {
                    selected.set(rename);
                  }
                }}
                onSortChange={([sort, direction]) => {
                  const next = { ...queryState(), sort, direction };
                  query.push(next);
                }}
              />
              <Show when={current().renames.length === 0}>
                <AsyncState kind="empty" title={t("admin-renames-empty")} detail="" />
              </Show>
              <Show when={notice()}>
                {(feedback) => (
                  <p
                    class={
                      feedback().kind === "success"
                        ? "action-feedback action-feedback--success"
                        : "action-feedback action-feedback--error"
                    }
                    role={feedback().kind === "success" ? "status" : "alert"}
                  >
                    {adminActionFeedbackMessage(feedback(), t("admin-renames-revert-success"))}
                  </p>
                )}
              </Show>
              <AdminPager
                pagination={pagination}
                page={current().page}
                pageSize={current().page_size}
                total={current().total}
                label={t("page-renames-title")}
                onPageChange={(nextPage) => {
                  const next = { ...queryState(), page: BigInt(nextPage) };
                  query.push(next);
                }}
              />
            </>
          )}
        </Show>
        <Show when={selected()}>
          {(rename) => (
            <Dialog
              title={t("admin-renames-revert-confirm-title")}
              closeLabel={t("admin-renames-cancel")}
              busy={pending.anyBusy()}
              onClose={() => selected.set(null)}
            >
              <p>
                {t("admin-renames-revert-confirm-description", {
                  from: rename().next_username,
                  to: rename().previous_username,
                })}
              </p>
              <div class="admin-shell__dialog-actions">
                <button
                  type="button"
                  disabled={
                    pending.anyBusy() || pending.contains(userCommandEntity(rename().user_id))
                  }
                  onClick={confirmRevert}
                >
                  {t("admin-users-action-confirm")}
                </button>
                <button
                  type="button"
                  autofocus={true}
                  disabled={pending.anyBusy()}
                  onClick={() => selected.set(null)}
                >
                  {t("admin-renames-cancel")}
                </button>
              </div>
            </Dialog>
          )}
        </Show>
      </section>
    </PendingCommandsContext>
  );
}

/** 当前页改名表：桌面表头、排序与行内回滚入口。 */
function RenameTable(props: {
  readonly page: AdminRenamePage;
  /** 当前排序列（空串 = 服务端原始顺序）与方向；变更经 `onSortChange` 由页面写回 URL。 */
  readonly sort: string;
  readonly direction: SortDirection;
  readonly onSelect: (rename: AdminRename) => void;
  readonly onSortChange: (value: readonly [string, SortDirection]) => void;
}): JSX.Element {
  const pending = requirePendingCommands();
  const sortedRenames = createValueMemo(() => {
    const next = [...props.page.renames];
    sortRenames(next, props.sort, props.direction);
    return next;
  });
  return (
    <div class="admin-shell__table-wrap">
      <table
        class="admin-shell__table"
        aria-busy={pending.anyBusy() ? "true" : "false"}
        aria-label={t("page-renames-title")}
      >
        <TableColumns layout="admin-renames" widths={[21, 21, 9, 20, 20, 9]} />
        <thead>
          <tr>
            <SortableTh
              column="previous"
              label={t("admin-renames-previous")}
              active={props.sort}
              direction={props.direction}
              onSort={props.onSortChange}
            />
            <SortableTh
              column="next"
              label={t("admin-renames-next")}
              active={props.sort}
              direction={props.direction}
              onSort={props.onSortChange}
            />
            <SortableTh
              column="actor"
              label={t("admin-renames-actor")}
              active={props.sort}
              direction={props.direction}
              onSort={props.onSortChange}
            />
            <SortableTh
              column="created"
              label={t("admin-renames-created")}
              active={props.sort}
              direction={props.direction}
              onSort={props.onSortChange}
            />
            <th scope="col">{t("admin-renames-reverted")}</th>
            <th scope="col">{t("admin-users-actions")}</th>
          </tr>
        </thead>
        <tbody>
          <For each={sortedRenames()} keyed={(rename) => rename.id}>
            {(rename) => <RenameRow rename={rename()} onSelect={props.onSelect} />}
          </For>
        </tbody>
      </table>
    </div>
  );
}

/** 一行改名事实；回滚按钮只在该用户没有未决命令时可用。 */
function RenameRow(props: {
  readonly rename: AdminRename;
  readonly onSelect: (rename: AdminRename) => void;
}): JSX.Element {
  const pending = requirePendingCommands();
  return (
    <tr>
      <td>
        <TruncatedText value={props.rename.previous_username} />
      </td>
      <td>
        <TruncatedText value={props.rename.next_username} />
      </td>
      <td>
        <TruncatedText value={props.rename.actor} />
      </td>
      <td>
        <Time value={props.rename.created_at} />
      </td>
      <td>
        <Show when={props.rename.reverted_at !== null ? props.rename.reverted_at : null}>
          {(revertedAt) => <Time value={revertedAt()} />}
        </Show>
      </td>
      <td>
        <Show
          when={props.rename.reverted_at !== null}
          fallback={
            <button
              type="button"
              disabled={
                pending.anyBusy() || pending.contains(userCommandEntity(props.rename.user_id))
              }
              onClick={() => props.onSelect(props.rename)}
            >
              {t("admin-renames-revert")}
            </button>
          }
        >
          {t("admin-renames-no-action")}
        </Show>
      </td>
    </tr>
  );
}

/** 对当前页改名记录排序；`column` 为空或不在允许集合时保持服务端原始顺序。 */
function sortRenames(renames: AdminRename[], column: string, direction: SortDirection): void {
  sortRows(renames, column, direction, (left, right, current) => {
    switch (current) {
      case "created":
        return compareText(left.created_at, right.created_at);
      case "previous":
        return compareText(left.previous_username, right.previous_username);
      case "next":
        return compareText(left.next_username, right.next_username);
      case "actor":
        return compareText(left.actor, right.actor);
      default:
        return 0;
    }
  });
}

interface LifetimeToken {
  active(): boolean;
}

/** 组件卸载后仍可能收到命令回执；写页面反馈前检查自身 owner 是否仍存活。 */
function createLifetimeToken(): LifetimeToken {
  let active = true;
  onCleanup(() => {
    active = false;
  });
  return { active: () => active };
}
