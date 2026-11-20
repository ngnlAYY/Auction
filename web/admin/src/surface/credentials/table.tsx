// 凭据列表、状态操作和改名对话框入口。

import { focusElementById } from "@auction/web-shared/platform/a11y";
import { type Cell, createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { compareText } from "@auction/web-shared/text";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { Dialog } from "@auction/web-shared/ui/components/dialog";
import { TableColumns } from "@auction/web-shared/ui/components/table-columns";
import { Time } from "@auction/web-shared/ui/components/time";
import { TruncatedText } from "@auction/web-shared/ui/components/truncated-text";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, onCleanup, Show, untrack } from "solid-js";
import {
  type AdminCredential,
  type AdminCredentialPage,
  mutateAdminCredential,
  prepareAdminCredentialMutation,
} from "../../api/credentials";
import { copyCredential } from "../../api/credentials/prepare";
import type { AdminSessionHost } from "../../api/session";
import { useAdminSessionHost } from "../auth/shell";
import { FieldLiveRegion, useFieldAnnouncer } from "../config/field-announcement";
import {
  type CommandDelivery,
  type PendingCommands,
  requirePendingCommands,
  setIfAlive,
} from "../pending";
import { MobileSortControl, SortableTh, type SortDirection, sortRows } from "../sort";
import { CredentialCheckAction } from "./check";
import {
  CredentialFieldError,
  credentialFieldErrorsDefault,
  credentialFieldErrorsSet,
  credentialFieldIssue,
  credentialFieldIssueMessage,
  useCredentialFields,
} from "./field-validation";
import {
  type CredentialFeedback,
  credentialFeedbackMessage,
  credentialKindLabel,
  credentialStatusClass,
  credentialStatusLabel,
  credentialWorldLabel,
} from "./labels";

/** 凭据列表分页、读取世代、排序状态与删除入口；排序变更由页面写回 URL。 */
export interface CredentialTableProps {
  readonly page: AdminCredentialPage;
  readonly reloadGeneration: Cell<bigint>;
  readonly loadedGeneration: bigint;
  /** 当前排序列（空串 = 服务端原始顺序）与方向；变更经 `onSortChange` 由页面写回 URL。 */
  readonly sort: string;
  readonly direction: SortDirection;
  readonly onSortChange: (value: readonly [string, SortDirection]) => void;
  readonly onDelete: (credential: AdminCredential) => void;
}

/** 凭据列表：提示、窄屏排序入口、桌面表头与操作行。 */
export function CredentialTable(props: CredentialTableProps): JSX.Element {
  const sortedCredentials = createValueMemo(() => {
    const credentials = [...props.page.credentials];
    sortCredentials(credentials, props.sort, props.direction);
    return credentials;
  });
  const isEmpty = createValueMemo(() => props.page.credentials.length === 0);
  const sortColumns = createValueMemo<readonly (readonly [string, string])[]>(() => [
    ["label", t("admin-credentials-label")],
    ["world", t("admin-credentials-world")],
    ["kind", t("admin-credentials-kind")],
    ["status", t("admin-credentials-status")],
    ["checked", t("admin-credentials-rotated-at")],
  ]);

  return (
    <>
      <p class="admin-credentials__feedback">{t("admin-credentials-check-hint")}</p>
      <MobileSortControl
        columns={sortColumns()}
        sort={props.sort}
        direction={props.direction}
        onSortChange={props.onSortChange}
      />
      <div class="admin-credentials-table-wrap">
        <table class="admin-credentials-table" aria-label={t("page-credentials-title")}>
          {/* 列序：名称、世界、类型、状态、轮换时间、使用时间、导入者、操作；五个操作按钮允许换行。 */}
          <TableColumns layout="admin-credentials" widths={[14, 6, 10, 6, 14, 14, 6, 30]} />
          <thead>
            <tr>
              <SortableTh
                column="label"
                label={t("admin-credentials-label")}
                active={props.sort}
                direction={props.direction}
                onSort={props.onSortChange}
              />
              <SortableTh
                column="world"
                label={t("admin-credentials-world")}
                active={props.sort}
                direction={props.direction}
                onSort={props.onSortChange}
              />
              <SortableTh
                column="kind"
                label={t("admin-credentials-kind")}
                active={props.sort}
                direction={props.direction}
                onSort={props.onSortChange}
              />
              <SortableTh
                column="status"
                label={t("admin-credentials-status")}
                active={props.sort}
                direction={props.direction}
                onSort={props.onSortChange}
              />
              <SortableTh
                column="checked"
                // 保留旧 URL 排序键 checked，但 rotated_at 不能冒充真实检查时间。
                label={t("admin-credentials-rotated-at")}
                active={props.sort}
                direction={props.direction}
                onSort={props.onSortChange}
              />
              <th scope="col">{t("admin-credentials-last-used")}</th>
              <th scope="col">{t("admin-credentials-imported-by")}</th>
              <th scope="col">{t("admin-credentials-actions")}</th>
            </tr>
          </thead>
          <tbody>
            <For each={sortedCredentials()} keyed={(credential) => credential.id}>
              {(credential) => (
                <CredentialRow
                  credential={credential()}
                  reloadGeneration={props.reloadGeneration}
                  loadedGeneration={props.loadedGeneration}
                  onDelete={props.onDelete}
                />
              )}
            </For>
          </tbody>
        </table>
      </div>
      <Show when={isEmpty()}>
        <AsyncState kind="empty" title={t("admin-credentials-empty")} detail="" />
      </Show>
    </>
  );
}

