// Admin 邮件轮询触发和暂停/恢复操作。

import type { Cell } from "@auction/web-shared/reactive/cell";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { err, ok, type Result } from "@auction/web-shared/result";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { onCleanup, Show } from "solid-js";
import type { AdminMailPolling } from "../../api/mail";
import {
  prepareMailPauseCommand,
  prepareMailPollTriggerCommand,
  setMailPaused,
  triggerMailPoll,
} from "../../api/mail";
import { useAdminSessionHost } from "../auth/shell";
import {
  commandDeliveryReported,
  PendingCommandsContext,
  requirePendingCommands,
  setIfAlive,
  trySetIfAlive,
} from "../pending";
import { PendingCommandPanel } from "../pending/panel";
import { usePendingCommands } from "../pending/sources";
import { adminApiErrorMessage, formatAdminTime } from "../presentation";
import { MAILBOX_ENTITY_PREFIX, mailboxCommandEntity } from ".";
import { MailPollingProgress } from "./progress";

/** 立即轮询的本地结果；成功时携带本次唤醒的 poller 数。 */
export type MailNotice = Result<number, string>;

/** 轮询动作区属性；命令宿主按活动邮箱筛选实体。 */
export interface MailPollingCommandsProps {
  readonly refresh: Cell<bigint>;
  readonly activeMailbox: Cell<string>;
  readonly children: JSX.Element;
}

/** 轮询卡片可以随邮箱切换，命令宿主始终保留；成功只刷新轮询状态，不干扰入站投递。 */
export function MailPollingCommands(props: MailPollingCommandsProps): JSX.Element {
  const owner = usePendingCommands(props.refresh, (entity: string) => {
    if (entity === "mail-poll-trigger") {
      return true;
    }
    return (
      entity.startsWith(MAILBOX_ENTITY_PREFIX) &&
      entity.slice(MAILBOX_ENTITY_PREFIX.length) === props.activeMailbox.peek()
    );
  });
  return (
    <PendingCommandsContext value={owner}>
      <PendingCommandPanel owner={owner} />
      {props.children}
    </PendingCommandsContext>
  );
}

/** 立即轮询按钮，以及本地的成功计数与失败提示。 */
export function MailTriggerButton(): JSX.Element {
  const owner = requirePendingCommands();
  const host = useAdminSessionHost();
  const busy = (): boolean => owner.isBusy("mail-poll-trigger");
  const blocked = (): boolean => owner.contains("mail-poll-trigger");
  const lifetime = createLifetimeToken();
  const notice = createCell<MailNotice | null>(null);
  const accepted = createValueMemo(() => {
    const current = notice();
    return current?.ok ? { pollers: current.value } : null;
  });
  const failure = createValueMemo(() => {
    const current = notice();
    return current !== null && !current.ok ? current.error : null;
  });
  return (
    <>
      <button
        class="admin-btn-primary"
        type="button"
        disabled={blocked()}
        onClick={() => {
          if (blocked()) {
            return;
          }
          notice.set(null);
          const prepare = prepareMailPollTriggerCommand();
          owner.submitWithReceipt(
            "mail-poll-trigger",
            t("admin-mail-trigger"),
            async (commandId, signal) => {
              const prepared = prepare(commandId);
              if (!prepared.ok) {
                return err(prepared.error.code);
              }
              const result = await triggerMailPoll(host, prepared.value, signal);
              return result.ok ? ok(result.value) : err(result.error.code);
            },
            (pollers, sourceAlive) => {
              const delivered = sourceAlive && trySetIfAlive(notice, ok(pollers), lifetime.active);
              return commandDeliveryReported(delivered, () => ({
                kind: "text",
                text: t("admin-mail-trigger-accepted", { pollers }),
              }));
            },
            (code) => {
              setIfAlive(notice, err(code), lifetime.active);
            },
          );
        }}
      >
        {busy() ? t("admin-action-working") : t("admin-mail-trigger")}
      </button>
      <div role="status">
        <Show when={accepted()}>
          {(current) => (
            <p class="admin-panel__note action-feedback action-feedback--success">
              {t("admin-mail-trigger-accepted", { pollers: current().pollers })}
            </p>
          )}
        </Show>
      </div>
      <Show when={failure()}>
        {(code) => (
          <p class="admin-panel__note action-feedback action-feedback--error" role="alert">
            {adminApiErrorMessage(code())}
          </p>
        )}
      </Show>
    </>
  );
}

