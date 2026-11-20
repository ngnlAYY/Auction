// 运行时开关卡片：展示数据库意图与本进程实际值，并提供外部访问暂停控制。

import type {
  RuntimeSwitchIntentDto,
  RuntimeSwitchListResponse,
} from "@auction/contracts/dto/admin/runtime-switches";
import { sleep } from "@auction/web-shared/platform/timer";
import { createPageVisibility } from "@auction/web-shared/platform/visibility";
import { type Cell, createCell } from "@auction/web-shared/reactive/cell";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import { err, type Result } from "@auction/web-shared/result";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Match, onSettled, Show, Switch } from "solid-js";
import { type AdminWriteFailure, writeOutcomeUnknown } from "../../api/failure";
import {
  EXTERNAL_ACCESS_PAUSE_MAX_REASON_BYTES,
  type ExternalAccessPauseAttempt,
  fetchRuntimeSwitches,
  prepareExternalAccessPauseOnce,
  submitExternalAccessPause,
  validExternalAccessPauseReason,
} from "../../api/runtime-switches";
import type { AdminSessionHost } from "../../api/session";
import { trimRustWhitespace } from "../../api/session-time";
import { useAdminSessionHost } from "../auth/shell";
import {
  type CommandDelivery,
  commandDeliveryReported,
  type PendingCommands,
  setIfAlive,
} from "../pending";
import { PendingCommandPanel } from "../pending/panel";
import { createCommandReload, usePendingCommands } from "../pending/sources";
import { adminApiErrorMessage } from "../presentation";
import { FieldLiveRegion, useFieldAnnouncer } from "./field-announcement";
import {
  displayTimestamp,
  formatOneDecimal,
  formatTimestamp,
  incrementUint64,
  PAUSE_HOURS,
  parsePauseHours,
  pauseExpiry,
  pauseRefreshMillis,
  pauseState,
  reasonErrorText,
  rustJsonText,
} from "./runtime-switches-values";

const PAUSE_ENTITY = "runtime-switch:external_access_paused";
const PAUSE_REASON_ID = "admin-pause-reason";
/** 浏览器定时器的单次上限；较长期限分片等待。 */
const MAX_TIMER_SLICE_MILLIS = 2_147_483_647;

interface PauseConfirmation {
  readonly expectedRevision: number;
  readonly paused: boolean;
  readonly expiresAt: bigint | null;
  readonly reason: string;
}
interface PauseControlState {
  readonly pending: PendingCommands;
  readonly confirmation: Cell<PauseConfirmation | null>;
  readonly hours: Cell<number>;
  readonly reason: Cell<string>;
  readonly accepted: Cell<boolean>;
  submit(confirmation: PauseConfirmation): void;
}

/** 创建暂停卡片的状态与提交入口；宿主句柄由组件体显式传入。 */
function createPauseControlState(
  host: AdminSessionHost,
  pending: PendingCommands,
): PauseControlState {
  const confirmation = createCell<PauseConfirmation | null>(null);
  const hours = createCell(2);
  const reason = createCell("");
  const accepted = createCell(false);

  const submit = (candidate: PauseConfirmation): void => {
    const current = confirmation.peek();
    if (!samePauseConfirmation(current, candidate)) {
      return;
    }
    const action = candidate.paused ? t("admin-pause-start") : t("admin-pause-resume");
    let prepared: ReturnType<typeof prepareExternalAccessPauseOnce> | null = null;
    const acceptedByHost = pending.submitWithReceipt(
      PAUSE_ENTITY,
      action,
      async (commandId, signal): Promise<Result<RuntimeSwitchIntentDto, AdminWriteFailure>> => {
        try {
          let prepare = prepared;
          if (prepare === null) {
            const attempt: ExternalAccessPauseAttempt = {
              commandId,
              expectedRevision: candidate.expectedRevision,
              paused: candidate.paused,
              expiresAt:
                candidate.expiresAt === null ? null : formatTimestamp(candidate.expiresAt, true),
              reason: candidate.reason,
            };
            prepare = prepareExternalAccessPauseOnce(attempt);
            prepared = prepare;
          }
          const command = prepare(commandId);
          if (!command.ok) {
            return command;
          }
          const result = await submitExternalAccessPause(host, command.value, signal);
          return result;
        } catch {
          // 执行体异常按结果未知处理；异常对象与消息不进入控制台。
          return err(writeOutcomeUnknown("network_error"));
        }
      },
      (_value, sourceAlive): CommandDelivery => {
        if (sourceAlive) {
          setIfAlive(accepted, true, () => sourceAlive);
          setIfAlive(reason, "", () => sourceAlive);
        }
        return commandDeliveryReported(sourceAlive, () => ({
          kind: "text",
          text: t("admin-pause-done"),
        }));
      },
      () => undefined,
    );
    if (acceptedByHost) {
      confirmation.set(null);
    }
  };

  return { pending, confirmation, hours, reason, accepted, submit };
}

