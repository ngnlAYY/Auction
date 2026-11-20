// Admin 来源页未决面板与认证外壳的恢复、回执入口。

import { focusElementById } from "@auction/web-shared/platform/a11y";
import { sleep } from "@auction/web-shared/platform/timer";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createTaskScope, type TaskScope } from "@auction/web-shared/reactive/tasks";
import type { Result } from "@auction/web-shared/result";
import { Dialog } from "@auction/web-shared/ui/components/dialog";
import { TruncatedText } from "@auction/web-shared/ui/components/truncated-text";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Show } from "solid-js";

import { adminSessionWritesAllowed } from "../../api/session";
import { adminApiErrorMessage } from "../presentation";
import { type AdminUserGeneratedKey, AdminUserGeneratedKeyDialog } from "../users/create";
import {
  type CommandIdentity,
  type CommandNotice,
  type CommandReceipt,
  type PendingCommand,
  type PendingCommandHost,
  type PendingCommands,
  pendingCommandBelongsTo,
  pendingCommandRetryBlocked,
} from ".";
import { liveSource } from "./sources";

interface PendingPanelRow {
  readonly commandId: string;
  readonly identity: CommandIdentity;
  readonly active: PendingCommand | null;
  readonly error: string | null;
}

/** 来源页内的操作区读取会话宿主的同一条命令，并展示本页的拒绝原因。 */
export function PendingCommandPanel(props: { readonly owner: PendingCommands }): JSX.Element {
  const source = () => props.owner.source();
  const rejections = () => source().rejections();
  const activeEntries = () =>
    props.owner.host.entries().filter((entry) => pendingCommandBelongsTo(entry, source()));
  const hasActive = () => activeEntries().length > 0;
  // 在 JSX 的跟踪上下文里读取会话宿主许可；同代复核恢复后按钮随之更新。
  const writesBlocked = () => !adminSessionWritesAllowed(props.owner.host.sessionHost);
  const rows = (): readonly PendingPanelRow[] => [
    ...activeEntries().map((entry) => ({
      commandId: entry.identity.commandId,
      identity: entry.identity,
      active: entry,
      error: entry.error,
    })),
    ...rejections().map((notice) => ({
      commandId: notice.identity.commandId,
      identity: notice.identity,
      active: null,
      error: notice.result.ok ? null : notice.result.error,
    })),
  ];
  return (
    <Show when={hasActive() || rejections().length > 0}>
      <section class="admin-panel admin-pending" aria-label={t("admin-pending-title")}>
        <h2>{t("admin-pending-title")}</h2>
        <Show when={hasActive()}>
          <p class="admin-panel__note">{t("admin-pending-session-help")}</p>
        </Show>
        <ul class="admin-pending__list">
          <For each={rows()} keyed={(row) => row.commandId}>
            {(row) => (
              <li class="admin-pending__entry" data-command-id={row().commandId}>
                {commandIdentity(row().identity)}
                <div class="admin-pending__result">
                  <p role="status">
                    <Show when={row().active?.busy === true}>{t("admin-pending-busy")}</Show>
                    <Show when={row().active?.unknown === true}>{t("admin-pending-unknown")}</Show>
                    <Show when={row().active === null}>{t("admin-pending-rejected")}</Show>
                  </p>
                  <Show when={row().error !== null}>
                    <p class="action-feedback action-feedback--error">
                      {adminApiErrorMessage(row().error ?? "")}
                    </p>
                  </Show>
                </div>
                <div class="admin-pending__actions">
                  <Show when={row().active?.unknown === true ? row().active : null}>
                    {(entry) => (
                      <button
                        type="button"
                        disabled={
                          writesBlocked() || pendingCommandRetryBlocked(entry(), props.owner.host)
                        }
                        onClick={() => props.owner.host.retry(row().commandId)}
                      >
                        {t("admin-pending-retry")}
                      </button>
                    )}
                  </Show>
                  <Show when={row().active === null}>
                    <button type="button" onClick={() => props.owner.dismiss(row().commandId)}>
                      {t("admin-pending-dismiss")}
                    </button>
                  </Show>
                </div>
              </li>
            )}
          </For>
        </ul>
      </section>
    </Show>
  );
}

function commandIdentity(identity: CommandIdentity): JSX.Element {
  return (
    <div class="admin-pending__identity">
      <strong>
        <TruncatedText value={identity.action} />
      </strong>
      <TruncatedText value={identity.entity} />
      <code>
        <TruncatedText value={identity.commandId} />
      </code>
    </div>
  );
}