/** 单条凭据的安全投影；写操作等待新的读取世代，并受同一实体的未决命令封锁。 */
export interface CredentialRowProps {
  readonly credential: AdminCredential;
  readonly onDelete: (credential: AdminCredential) => void;
  readonly reloadGeneration: Cell<bigint>;
  readonly loadedGeneration: bigint;
}

/** 凭据行：安全字段展示、行内状态操作、手动校验与改名对话框。 */
export function CredentialRow(props: CredentialRowProps): JSX.Element {
  const host = useAdminSessionHost();
  const owner = requirePendingCommands();
  const renameDialog = createCell(false);
  const busy = createCell(false);
  const feedback = createCell<CredentialFeedback | null>(null);
  let alive = true;
  onCleanup(() => {
    alive = false;
  });
  const feedbackAlive = () => alive;
  const entity = createValueMemo(() => `credential:${props.credential.id}`);
  // 重读期间的旧行只供展示，写操作必须等待新凭据版本。
  const readIsCurrent = () => props.reloadGeneration() === props.loadedGeneration;
  const blocked = createValueMemo(() => !readIsCurrent() || owner.contains(entity()));
  const feedbackId = createValueMemo(() => `admin-credential-feedback-${props.credential.id}`);
  const renameInputId = createValueMemo(() => `label-${props.credential.id}`);

  return (
    <>
      <tr class="admin-credentials__row">
        <td data-label={t("admin-credentials-label")}>
          <TruncatedText value={props.credential.label} />
          <Show when={props.credential.player_name !== null}>
            <span title={t("admin-credentials-player-name-help")}>
              {t("admin-credentials-player-name")}
              <TruncatedText value={props.credential.player_name ?? ""} size="compact" />
            </span>
          </Show>
        </td>
        <td data-label={t("admin-credentials-world")}>
          <TruncatedText value={credentialWorldLabel(props.credential.world)} size="compact" />
        </td>
        <td data-label={t("admin-credentials-kind")}>
          {credentialKindLabel(props.credential.kind)}
        </td>
        <td data-label={t("admin-credentials-status")}>
          <span class={credentialStatusClass(props.credential.status)}>
            {credentialStatusLabel(props.credential.status)}
          </span>
        </td>
        <td data-label={t("admin-credentials-rotated-at")}>
          <Show when={props.credential.rotated_at !== null} fallback={t("value-empty")}>
            <Time value={props.credential.rotated_at ?? ""} />
          </Show>
        </td>
        <td data-label={t("admin-credentials-last-used")}>{t("value-empty")}</td>
        {/* 安全 DTO 不含导入者，当前登录 actor 不能代替历史操作事实。 */}
        <td data-label={t("admin-credentials-imported-by")}>{t("value-empty")}</td>
        <td
          data-label={t("admin-credentials-actions")}
          aria-describedby={feedback() === null ? undefined : feedbackId()}
        >
          <div class="admin-credentials__row-actions">
            <button
              type="button"
              disabled={busy() || blocked() || props.credential.status === "active"}
              onClick={() => {
                if (busy() || !readIsCurrent()) {
                  return;
                }
                submitCredential(
                  owner,
                  host,
                  props.credential,
                  "set_status",
                  null,
                  "active",
                  feedback,
                  feedbackAlive,
                );
              }}
            >
              {t("admin-credentials-action-enable")}
            </button>
            <button
              type="button"
              disabled={busy() || blocked() || props.credential.status === "disabled"}
              onClick={() => {
                if (busy() || !readIsCurrent()) {
                  return;
                }
                submitCredential(
                  owner,
                  host,
                  props.credential,
                  "set_status",
                  null,
                  "disabled",
                  feedback,
                  feedbackAlive,
                );
              }}
            >
              {t("admin-credentials-action-disable")}
            </button>
            <button
              type="button"
              disabled={busy() || blocked()}
              onClick={() => {
                if (readIsCurrent()) {
                  renameDialog.set(true);
                }
              }}
            >
              {t("admin-credentials-action-rename-open")}
            </button>
            <button
              type="button"
              class="admin-btn-danger"
              disabled={busy() || blocked()}
              // 点击时复制安全投影，确认框持有点击瞬间的独立快照。
              onClick={() => props.onDelete(copyCredential(props.credential))}
            >
              {t("admin-credentials-action-delete")}
            </button>
            <CredentialCheckAction credential={props.credential} busy={busy} blocked={blocked()} />
            <Show when={feedback()}>
              {(current) => (
                <span
                  id={feedbackId()}
                  class="admin-credentials__feedback admin-credentials__feedback--error"
                  role="alert"
                  aria-label={`${props.credential.label}: ${credentialFeedbackMessage(current())}`}
                >
                  {credentialFeedbackMessage(current())}
                </span>
              )}
            </Show>
          </div>
        </td>
      </tr>
      <Show when={renameDialog()}>
        <tr class="admin-credentials-dialog-row">
          <td colspan="8">
            <CredentialRenameDialog
              initialLabel={props.credential.label}
              inputId={renameInputId()}
              busy={busy()}
              blocked={blocked()}
              onClose={() => renameDialog.set(false)}
              onSave={(label) => {
                if (!readIsCurrent()) {
                  return;
                }
                if (
                  submitCredential(
                    owner,
                    host,
                    props.credential,
                    "rename",
                    label,
                    null,
                    feedback,
                    feedbackAlive,
                  )
                ) {
                  renameDialog.set(false);
                }
              }}
            />
          </td>
        </tr>
      </Show>
    </>
  );
}

