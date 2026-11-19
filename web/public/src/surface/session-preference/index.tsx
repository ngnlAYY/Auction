// “我的”登录时长设置；账号偏好与当前会话实际截止分开展示，不从永久默认值伪造已加载状态。

import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskScope } from "@auction/web-shared/reactive/tasks";
import { Time } from "@auction/web-shared/ui/components/time";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { createEffect, Show, useContext } from "solid-js";
import { commandNotSent } from "../../api/commands";
import { usePreferenceReload } from "../account/preference-reload";
import { PublicCommandOwnerContext, PublicSessionResourceContext } from "../index";
import { useContentLoading } from "../loading";
import { durationUnitFromValue, durationUnitMaximum, durationUnitValue } from "./duration";
import {
  type CurrentSessionExpiry,
  type PreferenceOwner,
  type PreferenceState,
  preferenceOwnerChange,
  preferenceOwnerLoad,
  preferenceOwnerSessionReady,
  preferenceOwnerSubmit,
  preferenceStateDefault,
} from "./state";

export interface SessionPreferenceProps {
  readonly userId: string;
}

function finiteExpiry(
  expiry: CurrentSessionExpiry | null,
): Extract<CurrentSessionExpiry, { kind: "finite" }> | null {
  return expiry?.kind === "finite" ? expiry : null;
}

