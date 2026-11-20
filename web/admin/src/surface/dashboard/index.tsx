// Admin 管理总览页面。
// 旧版 Dashboard 只读取一次聚合快照；各项指标、邮件控制和局部降级都从该快照派生，不能再
// 额外请求列表接口拼装第二套页面状态。

import type { Cell } from "@auction/web-shared/reactive/cell";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import { err } from "@auction/web-shared/result";
import { useAppBootReady } from "@auction/web-shared/ui/bootstrap";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, onCleanup, Show, useContext } from "solid-js";
import { fetchDashboard } from "../../api/dashboard";
import { observedResponseStatus, writeOutcomeUnknown } from "../../api/failure";
import {
  type AdminMailPolling,
  prepareMailPauseCommand,
  prepareMailPollTriggerCommand,
  setMailPaused,
  triggerMailPoll,
} from "../../api/mail";
import { AdminReadinessContext } from "../auth";
import { useAdminSessionHost } from "../auth/shell";
import { MAILBOX_ENTITY_PREFIX, mailboxCommandEntity } from "../mail";
import {
  type PendingCommands,
  PendingCommandsContext,
  requirePendingCommands,
  setIfAlive,
} from "../pending";
import { PendingCommandPanel } from "../pending/panel";
import { advanceCommandReload, createCommandReload, usePendingCommands } from "../pending/sources";
import { AutoPollControls, useAutoPoll } from "../poll";
import { adminApiErrorMessage } from "../presentation";
import { useDashboardQueryState } from "../query/dashboard";
import { DashboardContent } from "./content";

/** 按旧版单快照请求模型加载管理总览。 */
export function page(): JSX.Element {
  const host = useAdminSessionHost();
  const refresh = createCommandReload(0n);
  const pending = usePendingCommands(
    refresh,
    (entity) => entity === "mail-poll-trigger" || entity.startsWith(MAILBOX_ENTITY_PREFIX),
  );
  const dashboard = createTaskResource(
    () => refresh(),
    async (generation, context) =>
      [generation, await fetchDashboard(host, context.signal)] as const,
  );
  // 自动轮询开关与间隔入 URL（`poll` 键），默认 30 秒；页面隐藏时轮询挂起。
  // 轮询只重取当前快照，不额外写地址栏。
  const query = useDashboardQueryState();
  const queryState = query.state;
  const pollSeconds = createValueMemo(() => BigInt(queryState().pollSeconds));
  useAutoPoll(pollSeconds, () => {
    // 手动刷新或命令刚触发的 GET 仍在进行时，轮询不能取消它并重新计时。
    if (dashboard.state() === "ready") {
      advanceCommandReload(refresh);
    }
  });
  const readiness = useContext(AdminReadinessContext);
  const manualRefresh = (): void => {
    advanceCommandReload(refresh);
    // 用外壳注入的同一份就绪资源复核；正在进行的检查继续完成，不重启请求。
    if (readiness.state() !== "pending") {
      readiness.restart();
    }
  };
  // 默认管理首页的聚合快照无论成功或失败，只要已有结果就一次性揭示真实页面。
  useAppBootReady(() => readiness() !== undefined && dashboard() !== undefined);
  const state = createValueMemo(() => dashboard());
  const error = createValueMemo((): string | null => {
    const current = state();
    if (current === undefined) {
      return null;
    }
    return current[1].ok ? null : current[1].error;
  });
  const loaded = createValueMemo(() => {
    const current = state();
    if (current === undefined) {
      return null;
    }
    return current[1].ok ? { generation: current[0], snapshot: current[1].value } : null;
  });

  return (
    <PendingCommandsContext value={pending}>
      <section class="admin-dashboard-page" aria-labelledby="dashboard-title">
        <div class="admin-dashboard-heading">
          <h1 id="dashboard-title">{t("page-dashboard-title")}</h1>
          <div class="admin-dashboard-heading__actions">
            <AutoPollControls
              pollSeconds={pollSeconds()}
              onChange={(seconds) => {
                // 开关与间隔变更立即生效并写回地址栏。
                query.push({ ...queryState(), pollSeconds: Number(seconds) });
              }}
            />
            <button type="button" class="admin-btn-secondary" onClick={manualRefresh}>
              {t("dashboard-refresh")}
            </button>
          </div>
        </div>
        <PendingCommandPanel owner={pending} />
        <Show when={state() === undefined}>
          <AsyncState kind="loading" title={t("dashboard-legacy-loading")} detail="" />
        </Show>
        <Show when={error()}>
          {(code) => (
            <AsyncState
              kind="error"
              title={adminApiErrorMessage(code())}
              detail=""
              retryLabel={t("admin-retry")}
              onRetry={manualRefresh}
            />
          )}
        </Show>
        <Show when={loaded()}>
          {(data) => (
            <DashboardContent snapshot={data().snapshot}>
              <MailPollerControls
                pollers={data().snapshot.mail.data?.pollers ?? []}
                refresh={refresh}
                loadedGeneration={data().generation}
              />
            </DashboardContent>
          )}
        </Show>
      </section>
    </PendingCommandsContext>
  );
}