/** 每个认证路由均可恢复未知命令；回执保留到操作员关闭，播报区始终挂载。 */
export function PendingCommandNotices(props: { readonly host: PendingCommandHost }): JSX.Element {
  const confirmAbandon = createCell<string | null>(null);
  const tasks = createTaskScope();
  const entries = () => props.host.entries();
  const unknown = () => entries().filter((entry) => entry.unknown);
  const count = () => unknown().length;
  const retrying = () => unknown().filter((entry) => entry.busy).length;
  const writesBlocked = () => !adminSessionWritesAllowed(props.host.sessionHost);
  const abandon = () => {
    const commandId = confirmAbandon();
    if (commandId === null) {
      return null;
    }
    return (
      entries().find(
        (entry) => entry.identity.commandId === commandId && entry.unknown && !entry.busy,
      ) ?? null
    );
  };
  return (
    <>
      <p class="admin-command-notices__status" role="status" aria-atomic="true">
        {t("admin-pending-announcement", { count: count(), retrying: retrying() })}
      </p>
      <Show when={count() > 0}>
        <section
          class="admin-panel admin-pending admin-command-notices"
          aria-labelledby="admin-command-notices-title"
        >
          <h2 id="admin-command-notices-title">
            {t("admin-pending-unknown-count", { count: count() })}
          </h2>
          <p class="admin-panel__note">{t("admin-pending-session-help")}</p>
          <ul class="admin-pending__list">
            <For each={unknown()} keyed={(entry) => entry.identity.commandId}>
              {(entry) => (
                <li class="admin-pending__entry" data-command-id={entry().identity.commandId}>
                  {commandIdentity(entry().identity)}
                  <div class="admin-pending__result">
                    <Show when={entry().busy}>
                      <p>{t("admin-pending-busy")}</p>
                    </Show>
                    <Show when={entry().error !== null}>
                      <p class="action-feedback action-feedback--error">
                        {adminApiErrorMessage(entry().error ?? "")}
                      </p>
                    </Show>
                  </div>
                  <div class="admin-pending__actions">
                    <button
                      id={recordButtonId(entry().identity.commandId, "retry")}
                      type="button"
                      disabled={writesBlocked() || pendingCommandRetryBlocked(entry(), props.host)}
                      onClick={() => props.host.retry(entry().identity.commandId)}
                    >
                      {t("admin-pending-retry")}
                    </button>
                    <button
                      id={recordButtonId(entry().identity.commandId, "abandon")}
                      type="button"
                      disabled={writesBlocked() || entry().busy}
                      onClick={() => confirmAbandon.set(entry().identity.commandId)}
                    >
                      {t("admin-pending-abandon")}
                    </button>
                  </div>
                </li>
              )}
            </For>
          </ul>
        </section>
      </Show>
      <Show when={abandon()}>
        {(entry) => (
          <Dialog
            title={t("admin-pending-abandon-title")}
            closeLabel={t("admin-pending-keep")}
            onClose={() => confirmAbandon.set(null)}
          >
            {commandIdentity(entry().identity)}
            <p>{t("admin-pending-abandon-detail")}</p>
            <div class="admin-shell__dialog-actions">
              <button
                type="button"
                class="admin-btn-secondary"
                autofocus={true}
                onClick={() => confirmAbandon.set(null)}
              >
                {t("admin-pending-keep")}
              </button>
              <button
                type="button"
                class="admin-btn-danger"
                disabled={writesBlocked()}
                onClick={() => {
                  const commandId = entry().identity.commandId;
                  const focus = focusTargetsAfter(props.host, commandId);
                  confirmAbandon.set(null);
                  if (props.host.abandon(commandId)) {
                    focusAfterRemoval(tasks, focus);
                  }
                }}
              >
                {t("admin-pending-abandon-confirm")}
              </button>
            </div>
          </Dialog>
        )}
      </Show>
      <CommandReceipts host={props.host} />
    </>
  );
}

function CommandReceipts(props: { readonly host: PendingCommandHost }): JSX.Element {
  const openKey = createCell<string | null>(null);
  const tasks = createTaskScope();
  const notices = () => props.host.notices();
  const visible = () =>
    notices().filter((notice) => !pendingCommandHostKeyInSource(props.host, notice));
  const activeKey = () => {
    const commandId = openKey();
    if (commandId === null) {
      return null;
    }
    const notice = visible().find((candidate) => candidate.identity.commandId === commandId);
    if (notice === undefined) {
      return null;
    }
    const key = generatedKeyOf(notice.result);
    return key === null ? null : Object.freeze({ commandId, key });
  };
  return (
    <>
      <div
        class="admin-command-notices__status"
        role="status"
        aria-atomic="false"
        aria-relevant="additions text"
      >
        <For each={visible()} keyed={(notice) => notice.identity.commandId}>
          {(notice) => (
            <p>
              {receiptMessage(notice().identity, notice().result)}
              <Show when={receiptText(notice().result) !== null}>
                {" "}
                {receiptText(notice().result)}
              </Show>
            </p>
          )}
        </For>
      </div>
      <Show when={visible().length > 0}>
        <section
          class="admin-panel admin-pending admin-command-notices"
          aria-label={t("admin-pending-receipts")}
        >
          <ul class="admin-pending__list">
            {/* 直接生成同级回执，让关闭前一项时后续回执保留原按钮及焦点。 */}
            <For each={visible()} keyed={(notice) => notice.identity.commandId}>
              {(notice) => (
                <li class="admin-command-notices__receipt">
                  <p>{receiptMessage(notice().identity, notice().result)}</p>
                  <Show when={receiptText(notice().result) !== null}>
                    <p class="admin-panel__note">{receiptText(notice().result)}</p>
                  </Show>
                  <Show
                    when={generatedKeyOf(notice().result)}
                    fallback={
                      <button
                        id={recordButtonId(notice().identity.commandId, "receipt")}
                        type="button"
                        onClick={() => closeReceipt(props.host, notice().identity.commandId, tasks)}
                      >
                        {t("admin-pending-close-receipt")}
                      </button>
                    }
                  >
                    <button
                      id={recordButtonId(notice().identity.commandId, "receipt")}
                      type="button"
                      onClick={() => openKey.set(notice().identity.commandId)}
                    >
                      {t("admin-pending-view-key")}
                    </button>
                  </Show>
                </li>
              )}
            </For>
          </ul>
        </section>
      </Show>
      <Show when={activeKey()}>
        {(active) => (
          <AdminUserGeneratedKeyDialog
            generated={active().key}
            onClose={() => {
              openKey.set(null);
              closeReceipt(props.host, active().commandId, tasks);
            }}
          />
        )}
      </Show>
    </>
  );
}