/** 父页面按用户 UUID 隔离宿主；账号变化或卸载取消任务并销毁未决命令。 */
export function SessionPreference(props: SessionPreferenceProps): JSX.Element {
  const session = useContext(PublicSessionResourceContext);
  const identity = useContext(PublicCommandOwnerContext);
  const state = createCell<PreferenceState>(preferenceStateDefault());
  const tasks = createTaskScope();
  const owner: PreferenceOwner = {
    userId: props.userId,
    identity,
    session,
    state,
    tasks,
    readStarted: () => readStarted(),
  };
  const readStarted = usePreferenceReload(
    () => preferenceOwnerSessionReady(owner),
    () => state().busy || state().pendingUncertain,
    (preserveDraft) => preferenceOwnerLoad(owner, false, preserveDraft),
  );
  const currentExpiry = createValueMemo((): CurrentSessionExpiry | null => {
    const snapshot = session();
    if (snapshot === undefined || !snapshot.ok || snapshot.value === null) {
      return null;
    }
    if (!snapshot.value.authenticated) {
      return null;
    }
    return snapshot.value.expires_at === null
      ? { kind: "permanent" }
      : { kind: "finite", expiresAt: snapshot.value.expires_at };
  });
  createEffect(
    () => currentExpiry(),
    (expiry) => {
      // 只跟随实际截止变化，不因同身份重读重置用户草稿或未知命令；GET 可提供更新的截止。
      state.update((current) => ({ ...current, currentExpiresAt: expiry }));
    },
  );

  const busy = () => state().busy;
  const pending = () => state().pending !== null;
  const ready = () => preferenceOwnerSessionReady(owner) && state().authoritative !== null;
  // 读取偏好由顶栏提示；保存是本表单的命令反馈，留在表单内。
  useContentLoading(() => busy() && !pending());
  const controlsDisabled = () => !ready() || busy() || pending();
  const permanent = () => ready() && state().draft.permanent;
  const custom = () => ready() && !state().draft.permanent;
  const amount = () => state().draft.amount;
  const unit = () => durationUnitValue(state().draft.unit);
  const maximum = () => durationUnitMaximum(state().draft.unit);
  const validationError = () => {
    const snapshot = state();
    return (
      snapshot.validationError ||
      (!snapshot.pendingUncertain && snapshot.error?.code === "invalid_session_preference")
    );
  };
  const message = () => feedback(state());
  const uncertain = () => state().pendingUncertain;
  const confirmed = () => state().confirmed;
  const failed = () => state().error !== null;

  return (
    <section
      id="login-duration"
      class="public-session-preference"
      aria-busy={busy() ? "true" : "false"}
    >
      <form
        onSubmit={(event) => {
          event.preventDefault();
          preferenceOwnerSubmit(owner);
        }}
      >
        <fieldset aria-describedby="session-duration-hint session-duration-feedback">
          <legend>{t("session-duration-title")}</legend>
          <p id="session-duration-hint" class="public-session-preference__hint">
            {t("session-duration-hint")}
          </p>
          <div class="public-session-preference__choices">
            <div>
              <input
                id="session-duration-permanent"
                type="radio"
                name="session-duration-mode"
                value="permanent"
                checked={permanent()}
                disabled={controlsDisabled()}
                onChange={() => preferenceOwnerChange(owner, (draft) => (draft.permanent = true))}
              />
              <label for="session-duration-permanent">{t("session-duration-permanent")}</label>
            </div>
            <div>
              <input
                id="session-duration-custom"
                type="radio"
                name="session-duration-mode"
                value="custom"
                checked={custom()}
                disabled={controlsDisabled()}
                onChange={() => preferenceOwnerChange(owner, (draft) => (draft.permanent = false))}
              />
              <label for="session-duration-custom">{t("session-duration-custom")}</label>
            </div>
          </div>
          <Show when={custom()}>
            <div class="public-session-preference__duration">
              <div class="public-session-preference__field">
                <label for="session-duration-amount">{t("session-duration-amount")}</label>
                <input
                  id="session-duration-amount"
                  name="session-duration-amount"
                  type="number"
                  inputmode="decimal"
                  required={true}
                  min="0"
                  max={maximum()}
                  step="any"
                  value={amount()}
                  disabled={controlsDisabled()}
                  aria-invalid={validationError() ? "true" : "false"}
                  aria-describedby="session-duration-range session-duration-feedback"
                  onInput={(event) =>
                    preferenceOwnerChange(owner, (draft) => {
                      draft.amount = event.currentTarget.value;
                    })
                  }
                />
              </div>
              <div class="public-session-preference__field">
                <label for="session-duration-unit">{t("session-duration-unit")}</label>
                <select
                  id="session-duration-unit"
                  name="session-duration-unit"
                  value={unit()}
                  disabled={controlsDisabled()}
                  aria-describedby="session-duration-range session-duration-feedback"
                  onChange={(event) => {
                    const next = durationUnitFromValue(event.currentTarget.value);
                    if (next !== null) {
                      preferenceOwnerChange(owner, (draft) => {
                        draft.unit = next;
                      });
                    }
                  }}
                >
                  <option value="minutes">{t("session-duration-minutes")}</option>
                  <option value="hours">{t("session-duration-hours")}</option>
                  <option value="days">{t("session-duration-days")}</option>
                </select>
              </div>
            </div>
            <p id="session-duration-range" class="public-session-preference__hint">
              {t("session-duration-range")}
            </p>
          </Show>
          <Show when={state().currentExpiresAt}>
            {(expiry) => (
              <p class="public-session-preference__current">
                <span>{t("session-duration-current")}</span>
                <Show
                  keyed
                  when={finiteExpiry(expiry())}
                  fallback={<span>{t("session-duration-current-permanent")}</span>}
                >
                  {(finite) => <Time value={finite.expiresAt} />}
                </Show>
              </p>
            )}
          </Show>
          <p class="public-session-preference__hint">{t("session-duration-security")}</p>
          <div class="public-session-preference__actions">
            <button
              type="submit"
              disabled={controlsDisabled()}
              aria-describedby="session-duration-feedback"
            >
              {t("session-duration-save")}
            </button>
            <Show
              when={failed() && uncertain()}
              fallback={
                <button
                  type="button"
                  disabled={busy()}
                  aria-describedby="session-duration-feedback"
                  onClick={() => preferenceOwnerLoad(owner, false)}
                >
                  {t("session-duration-refresh")}
                </button>
              }
            >
              <button
                type="button"
                disabled={busy() || !preferenceOwnerSessionReady(owner)}
                aria-describedby="session-duration-feedback"
                onClick={() => preferenceOwnerSubmit(owner)}
              >
                {t("session-duration-retry")}
              </button>
            </Show>
          </div>
          <div id="session-duration-feedback" aria-live="polite" aria-atomic="true">
            <Show
              when={message() !== null}
              fallback={
                <Show
                  when={busy() && pending()}
                  fallback={
                    <Show when={confirmed()}>{<p>{t("session-duration-confirmed")}</p>}</Show>
                  }
                >
                  <p>{t("session-duration-saving")}</p>
                </Show>
              }
            >
              <p class="public-session-preference__error">{message() ?? ""}</p>
            </Show>
          </div>
        </fieldset>
      </form>
    </section>
  );
}

function feedback(state: PreferenceState): string | null {
  if (state.validationError) {
    return t("session-duration-validation");
  }
  const error = state.error;
  if (error === null) {
    return null;
  }
  if (error.status === 409 && error.code === "session_preference_conflict") {
    return t("session-duration-conflict");
  }
  if (state.pendingUncertain) {
    return t("session-duration-unknown");
  }
  if (commandNotSent(error)) {
    return t("command-not-sent");
  }
  if (error.code === "session_lock_unavailable") {
    return t("session-lock-unavailable");
  }
  if (error.code === "session_lock_timeout") {
    return t("session-lock-timeout");
  }
  if (error.code === "invalid_session_preference") {
    return t("session-duration-validation");
  }
  if (state.pending === null) {
    return t("session-duration-load-error");
  }
  return t("session-duration-save-error");
}
