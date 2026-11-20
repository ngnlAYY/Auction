// Admin 用户筛选、列表和分页。

import { type Cell, createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { compareText } from "@auction/web-shared/text";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { TableColumns } from "@auction/web-shared/ui/components/table-columns";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { createEffect, For, Show, untrack } from "solid-js";

import type { AdminUser, AdminUserPage, AdminUserStatusFilter } from "../../api/users";
import { AdminPager, type AdminPagination } from "../pagination";
import type { AdminActionFeedback } from "../presentation";
import { SortableTh, type SortDirection, sortRows } from "../sort";
import { useFocusField } from "../validation";
import { parseStatusFilter, statusFilterValue } from "./dialogs";
import {
  searchNameIssue,
  UserFieldError,
  type UserFieldIssue,
  userFieldIssueMessage,
} from "./fields";
import { AdminUserRow } from "./row";

export interface AdminUserFiltersProps {
  readonly applied: () => readonly [string, AdminUserStatusFilter];
  readonly historyRevision: Cell<number>;
  readonly onNameApply: (name: string) => void;
  readonly onStatusChange: (status: AdminUserStatusFilter) => void;
}

/** 用户名筛选是提交前草稿：输入不改变查询，提交或历史恢复才写回地址栏。 */
export function AdminUserFilters(props: AdminUserFiltersProps): JSX.Element {
  const draft = useSearchDraft(props.applied, props.historyRevision);
  const focusField = useFocusField();
  const nameFocused = createCell(false);
  const status = createValueMemo(() => props.applied()[1]);
  const issue = draft.issue;
  const announced = createValueMemo(() => draft.announcement().issue);
  const describedBy = createValueMemo(() => {
    if (announced() !== null) return "user-name-filter-announcement";
    return issue() === null ? undefined : "user-name-filter-error";
  });

  return (
    <>
      {/* biome-ignore lint/a11y/useSemanticElements: 源码使用带 role="search" 的 form，保留既有 DOM。 */}
      <form
        class="admin-users-filter"
        role="search"
        aria-label={t("admin-users-filter-apply")}
        method="get"
        action="/users"
        autocomplete="off"
        onSubmit={(event) => {
          event.preventDefault();
          const name = draft.validateSubmission(nameFocused());
          if (name !== null) {
            props.onNameApply(name);
          } else if (!nameFocused()) {
            focusField("user-name-filter");
          }
        }}
      >
        <label for="user-name-filter">
          {t("admin-users-filter-name")}
          <input
            id="user-name-filter"
            name="name"
            type="search"
            autocapitalize="none"
            spellcheck="false"
            enterkeyhint="search"
            aria-invalid={issue() === null ? "false" : "true"}
            aria-describedby={describedBy()}
            value={draft.name()}
            onFocus={() => nameFocused.set(true)}
            onBlur={() => nameFocused.set(false)}
            onInput={(event) => draft.edit(event.currentTarget.value)}
          />
        </label>
        <label for="user-status-filter">
          {t("admin-users-filter-status")}
          <select
            id="user-status-filter"
            name="status"
            value={statusFilterValue(status())}
            onChange={(event) => {
              props.onStatusChange(parseStatusFilter(event.currentTarget.value));
            }}
          >
            <option value="all">{t("admin-users-status-all")}</option>
            <option value="active">{t("admin-users-status-active")}</option>
            <option value="blacklisted">{t("admin-users-status-blacklisted")}</option>
          </select>
        </label>
        <button type="submit" class="admin-btn-secondary">
          {t("admin-users-filter-apply")}
        </button>
      </form>
      <UserFieldError id="user-name-filter-error" issue={announced() === null ? issue() : null} />
      <UserSearchAnnouncement announcement={draft.announcement} />
    </>
  );
}

function UserSearchAnnouncement(props: { announcement: Cell<SearchAnnouncement> }): JSX.Element {
  const issue = createValueMemo(() => props.announcement().issue);
  const announced = createValueMemo(() => {
    const current = props.announcement();
    return current.issue === null ? [] : [{ revision: current.revision, issue: current.issue }];
  });
  return (
    <div
      id="user-name-filter-announcement"
      aria-live="polite"
      aria-atomic="true"
      class={issue() === null ? undefined : "admin-field-error"}
    >
      {/* 同一错误再次提交也产生新增文本节点，常驻播报区与字段焦点保持稳定。 */}
      <For each={announced()} keyed={(entry) => entry.revision}>
        {(entry) => <span>{userFieldIssueMessage(entry().issue)}</span>}
      </For>
    </div>
  );
}

interface SearchAnnouncement {
  readonly revision: number;
  readonly issue: UserFieldIssue | null;
}

interface UserSearchDraft {
  readonly name: Cell<string>;
  readonly issue: Cell<UserFieldIssue | null>;
  readonly announcement: Cell<SearchAnnouncement>;
  edit(name: string): void;
  validateSubmission(focused: boolean): string | null;
}

function useSearchDraft(
  applied: () => readonly [string, AdminUserStatusFilter],
  historyRevision: Cell<number>,
): UserSearchDraft {
  const name = createCell(untrack(() => applied()[0]));
  const issue = createCell<UserFieldIssue | null>(null);
  const announcement = createCell<SearchAnnouncement>({ revision: 0, issue: null });
  const draft: UserSearchDraft = {
    name,
    issue,
    announcement,
    edit(nextName) {
      announcement.update((current) => ({ ...current, issue: null }));
      if (issue.peek() !== null) {
        issue.set(searchNameIssue(nextName));
      }
      name.set(nextName);
    },
    validateSubmission(focused) {
      const value = name.peek();
      const nextIssue = searchNameIssue(value);
      issue.set(nextIssue);
      // Enter 提交时焦点可能已经在错误字段上，重新聚焦不会播报。
      announcement.update((current) => {
        const announced = focused ? nextIssue : null;
        return {
          issue: announced,
          revision: announced === null ? current.revision : current.revision + 1,
        };
      });
      return nextIssue === null ? value : null;
    },
  };
  createEffect(
    () => {
      historyRevision();
      return applied()[0];
    },
    (value) => {
      name.set(value);
      issue.set(null);
      announcement.update((current) => ({ ...current, issue: null }));
    },
  );
  return draft;
}

export interface AdminUserTableProps {
  readonly page: AdminUserPage;
  readonly pagination: AdminPagination;
  readonly sort: string;
  readonly direction: SortDirection;
  readonly onPageChange: (page: number) => void;
  readonly onSortChange: (value: readonly [string, SortDirection]) => void;
  readonly reloadGeneration: Cell<bigint>;
  readonly loadedGeneration: bigint;
  readonly actionBusy: Cell<boolean>;
  readonly actionFeedback: Cell<AdminActionFeedback | null>;
}

/** 用户表格；排序在浏览器内对当前页副本进行，不改变服务端返回对象。 */
export function AdminUserTable(props: AdminUserTableProps): JSX.Element {
  const isEmpty = createValueMemo(() => props.page.users.length === 0);
  const users = createValueMemo(() => {
    const rows = [...props.page.users];
    sortUsers(rows, props.sort, props.direction);
    return rows;
  });
  return (
    <>
      <div class="admin-users-table-wrap">
        <table class="admin-users-table" aria-label={t("page-users-title")}>
          {/* 列序：名称、论坛 ID、状态、封禁理由、次数、注册时间、操作；为四个操作按钮预留同排空间。 */}
          <TableColumns layout="admin-users" widths={[12, 10, 6, 14, 5, 13, 40]} />
          <thead>
            <tr>
              <SortableTh
                column="username"
                label={t("admin-users-username")}
                active={props.sort}
                direction={props.direction}
                onSort={props.onSortChange}
              />
              <th scope="col">{t("admin-users-forum-account-id")}</th>
              <SortableTh
                column="status"
                label={t("admin-users-status")}
                active={props.sort}
                direction={props.direction}
                onSort={props.onSortChange}
              />
              <th scope="col">{t("admin-users-blacklist-history")}</th>
              <SortableTh
                column="blacklist_count"
                label={t("admin-users-blacklist-count")}
                active={props.sort}
                direction={props.direction}
                onSort={props.onSortChange}
              />
              <SortableTh
                column="created_at"
                label={t("admin-users-created-at")}
                active={props.sort}
                direction={props.direction}
                onSort={props.onSortChange}
              />
              <th scope="col">{t("admin-users-actions")}</th>
            </tr>
          </thead>
          <tbody>
            <For each={users()} keyed={(user) => user.id}>
              {(user) => (
                <AdminUserRow
                  user={user()}
                  reloadGeneration={props.reloadGeneration}
                  loadedGeneration={props.loadedGeneration}
                  busy={props.actionBusy}
                  feedback={props.actionFeedback}
                />
              )}
            </For>
          </tbody>
        </table>
      </div>
      <Show when={isEmpty()}>
        <AsyncState kind="empty" title={t("admin-users-no-results")} detail="" />
      </Show>
      <AdminPager
        pagination={props.pagination}
        page={props.page.pagination.page}
        pageSize={props.page.pagination.page_size}
        total={props.page.pagination.total}
        label={t("page-users-title")}
        onPageChange={props.onPageChange}
      />
    </>
  );
}

/** 对当前页用户行排序；`column` 为空或不在允许集合时保持服务端原始顺序。 */
function sortUsers(users: AdminUser[], column: string, direction: SortDirection): void {
  sortRows(users, column, direction, (left, right, current) => {
    switch (current) {
      case "username":
        return compareText(left.username, right.username);
      case "status":
        return compareText(left.status, right.status);
      case "blacklist_count":
        return left.blacklist_count === right.blacklist_count
          ? 0
          : left.blacklist_count < right.blacklist_count
            ? -1
            : 1;
      case "created_at":
        return compareText(left.created_at, right.created_at);
      default:
        return 0;
    }
  });
}