function pendingCommandHostKeyInSource(host: PendingCommandHost, notice: CommandNotice): boolean {
  return notice.keySource !== null && liveSource(host, notice.keySource) !== null;
}

function closeReceipt(host: PendingCommandHost, commandId: string, tasks: TaskScope): void {
  const focus = focusTargetsAfter(host, commandId);
  if (host.dismiss(commandId)) {
    focusAfterRemoval(tasks, focus);
  }
}

function recordButtonId(commandId: string, action: string): string {
  return `admin-command-${action}-${commandId}`;
}

function focusTargetsAfter(host: PendingCommandHost, removed: string): readonly string[] {
  const writesAllowed = adminSessionWritesAllowed(host.sessionHost);
  const entries = host.entries.peek();
  const notices = host.notices.peek();
  const rows: { readonly commandId: string; readonly buttons: readonly (string | null)[] }[] = [];
  for (const entry of entries) {
    if (!entry.unknown) {
      continue;
    }
    const actionable = writesAllowed && !entry.busy;
    rows.push({
      commandId: entry.identity.commandId,
      buttons: [
        actionable && !pendingCommandRetryBlocked(entry, host) ? "retry" : null,
        actionable ? "abandon" : null,
      ],
    });
  }
  for (const notice of notices) {
    if (pendingCommandHostKeyInSource(host, notice)) {
      continue;
    }
    rows.push({ commandId: notice.identity.commandId, buttons: ["receipt", null] });
  }
  const index = rows.findIndex((row) => row.commandId === removed);
  let ordered = rows;
  if (index >= 0 && rows.length > 0) {
    const next = (index + 1) % rows.length;
    ordered = rows.slice(next).concat(rows.slice(0, next));
  }
  const targets: string[] = [];
  for (const row of ordered) {
    if (row.commandId === removed) {
      continue;
    }
    for (const button of row.buttons) {
      if (button !== null) {
        targets.push(recordButtonId(row.commandId, button));
      }
    }
  }
  targets.push("admin-content");
  return targets;
}

function focusAfterRemoval(tasks: TaskScope, targets: readonly string[]): void {
  tasks.spawn(async (context) => {
    // 先让组件移除条目与弹窗，避免焦点陷阱把焦点送回已消失的打开按钮。
    if (!(await sleep(0, context.signal)) || !context.alive()) {
      return;
    }
    if (typeof document === "undefined") {
      return;
    }
    for (const target of targets) {
      const element = document.getElementById(target);
      if (element !== null && !element.matches(":disabled")) {
        focusElementById(target);
        break;
      }
    }
  });
}

function receiptMessage(
  identity: CommandIdentity,
  result: Result<CommandReceipt | null, string>,
): string {
  const action = identity.action;
  const entity = identity.entity;
  if (result.ok) {
    return t("admin-pending-success-receipt", { action, entity });
  }
  return t("admin-pending-rejection-receipt", {
    action,
    entity,
    reason: adminApiErrorMessage(result.error),
  });
}

function receiptText(result: Result<CommandReceipt | null, string>): string | null {
  if (!result.ok || result.value === null || result.value.kind !== "text") {
    return null;
  }
  return result.value.text;
}

function generatedKeyOf(
  result: Result<CommandReceipt | null, string>,
): AdminUserGeneratedKey | null {
  if (!result.ok || result.value === null || result.value.kind !== "generated-key") {
    return null;
  }
  // 页面与对话框只拿到独立的只读副本，不能改写宿主持有的一次性 Key；
  // 复制保留脱敏封装，不能退化成含明文键的普通对象。
  return result.value.key.copy();
}
