// Admin 用户行：状态徽标、论坛 ID、拉黑历史提示与用户状态操作对话框。

import { type Cell, createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { err, ok } from "@auction/web-shared/result";
import { Dialog } from "@auction/web-shared/ui/components/dialog";
import { Time } from "@auction/web-shared/ui/components/time";
import { TruncatedText } from "@auction/web-shared/ui/components/truncated-text";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Match, Show, Switch, untrack, useContext } from "solid-js";

import {
  type AdminUser,
  blacklistAdminUser,
  mutateAdminUser,
  prepareAdminUserBlacklistOnce,
  prepareAdminUserMutationOnce,
  prepareAdminUserUnblacklistOnce,
  unblacklistAdminUser,
} from "../../api/users";
import { KeyVisibilityContext } from "../auth/key-visibility";
import { useAdminSessionHost } from "../auth/shell";
import { requirePendingCommands } from "../pending";
import type { AdminActionFeedback } from "../presentation";
import { userCommandEntity } from "../users";
import { useFocusField } from "../validation";
import {
  AdminUserDialogActions,
  type AdminUserDialogKind,
  adminUserDialogKindTitle,
  runUserAction,
} from "./dialogs";
import {
  blacklistReasonIssue,
  forumAccountIssue,
  UserFieldError,
  type UserFieldIssue,
} from "./fields";

/** 黑名单历史的浮动提示框位置；由触发按钮的视口矩形算出。 */
interface HistoryTooltipPosition {
  readonly left: number;
  readonly top: number;
  readonly width: number;
  readonly maximumHeight: number;
  readonly placement: "above" | "below";
}

// 每个按钮的提示框都需要一个独立的 aria-describedby 目标；Rust 侧是进程内计数器。
let nextHistoryTooltipId = 1;

export interface AdminUserRowProps {
  readonly user: AdminUser;
  readonly reloadGeneration: Cell<bigint>;
  readonly loadedGeneration: bigint;
  readonly busy: Cell<boolean>;
  readonly feedback: Cell<AdminActionFeedback | null>;
}