/** Admin Dashboard 邮件轮询控制。 */
interface MailPollerControlProps {
  readonly pollers: readonly AdminMailPolling[];
  readonly refresh: Cell<bigint>;
  readonly loadedGeneration: bigint;
}

interface PollerButton {
  readonly polling: AdminMailPolling;
  readonly label: string;
  readonly entity: string;
}

// 命令由认证会话持有，后台快照失败卸载控件时仍保留原参数与 UUID；
// 暂停冲突会刷新唯一 Dashboard 快照，但不会把 stale 结果伪装成成功。
function MailPollerControls(props: MailPollerControlProps): JSX.Element {
  const host = useAdminSessionHost();
  const owner: PendingCommands = requirePendingCommands();
  const errorCode = createCell<string | null>(null);
  let alive = true;
  onCleanup(() => {
    alive = false;
  });
  // 暂停与恢复携带邮箱版本，快照被命令失效后先读同一邮箱的新版本。
  const readIsCurrent = (): boolean => props.refresh() === props.loadedGeneration;
  // 按钮文案在 JSX 外配好，循环体根节点才能直接是按邮箱带 key 的按钮。
  const pollerButtons = createValueMemo((): readonly PollerButton[] =>
    props.pollers.map((polling) => {
      const action = polling.paused
        ? t("dashboard-legacy-resume-poll")
        : t("dashboard-legacy-pause-poll");
      return {
        polling,
        label: t("dashboard-legacy-poller-action", {
          action,
          mailbox: polling.mailbox_key,
        }),
        entity: mailboxCommandEntity(polling.mailbox_key),
      };
    }),
  );
  const errorDisplay = createValueMemo(() => {
    const code = errorCode();
    return code === null ? null : adminApiErrorMessage(code);
  });

  return (
    <>
      <div class="admin-dashboard-actions">
        <button
          class="admin-btn-primary"
          type="button"
          disabled={owner.contains("mail-poll-trigger")}
          onClick={() => {
            if (owner.contains("mail-poll-trigger")) {
              return;
            }
            errorCode.set(null);
            const prepare = prepareMailPollTriggerCommand();
            owner.submitWithReceipt(
              "mail-poll-trigger",
              t("dashboard-legacy-trigger-poll"),
              async (commandId, signal) => {
                const prepared = prepare(commandId);
                if (!prepared.ok) {
                  return prepared;
                }
                const result = await triggerMailPoll(host, prepared.value, signal);
                return result;
              },
              (count, sourceAlive) => {
                if (sourceAlive) {
                  setIfAlive(errorCode, null, () => alive);
                }
                return {
                  kind: "receipt",
                  receipt: {
                    kind: "text",
                    text: t("dashboard-legacy-poll-triggered", { count }),
                  },
                };
              },
              (code) => {
                setIfAlive(errorCode, code, () => alive);
              },
            );
          }}
        >
          {t("dashboard-legacy-trigger-poll")}
        </button>
        <For each={pollerButtons()} keyed={(button) => button.polling.mailbox_key}>
          {(button) => (
            <button
              class="admin-btn-secondary"
              type="button"
              disabled={
                !readIsCurrent() ||
                owner.contains(button().entity) ||
                owner.containsLocal(MAILBOX_ENTITY_PREFIX)
              }
              onClick={() => {
                if (
                  !readIsCurrent() ||
                  owner.contains(button().entity) ||
                  owner.containsLocal(MAILBOX_ENTITY_PREFIX)
                ) {
                  return;
                }
                errorCode.set(null);
                const polling = button().polling;
                const entity = button().entity;
                const label = button().label;
                const desiredPaused = !polling.paused;
                const prepare = prepareMailPauseCommand(
                  polling.mailbox_key,
                  polling.state_version,
                  desiredPaused,
                );
                // 保留本页暂停/恢复串行策略；未知结果必须先原样解决。
                owner.submitWithReceipt(
                  entity,
                  label,
                  async (commandId, signal) => {
                    const prepared = prepare(commandId);
                    if (!prepared.ok) {
                      return prepared;
                    }
                    const result = await setMailPaused(host, prepared.value, signal);
                    // 任务取消后观察逻辑随 future 一起丢弃，不能推进页面刷新世代。
                    if (signal.aborted) {
                      return err(writeOutcomeUnknown("network_error"));
                    }
                    if (!result.ok && observedResponseStatus(result.error) === 409) {
                      advanceCommandReload(props.refresh);
                    }
                    return result;
                  },
                  (_updated, sourceAlive) => {
                    if (sourceAlive) {
                      setIfAlive(errorCode, null, () => alive);
                    }
                    // 聚合读取失败会卸载轮询控件，已接受的回执由会话交付。
                    return {
                      kind: "receipt",
                      receipt: {
                        kind: "text",
                        text: t("dashboard-legacy-poll-updated"),
                      },
                    };
                  },
                  (code) => {
                    setIfAlive(errorCode, code, () => alive);
                  },
                );
              }}
            >
              {button().label}
            </button>
          )}
        </For>
      </div>
      <Show when={errorDisplay()}>
        {(message) => (
          <p class="action-feedback action-feedback--error" role="alert">
            {message()}
          </p>
        )}
      </Show>
    </>
  );
}