interface MailPollingCardProps {
  readonly polling: AdminMailPolling;
  readonly refresh: Cell<bigint>;
  readonly loadedGeneration: bigint;
}

/** 单个邮箱的轮询状态卡；调用方必须以 `mailbox_key` 作列表 key，切换邮箱时销毁旧卡片的局部反馈。 */
export function MailPollingCard(props: MailPollingCardProps): JSX.Element {
  const owner = requirePendingCommands();
  const host = useAdminSessionHost();
  const lifetime = createLifetimeToken();
  const errorCode = createCell<string | null>(null);
  const entity = createValueMemo(() => mailboxCommandEntity(props.polling.mailbox_key));
  const busy = (): boolean => owner.isBusy(entity());
  // 切换回来也须等命令触发的 GET，不能按刚刚失效的邮箱版本分配新 UUID。
  const readIsCurrent = (): boolean => props.refresh() === props.loadedGeneration;
  const blocked = (): boolean => owner.contains(entity());
  const stateLabel = createValueMemo(() =>
    props.polling.paused ? t("admin-mail-paused") : t("admin-mail-running"),
  );
  const actionLabel = createValueMemo(() =>
    props.polling.paused ? t("admin-mail-resume") : t("admin-mail-pause"),
  );
  const buttonLabel = createValueMemo(() => (busy() ? t("admin-action-working") : actionLabel()));
  return (
    <article class="admin-mail-state" aria-busy={busy() ? "true" : "false"}>
      <h3>{props.polling.mailbox_key}</h3>
      <dl>
        <div>
          <dt>{t("admin-mail-polling-state")}</dt>
          <dd>{stateLabel()}</dd>
        </div>
        <div>
          <dt>{t("admin-mail-polling-failures")}</dt>
          <dd>{props.polling.consecutive_failures}</dd>
        </div>
        <div>
          <dt>{t("admin-mail-polling-last")}</dt>
          <dd>{formatAdminTime(props.polling.last_poll_at)}</dd>
        </div>
        <div>
          <dt>{t("admin-mail-polling-next")}</dt>
          <dd>{formatAdminTime(props.polling.next_poll_at)}</dd>
        </div>
      </dl>
      <MailPollingProgress diagnostics={props.polling.diagnostics} />
      <button
        type="button"
        disabled={blocked() || !readIsCurrent()}
        onClick={() => {
          if (!readIsCurrent() || owner.contains(entity())) {
            return;
          }
          errorCode.set(null);
          // 卡片随邮箱切换卸载，已提交暂停/恢复仍由会话宿主保留原负载和结果。
          const polling = props.polling;
          const frozenEntity = entity();
          const mailboxKey = polling.mailbox_key;
          const expectedStateVersion = polling.state_version;
          const desiredPaused = !polling.paused;
          const prepare = prepareMailPauseCommand(mailboxKey, expectedStateVersion, desiredPaused);
          owner.submitWithReceipt(
            frozenEntity,
            actionLabel(),
            async (commandId, signal) => {
              const prepared = prepare(commandId);
              if (!prepared.ok) {
                return err(prepared.error.code);
              }
              const result = await setMailPaused(host, prepared.value, signal);
              return result.ok ? ok(result.value) : err(result.error.code);
            },
            (_updated, sourceAlive) => {
              if (sourceAlive) {
                trySetIfAlive(errorCode, null, lifetime.active);
              }
              // 成功与邮箱绑定，由会话交付，卡片读取失败或切换邮箱都可以卸载卡片。
              return commandDeliveryReported(false, () => ({
                kind: "text",
                text: t("admin-mail-action-success"),
              }));
            },
            (code) => {
              setIfAlive(errorCode, code, lifetime.active);
            },
          );
        }}
      >
        {buttonLabel()}
      </button>
      <Show when={errorCode()}>
        {(code) => (
          <p class="admin-panel__note action-feedback action-feedback--error" role="alert">
            {adminApiErrorMessage(code())}
          </p>
        )}
      </Show>
    </article>
  );
}

interface LifetimeToken {
  active(): boolean;
}

/** 组件卸载后仍可能收到命令回执；写局部反馈前检查自身 owner 是否仍存活。 */
function createLifetimeToken(): LifetimeToken {
  let active = true;
  onCleanup(() => {
    active = false;
  });
  return { active: () => active };
}