/** 列表中的一行用户；行内还携带该行的操作对话框行。 */
export function AdminUserRow(props: AdminUserRowProps): JSX.Element {
  const host = useAdminSessionHost();
  const keyVisibility = useContext(KeyVisibilityContext);
  const owner = requirePendingCommands();
  const focusField = useFocusField();

  const readIsCurrent = () => props.reloadGeneration() === props.loadedGeneration;
  // 原命令了结后旧行可显示，获取当前用户版本前不能打开或提交会写入的对话框。
  const blocked = createValueMemo(
    () => !readIsCurrent() || owner.contains(userCommandEntity(props.user.id)),
  );
  const statusLabel = createValueMemo(() => {
    const status = props.user.status;
    if (status === "active") return t("admin-users-status-active");
    if (status === "blacklisted") return t("admin-users-status-blacklisted");
    return t("admin-error-invalid-response");
  });
  const dialog = createCell<AdminUserDialogKind | null>(null);
  const dialogReason = createCell("");
  const dialogTitle = createValueMemo(() => {
    const kind = dialog();
    return kind === null ? null : adminUserDialogKindTitle(kind, props.user.username);
  });
  const forumAccountIdValue = createCell(untrack(() => props.user.forum_account_id ?? ""));
  const fieldIssue = createCell<UserFieldIssue | null>(null);

  return (
    <>
      <tr>
        <td>
          <TruncatedText value={props.user.username} />
        </td>
        <td>
          <TruncatedText value={props.user.forum_account_id ?? t("admin-users-empty")} />
        </td>
        <td>
          <span class="admin-users-badge">{statusLabel()}</span>
        </td>
        <td class="admin-users-history-cell">
          <BlacklistHistoryCell user={props.user} />
        </td>
        <td>{props.user.blacklist_count}</td>
        <td>
          <Time value={props.user.created_at} />
        </td>
        <td>
          <div class="admin-users-actions">
            <button
              type="button"
              disabled={props.busy() || blocked()}
              onClick={() => {
                if (!readIsCurrent()) return;
                forumAccountIdValue.set(props.user.forum_account_id ?? "");
                fieldIssue.set(null);
                dialog.set("forum_account_id");
              }}
            >
              {t("admin-users-action-set-forum-account-id")}
            </button>
            <button
              type="button"
              disabled={props.busy() || blocked()}
              onClick={() => {
                if (readIsCurrent()) dialog.set("reissue_key");
              }}
            >
              {t("admin-users-action-reissue-key")}
            </button>
            <button
              type="button"
              disabled={props.busy() || blocked() || props.user.key_status === "revoked"}
              onClick={() => {
                if (readIsCurrent()) dialog.set("revoke_key");
              }}
            >
              {t("admin-users-action-revoke-key")}
            </button>
            <Show
              when={props.user.status === "blacklisted"}
              fallback={
                <button
                  type="button"
                  class="admin-btn-danger"
                  disabled={props.busy() || blocked()}
                  onClick={() => {
                    if (!readIsCurrent()) return;
                    dialogReason.set("");
                    fieldIssue.set(null);
                    dialog.set("blacklist");
                  }}
                >
                  {t("admin-users-action-blacklist")}
                </button>
              }
            >
              <button
                type="button"
                class="admin-btn-secondary"
                disabled={props.busy() || blocked()}
                onClick={() => {
                  if (readIsCurrent()) dialog.set("unblacklist");
                }}
              >
                {t("admin-users-action-unblacklist")}
              </button>
            </Show>
          </div>
        </td>
      </tr>
      <Show when={dialog()}>
        <tr class="admin-users-dialog-row">
          <td colspan="7">
            <Dialog
              title={dialogTitle() ?? ""}
              closeLabel={t("admin-users-action-cancel")}
              busy={props.busy()}
              onClose={() => dialog.set(null)}
            >
              <Switch>
                <Match when={dialog() === "forum_account_id"}>
                  <form
                    method="post"
                    action="/api/v1/admin/users"
                    aria-label={t("admin-users-action-set-forum-account-id")}
                    novalidate={true}
                    onSubmit={(event) => {
                      event.preventDefault();
                      if (props.busy() || !readIsCurrent()) return;
                      const user: AdminUser = structuredClone(props.user);
                      const value = forumAccountIdValue();
                      const issue = forumAccountIssue(value, true);
                      fieldIssue.set(issue);
                      if (issue !== null) {
                        focusField("admin-user-edit-forum-account");
                        return;
                      }
                      const prepare = prepareAdminUserMutationOnce(
                        user,
                        "set_forum_account_id",
                        null,
                        value,
                      );
                      const submitted = runUserAction(
                        owner,
                        userCommandEntity(user.id),
                        t("admin-users-action-set-forum-account-id"),
                        props.feedback,
                        props.reloadGeneration,
                        async (commandId, signal) => {
                          const result = await mutateAdminUser(host, commandId, prepare, signal);
                          return result.ok ? ok(null) : err(result.error);
                        },
                      );
                      if (submitted) dialog.set(null);
                    }}
                  >
                    <label for="admin-user-edit-forum-account">
                      {t("admin-users-forum-account-id")}
                      <span aria-hidden="true">{" *"}</span>
                    </label>
                    <input
                      id="admin-user-edit-forum-account"
                      name="forum_account_id"
                      type="text"
                      inputmode="numeric"
                      autofocus={true}
                      required={true}
                      disabled={props.busy()}
                      value={forumAccountIdValue()}
                      aria-invalid={fieldIssue() === null ? "false" : "true"}
                      aria-describedby={
                        fieldIssue() === null ? undefined : "admin-user-edit-forum-account-error"
                      }
                      onInput={(event) => {
                        const value = event.currentTarget.value;
                        if (fieldIssue.peek() !== null) {
                          fieldIssue.set(forumAccountIssue(value, true));
                        }
                        forumAccountIdValue.set(value);
                      }}
                    />
                    <UserFieldError id="admin-user-edit-forum-account-error" issue={fieldIssue()} />
                    <AdminUserDialogActions
                      busy={props.busy()}
                      blocked={blocked()}
                      onCancel={() => dialog.set(null)}
                      confirmLabel={t("admin-users-save-forum-account-id")}
                      confirmClass="admin-btn-primary"
                    />
                  </form>
                </Match>
                <Match when={dialog() === "blacklist"}>
                  <form
                    method="post"
                    action={`/api/v1/admin/users/${props.user.id}/blacklist`}
                    aria-label={t("admin-users-action-blacklist")}
                    novalidate={true}
                    onSubmit={(event) => {
                      event.preventDefault();
                      if (props.busy() || !readIsCurrent()) return;
                      const user: AdminUser = structuredClone(props.user);
                      const reason = dialogReason();
                      const issue = blacklistReasonIssue(reason);
                      fieldIssue.set(issue);
                      if (issue !== null) {
                        focusField("admin-user-blacklist-reason");
                        return;
                      }
                      const prepare = prepareAdminUserBlacklistOnce(user, reason);
                      const submitted = runUserAction(
                        owner,
                        userCommandEntity(user.id),
                        t("admin-users-action-blacklist"),
                        props.feedback,
                        props.reloadGeneration,
                        async (commandId, signal) => {
                          const result = await blacklistAdminUser(host, commandId, prepare, signal);
                          return result.ok ? ok(null) : err(result.error);
                        },
                      );
                      if (submitted) dialog.set(null);
                    }}
                  >
                    <p>{t("admin-users-blacklist-warning")}</p>
                    <label for="admin-user-blacklist-reason">
                      {t("admin-users-blacklist-reason")}
                    </label>
                    <textarea
                      id="admin-user-blacklist-reason"
                      name="public_reason"
                      autofocus={true}
                      required={true}
                      disabled={props.busy()}
                      value={dialogReason()}
                      aria-invalid={fieldIssue() === null ? "false" : "true"}
                      aria-describedby={
                        fieldIssue() === null ? undefined : "admin-user-blacklist-reason-error"
                      }
                      onInput={(event) => {
                        const value = event.currentTarget.value;
                        if (fieldIssue.peek() !== null) {
                          fieldIssue.set(blacklistReasonIssue(value));
                        }
                        dialogReason.set(value);
                      }}
                    />
                    <UserFieldError id="admin-user-blacklist-reason-error" issue={fieldIssue()} />
                    <AdminUserDialogActions
                      busy={props.busy()}
                      blocked={blocked()}
                      onCancel={() => dialog.set(null)}
                      confirmLabel={t("admin-users-action-blacklist")}
                      confirmClass="admin-btn-danger"
                    />
                  </form>
                </Match>
                <Match when={dialog() !== null}>
                  <Show when={dialog() === "reissue_key"}>
                    <p>{t("admin-users-reissue-warning")}</p>
                  </Show>
                  <Show when={dialog() === "revoke_key"}>
                    <p>{t("admin-users-revoke-warning")}</p>
                  </Show>
                  <div class="admin-shell__dialog-actions">
                    <button
                      type="button"
                      disabled={props.busy()}
                      autofocus={true}
                      onClick={() => dialog.set(null)}
                    >
                      {t("admin-users-action-cancel")}
                    </button>
                    <button
                      type="button"
                      class={dialog() === "revoke_key" ? "admin-btn-danger" : "admin-btn-primary"}
                      disabled={props.busy() || blocked()}
                      onClick={() => {
                        if (props.busy() || !readIsCurrent()) return;
                        const kind = dialog();
                        if (kind === null) return;
                        const action =
                          kind === "revoke_key"
                            ? "revoke_key"
                            : kind === "reissue_key"
                              ? "reissue_key"
                              : kind === "unblacklist"
                                ? "unblacklist"
                                : null;
                        if (action === null) return;
                        const user: AdminUser = structuredClone(props.user);
                        if (action === "unblacklist") {
                          const prepare = prepareAdminUserUnblacklistOnce(user);
                          const submitted = runUserAction(
                            owner,
                            userCommandEntity(user.id),
                            adminUserDialogKindTitle(kind, user.username),
                            props.feedback,
                            props.reloadGeneration,
                            async (commandId, signal) => {
                              const result = await unblacklistAdminUser(
                                host,
                                commandId,
                                prepare,
                                signal,
                              );
                              return result.ok ? ok(null) : err(result.error);
                            },
                          );
                          if (submitted) dialog.set(null);
                          return;
                        }
                        const prepare = prepareAdminUserMutationOnce(user, action, null, null);
                        const submitted = runUserAction(
                          owner,
                          userCommandEntity(user.id),
                          adminUserDialogKindTitle(kind, user.username),
                          props.feedback,
                          props.reloadGeneration,
                          async (commandId, signal) => {
                            const result = await mutateAdminUser(host, commandId, prepare, signal);
                            return result.ok ? ok(result.value.generated_key) : err(result.error);
                          },
                          { keyVisibility },
                        );
                        if (submitted) dialog.set(null);
                      }}
                    >
                      {props.busy() ? t("admin-action-working") : t("admin-users-action-confirm")}
                    </button>
                  </div>
                </Match>
              </Switch>
            </Dialog>
          </td>
        </tr>
      </Show>
    </>
  );
}