interface CredentialRenameDialogProps {
  readonly initialLabel: string;
  readonly inputId: string;
  readonly busy: boolean;
  readonly blocked?: boolean;
  readonly onClose: () => void;
  readonly onSave: (label: string) => void;
}

/** 行内改名对话框；只校验标签，成功后由调用方关闭。 */
function CredentialRenameDialog(props: CredentialRenameDialogProps): JSX.Element {
  // 标签只在弹窗打开时取一次快照，后续 props 变化不能覆盖正在编辑的草稿。
  const renameValue = createCell(untrack(() => props.initialLabel));
  const announcer = useFieldAnnouncer();
  const fields = useCredentialFields(announcer);
  const issue = () => fields.error("label");
  const focusId = createValueMemo(() => props.inputId);
  const errorId = createValueMemo(() => `${props.inputId}-error`);

  return (
    <Dialog
      title={t("admin-credentials-rename-title")}
      closeLabel={t("admin-credentials-cancel")}
      busy={props.busy}
      onClose={() => props.onClose()}
    >
      <form
        novalidate={true}
        onSubmit={(event) => {
          event.preventDefault();
          if (props.busy || props.blocked === true) {
            return;
          }
          const label = renameValue();
          const labelIssue = credentialFieldIssue("label", label);
          let errors = credentialFieldErrorsDefault();
          errors = credentialFieldErrorsSet(errors, "label", labelIssue);
          fields.show(errors);
          if (labelIssue !== null) {
            focusElementById(focusId());
            announcer.announce(credentialFieldIssueMessage(labelIssue));
            return;
          }
          props.onSave(label);
        }}
      >
        <label for={props.inputId}>
          {t("admin-credentials-label")}
          <input
            id={props.inputId}
            type="text"
            autofocus={true}
            required={true}
            disabled={props.busy}
            aria-invalid={issue() !== null ? "true" : "false"}
            aria-describedby={issue() !== null ? errorId() : undefined}
            value={renameValue()}
            onInput={(event) => {
              const value = event.currentTarget.value;
              fields.edited("label", value);
              renameValue.set(value);
            }}
            onBlur={() => fields.blurred("label", renameValue.peek())}
          />
        </label>
        <CredentialFieldError id={errorId()} issue={issue()} />
        <div class="admin-shell__dialog-actions">
          <button
            type="button"
            class="admin-btn-secondary"
            disabled={props.busy}
            onClick={() => props.onClose()}
          >
            {t("admin-credentials-cancel")}
          </button>
          <button
            type="submit"
            class="admin-btn-primary"
            disabled={props.busy || props.blocked === true}
          >
            {props.busy ? t("admin-action-working") : t("admin-credentials-rename-save")}
          </button>
        </div>
      </form>
      <FieldLiveRegion announcer={announcer} />
    </Dialog>
  );
}