function samePauseConfirmation(left: PauseConfirmation | null, right: PauseConfirmation): boolean {
  return (
    left !== null &&
    left.expectedRevision === right.expectedRevision &&
    left.paused === right.paused &&
    left.expiresAt === right.expiresAt &&
    left.reason === right.reason
  );
}
/** 运行时开关卡片；读取世代与命令世代分开，手动刷新不丢弃确认草稿。 */
export function RuntimeSwitchCard(): JSX.Element {
  const host = useAdminSessionHost();
  const refresh = createCell(0n);
  // 只把命令了结后的读取作为新意图的版本围栏；手动刷新期间保留原草稿和确认。
  const commandRefresh = createCommandReload(0n);
  const pending = usePendingCommands(commandRefresh, (entity) => entity === PAUSE_ENTITY);
  const pause = createPauseControlState(host, pending);

  const snapshot = createTaskResource(
    () => [refresh(), commandRefresh()] as const,
    async (generation, context) => {
      try {
        const result = await fetchRuntimeSwitches(host, context.signal);
        return { generation, result };
      } catch {
        return { generation, result: err("network_error") };
      }
    },
  );

  const visibility = createPageVisibility();
  onSettled(() => {
    visibility.install(() => {
      refresh.set(incrementUint64(refresh.peek()));
    });
    return () => visibility.cleanup();
  });

  createTaskResource(
    () => {
      const generation = [refresh(), commandRefresh()] as const;
      const entry = snapshot();
      const delay =
        entry !== undefined &&
        entry.generation[0] === generation[0] &&
        entry.generation[1] === generation[1] &&
        entry.result.ok
          ? pauseRefreshMillis(entry.result.value)
          : null;
      return { generation, delay };
    },
    async ({ generation, delay }, context) => {
      if (delay === null) {
        return;
      }
      if (!context.alive()) {
        return;
      }
      let remaining = delay;
      // CLI 可写入较长的期限，需分片等待以免超出浏览器定时器上限。
      while (remaining > 0) {
        const slice = Math.min(remaining, MAX_TIMER_SLICE_MILLIS);
        if (!(await sleep(slice, context.signal)) || !context.alive()) {
          return;
        }
        remaining -= slice;
      }
      // 定时器仅唤醒读取，由数据库快照确认到期。复核已开始时旧任务不得再次刷新。
      if (refresh.peek() === generation[0] && commandRefresh.peek() === generation[1]) {
        refresh.set(incrementUint64(generation[0]));
      }
    },
  );

  const current = () => snapshot();
  const failure = (): { readonly code: string } | null => {
    const entry = current();
    return entry !== undefined && !entry.result.ok ? { code: entry.result.error } : null;
  };
  const loaded = (): {
    readonly generation: readonly [bigint, bigint];
    readonly data: RuntimeSwitchListResponse;
  } | null => {
    const entry = current();
    return entry?.result.ok ? { generation: entry.generation, data: entry.result.value } : null;
  };

  return (
    <section class="admin-runtime-switches admin-panel">
      <h2>{t("admin-switches-title")}</h2>
      <p class="admin-panel__note">{t("admin-switches-note")}</p>
      <button
        class="admin-btn-secondary"
        type="button"
        onClick={() => refresh.set(incrementUint64(refresh.peek()))}
      >
        {t("admin-switches-refresh")}
      </button>
      <PendingCommandPanel owner={pause.pending} />
      <div role="status">
        <Show when={pause.accepted()}>
          <p class="admin-shell__banner">{t("admin-pause-done")}</p>
        </Show>
      </div>
      <Switch
        fallback={<AsyncState kind="loading" title={t("admin-switches-loading")} detail="" />}
      >
        <Match when={failure()}>
          {(failed) => (
            <AsyncState kind="error" title={adminApiErrorMessage(failed().code)} detail="" />
          )}
        </Match>
        <Match when={loaded()}>
          {(entry) => {
            const data = () => entry().data;
            const pauseInfo = () => pauseState(data());
            const ageLabel = (): string => {
              const age = data().admin_last_success_age_seconds;
              return age === null ? t("admin-switches-unconfirmed") : `${formatOneDecimal(age)} s`;
            };
            const stale = (): boolean => {
              const age = data().admin_last_success_age_seconds;
              return age === null || age > 10;
            };
            return (
              <>
                <ExternalAccessPauseControl
                  state={pause}
                  revision={pauseInfo().revision}
                  paused={pauseInfo().paused}
                  pausedUntil={pauseInfo().pausedUntil}
                  commandRefresh={commandRefresh}
                  loadedGeneration={entry().generation[1]}
                />
                <p>
                  {t("admin-switches-age")}
                  {": "}
                  {ageLabel()}
                </p>
                <Show when={stale()}>
                  <p class="admin-panel__note">{t("admin-switches-stale")}</p>
                </Show>
                <For each={data().database_intent} keyed={false}>
                  {(intent) => (
                    <article class="admin-runtime-switches__entry">
                      <h3>{intent().name}</h3>
                      <p>
                        {t("admin-switches-intent")}
                        {": "}
                        {intent().operation ?? t("admin-switches-unset")}
                        {" / "}
                        {intent().value === null ? "—" : rustJsonText(intent().value)}
                        {` / r${intent().revision}`}
                      </p>
                      <p>
                        {t("admin-switches-expiry")}
                        {": "}
                        {displayTimestamp(intent().expires_at)}
                      </p>
                      <p>
                        {t("admin-switches-actor")}
                        {": "}
                        {intent().actor_id ?? "—"}
                        {" / "}
                        {intent().entrypoint ?? "—"}
                      </p>
                      <p>
                        {t("admin-switches-reason")}
                        {": "}
                        {intent().reason ?? "—"}
                      </p>
                      <For
                        keyed={false}
                        each={data().admin_applied.filter((value) => value.name === intent().name)}
                      >
                        {(applied) => (
                          <p>
                            {t("admin-switches-applied")}
                            {": "}
                            {rustJsonText(applied().effective_value)}
                            {` / r${applied().revision} / `}
                            {t("admin-switches-baseline")}
                            {": "}
                            {rustJsonText(applied().toml_baseline)}
                          </p>
                        )}
                      </For>
                    </article>
                  )}
                </For>
              </>
            );
          }}
        </Match>
      </Switch>
    </section>
  );
}