/** 拉黑历史单元格：普通列里展示最近一次原因；按钮打开详情与完整历史。 */
function BlacklistHistoryCell(props: { user: AdminUser }): JSX.Element {
  const focused = createCell(false);
  const historyOpen = createCell(false);
  const tooltipPosition = createCell<HistoryTooltipPosition | null>(null);
  const tooltipId = `admin-users-history-tooltip-${nextHistoryTooltipId}`;
  nextHistoryTooltipId += 1;

  let trigger: HTMLButtonElement | undefined;

  const latest = createValueMemo(() => props.user.blacklist_history[0] ?? null);
  const describedBy = () => (tooltipPosition() === null ? undefined : tooltipId);

  return (
    <Show
      when={latest()}
      fallback={<span class="admin-users-history-empty">{t("admin-users-blacklist-never")}</span>}
    >
      {(entry) => (
        <>
          <button
            ref={(element) => {
              trigger = element;
            }}
            type="button"
            class="admin-users-history-trigger"
            aria-label={t("admin-users-blacklist-history-label", {
              count: props.user.blacklist_count,
            })}
            aria-describedby={describedBy()}
            aria-haspopup="dialog"
            onMouseEnter={() => {
              if (trigger !== undefined) showHistoryTooltip(trigger, tooltipPosition);
            }}
            onMouseLeave={() => {
              if (!focused()) tooltipPosition.set(null);
            }}
            onFocus={() => {
              focused.set(true);
              if (trigger !== undefined) showHistoryTooltip(trigger, tooltipPosition);
            }}
            onBlur={() => {
              focused.set(false);
              tooltipPosition.set(null);
            }}
            onClick={() => {
              // 触控与键盘用户没有悬停，直接进入可滚动的详情。
              tooltipPosition.set(null);
              historyOpen.set(true);
            }}
          >
            {entry().public_reason}
          </button>
          <Show when={tooltipPosition()}>
            {(position) => (
              <aside
                id={tooltipId}
                class="admin-users-history-tooltip"
                role="tooltip"
                data-placement={position().placement}
                style={`--admin-users-history-tooltip-left: ${position().left}px; --admin-users-history-tooltip-top: ${position().top}px; --admin-users-history-tooltip-width: ${position().width}px; --admin-users-history-tooltip-max-height: ${position().maximumHeight}px;`}
              >
                <strong>
                  {t("admin-users-blacklist-history-title", { count: props.user.blacklist_count })}
                </strong>
                <BlacklistHistoryEntries user={props.user} />
              </aside>
            )}
          </Show>
          <Show when={historyOpen()}>
            <Dialog
              title={t("admin-users-blacklist-history-title", {
                count: props.user.blacklist_count,
              })}
              closeLabel={t("admin-users-blacklist-history-close")}
              onClose={() => historyOpen.set(false)}
            >
              <div class="admin-users-history-details">
                <BlacklistHistoryEntries user={props.user} />
              </div>
            </Dialog>
          </Show>
        </>
      )}
    </Show>
  );
}