/** 只把当前已提交的安全凭据投影与候选值交给页宿主，不持有 Cookie 或结果。 */
function submitCredential(
  owner: PendingCommands,
  host: AdminSessionHost,
  credential: AdminCredential,
  action: string,
  label: string | null,
  status: string | null,
  feedback: Cell<CredentialFeedback | null>,
  feedbackAlive: () => boolean,
): boolean {
  const actionLabel =
    action === "rename"
      ? t("admin-credentials-action-rename-open")
      : status === "active"
        ? t("admin-credentials-action-enable")
        : t("admin-credentials-action-disable");
  // 提交瞬间按字段复制安全投影；执行体与重试都不再读取行上的可变引用。
  const frozenCredential = copyCredential(credential);
  // 同一 command_id 的重试复用首次生成的正文与回执核对字段。
  let prepared: ReturnType<typeof prepareAdminCredentialMutation> | null = null;
  const accepted = owner.submitWithReceipt(
    `credential:${frozenCredential.id}`,
    actionLabel,
    async (commandId, signal) => {
      if (prepared === null) {
        prepared = prepareAdminCredentialMutation(
          commandId,
          frozenCredential,
          action,
          label,
          status,
        );
      }
      return mutateAdminCredential(host, prepared, signal);
    },
    (result, sourceAlive): CommandDelivery => {
      if (sourceAlive) {
        setIfAlive(feedback, null, feedbackAlive);
      }
      // 写入后的列表重读可能卸载这一行，重启提示必须随独立的命令回执保留。
      const message = credentialFeedbackMessage({
        kind: "success",
        restartRequired: result.restart_required,
      });
      return { kind: "receipt", receipt: { kind: "text", text: message } };
    },
    (code) => setIfAlive(feedback, { kind: "error", code }, feedbackAlive),
  );
  if (accepted) {
    setIfAlive(feedback, null, feedbackAlive);
  }
  return accepted;
}

/**
 * 对当前页凭据行排序；`column` 为空或不在允许集合时保持服务端原始顺序。
 * 轮换时间缺省（`null`）在升序时排在最前；`checked` 保留为旧 URL 排序键。
 */
function sortCredentials(
  credentials: AdminCredential[],
  column: string,
  direction: SortDirection,
): void {
  sortRows(credentials, column, direction, (left, right, sortColumn) => {
    switch (sortColumn) {
      case "label":
        return compareText(left.label, right.label);
      case "world":
        return compareText(left.world, right.world);
      case "kind":
        return compareText(left.kind, right.kind);
      case "status":
        return compareText(left.status, right.status);
      case "checked":
        return compareOptionalText(left.rotated_at, right.rotated_at);
      default:
        return 0;
    }
  });
}

/** Rust `Option<String>` 的排序：`None` 在升序时排在最前，`Some` 之间按码点比较。 */
function compareOptionalText(left: string | null, right: string | null): number {
  if (left === null) {
    return right === null ? 0 : -1;
  }
  if (right === null) {
    return 1;
  }
  return compareText(left, right);
}