/** 外部访问暂停的确认与表单；确认冻结版本、期限和原因，重试只复用冻结值。 */
function ExternalAccessPauseControl(props: {
  readonly state: PauseControlState;
  readonly revision: number;
  readonly paused: boolean;
  readonly pausedUntil: bigint | null;
  readonly commandRefresh: Cell<bigint>;
  readonly loadedGeneration: bigint;
}): JSX.Element {
  const announcer = useFieldAnnouncer();
  const reasonEdited = createCell(false);
  const reasonInvalid = createCell(false);
  const occupied = () => props.state.pending.contains(PAUSE_ENTITY);
  const waitingForSnapshot = () => props.loadedGeneration !== props.commandRefresh();
  const draft = () => props.state.confirmation();
  const stale = (): boolean => {
    const current = draft();
    return (
      current !== null &&
      (current.expectedRevision !== props.revision || current.paused === props.paused)
    );
  };
  const entryLabel = () => (props.paused ? t("admin-pause-resume") : t("admin-pause-start"));
  const pausedUntilLabel = (): string => {
    if (props.pausedUntil === null) {
      return t("admin-pause-no-expiry");
    }
    return formatTimestamp(props.pausedUntil, false) ?? "";
  };
  const draftExpiry = (): string => {
    const current = draft();
    return current === null || current.expiresAt === null
      ? ""
      : (formatTimestamp(current.expiresAt, false) ?? "");
  };

  const begin = (event: SubmitEvent): void => {
    event.preventDefault();
    if (occupied() || props.state.confirmation.peek() !== null || waitingForSnapshot()) {
      return;
    }
    const reasonText = trimRustWhitespace(props.state.reason());
    if (!validExternalAccessPauseReason(reasonText)) {
      reasonInvalid.set(true);
      announcer.invalidSubmit(PAUSE_REASON_ID, reasonErrorText(), true);
      return;
    }
    reasonInvalid.set(false);
    announcer.clear();
    reasonEdited.set(false);
    const expiresAt = props.paused ? null : pauseExpiry(props.state.hours());
    if (!props.paused && expiresAt === null) {
      return;
    }
    props.state.accepted.set(false);
    props.state.confirmation.set({
      expectedRevision: props.revision,
      paused: !props.paused,
      expiresAt,
      reason: reasonText,
    });
  };

  return (
    <>
      <form
        class="admin-runtime-switches__entry admin-runtime-pause"
        method="post"
        action="/api/v1/admin/runtime-switches/external-access-paused"
        aria-labelledby="admin-runtime-pause-title"
        novalidate={true}
        onSubmit={begin}
      >
        <h3 id="admin-runtime-pause-title">{t("admin-pause-title")}</h3>
        <p class="admin-panel__note">{t("admin-pause-note")}</p>
        <Show when={props.paused} fallback={<p>{t("admin-pause-inactive")}</p>}>
          <p>
            {t("admin-pause-active")}
            {": "}
            {pausedUntilLabel()}
          </p>
        </Show>
        <Show when={draft()}>
          {(current) => (
            <>
              <Show when={current().paused} fallback={<p>{t("admin-pause-confirm-resume")}</p>}>
                <p>
                  {t("admin-pause-confirm-pause")}
                  {": "}
                  {draftExpiry()}
                </p>
              </Show>
              <button
                class={current().paused ? "admin-btn-danger" : "admin-btn-primary"}
                type="button"
                disabled={occupied() || waitingForSnapshot() || stale()}
                onClick={() => {
                  const confirmed = current();
                  if (!waitingForSnapshot() && !stale()) {
                    props.state.submit(confirmed);
                  }
                }}
              >
                {t("admin-pause-confirm")}
              </button>
              <button
                class="admin-btn-secondary"
                type="button"
                onClick={() => props.state.confirmation.set(null)}
              >
                {t("admin-pause-cancel")}
              </button>
            </>
          )}
        </Show>
        <Show when={draft() === null}>
          <Show when={!props.paused}>
            <label>
              {t("admin-pause-duration")}
              <select
                name="duration"
                disabled={occupied()}
                value={String(props.state.hours())}
                onChange={(event) => {
                  const selected = parsePauseHours(event.currentTarget.value);
                  props.state.hours.set(selected ?? 2);
                }}
              >
                <For each={PAUSE_HOURS} keyed={false}>
                  {(hours) => <option value={String(hours())}>{`${hours()} h`}</option>}
                </For>
              </select>
            </label>
          </Show>
          <div>
            <label for={PAUSE_REASON_ID}>
              {t("admin-pause-reason", { maximum: EXTERNAL_ACCESS_PAUSE_MAX_REASON_BYTES })}
              <input
                id={PAUSE_REASON_ID}
                name="reason"
                type="text"
                required={true}
                disabled={occupied()}
                aria-invalid={reasonInvalid() ? "true" : "false"}
                aria-describedby={reasonInvalid() ? "admin-pause-reason-error" : undefined}
                value={props.state.reason()}
                onInput={(event) => {
                  const value = event.currentTarget.value;
                  if (
                    reasonInvalid() &&
                    validExternalAccessPauseReason(trimRustWhitespace(value))
                  ) {
                    reasonInvalid.set(false);
                    announcer.clearField(PAUSE_REASON_ID);
                  }
                  reasonEdited.set(true);
                  props.state.reason.set(value);
                }}
                onBlur={() => {
                  if (
                    reasonEdited() &&
                    !reasonInvalid() &&
                    !validExternalAccessPauseReason(trimRustWhitespace(props.state.reason()))
                  ) {
                    reasonInvalid.set(true);
                    announcer.announceOnBlur(PAUSE_REASON_ID, reasonErrorText());
                  }
                }}
              />
            </label>
            <Show when={reasonInvalid()}>
              <p class="admin-field-error" id="admin-pause-reason-error">
                {reasonErrorText()}
              </p>
            </Show>
          </div>
          <button
            class={props.paused ? "admin-btn-primary" : "admin-btn-danger"}
            type="submit"
            disabled={occupied() || waitingForSnapshot()}
          >
            {entryLabel()}
          </button>
        </Show>
      </form>
      <FieldLiveRegion announcer={announcer} />
    </>
  );
}