/** 悬浮摘要与触控详情使用同一份已加载历史，不发起额外用户查询。 */
function BlacklistHistoryEntries(props: { user: AdminUser }): JSX.Element {
  return (
    <>
      <ol>
        <For each={props.user.blacklist_history} keyed={false}>
          {(entry) => (
            <li>
              <span>{entry().public_reason}</span>
              <Time value={entry().blacklisted_at} />
            </li>
          )}
        </For>
      </ol>
      <Show when={props.user.blacklist_count > props.user.blacklist_history.length}>
        <p>{t("admin-users-blacklist-history-limited")}</p>
      </Show>
    </>
  );
}

function showHistoryTooltip(
  element: HTMLElement,
  position: Cell<HistoryTooltipPosition | null>,
): void {
  const rect = element.getBoundingClientRect();
  const [viewportWidth, viewportHeight] = viewportSize();
  const width = Math.min(360, Math.max(viewportWidth - 16, 1));
  const gap = 8;
  const margin = 8;
  const availableAbove = Math.max(rect.top - gap - margin, 1);
  const availableBelow = Math.max(viewportHeight - rect.bottom - gap - margin, 1);
  const above = availableAbove > availableBelow;
  const maximumLeft = Math.max(viewportWidth - width - margin, margin);
  const placement = above ? "above" : "below";
  const top = placement === "above" ? rect.top - gap : rect.bottom + gap;
  const maximumHeight = above ? availableAbove : availableBelow;
  const left = Math.min(Math.max(rect.left, margin), maximumLeft);
  position.set({ left, top, width, maximumHeight, placement });
}

/** 窗口缺失或尺寸读取失败时回退到 Rust 侧的 1024×768。 */
function viewportSize(): readonly [number, number] {
  if (typeof window === "undefined") return [1024, 768];
  return [
    viewportDimension(() => window.innerWidth, 1024),
    viewportDimension(() => window.innerHeight, 768),
  ];
}

function viewportDimension(read: () => unknown, fallback: number): number {
  try {
    const value = read();
    return typeof value === "number" && Number.isFinite(value) ? value : fallback;
  } catch {
    return fallback;
  }
}
