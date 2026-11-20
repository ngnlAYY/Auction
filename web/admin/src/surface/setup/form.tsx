// 首次部署配置步骤：分组输入、统一提交校验和短生命周期异步状态。

import type { Cell } from "@auction/web-shared/reactive/cell";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskScope } from "@auction/web-shared/reactive/tasks";
import { unreachable } from "@auction/web-shared/result";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Show } from "solid-js";

import { submitFirstRun } from "../../api/setup";
import { useFocusField } from "../validation";
import {
  type DatabaseTarget,
  databaseTargetMatches,
  databaseTargetsEqual,
  SETUP_FIELD_ADMIN,
  SETUP_FIELD_ADVANCED,
  SETUP_FIELD_BASIC,
  SETUP_FIELD_DATABASE,
  SETUP_FIELD_LISTENER,
  SETUP_FIELD_SCHEDULE,
  type SetupDraft,
  type SetupField,
  setupDraftClearPasswords,
  setupDraftResetAdvanced,
  setupFieldId,
} from "./draft";
import { SetupErrors, SetupInput } from "./fields";
import { errorKey } from "./index";
import { databaseTarget, type SetupIssue, validate } from "./validation";

const CONFIRM_INITIALIZE_ID = "setup-confirm-initialize";

interface SetupFormGroup {
  readonly legend: string;
  readonly fields: readonly SetupField[];
}

const SETUP_FORM_GROUPS: readonly SetupFormGroup[] = [
  { legend: "setup-database", fields: SETUP_FIELD_DATABASE },
  { legend: "setup-admin", fields: SETUP_FIELD_ADMIN },
  { legend: "setup-basic", fields: SETUP_FIELD_BASIC },
  { legend: "setup-listener", fields: SETUP_FIELD_LISTENER },
];

/** 页面级反馈：成功与否和提示文案的译文键。 */
export type SetupNotice = readonly [success: boolean, key: string];

/** 首次设置配置表单；校验失败时展开含进阶字段的区域，并聚焦首个错误控件。 */
export function ConfigurationForm(props: {
  readonly token: Cell<string>;
  readonly draft: Cell<SetupDraft>;
  readonly notice: Cell<SetupNotice | null>;
  readonly outcomeUnknown: Cell<boolean>;
  readonly initializeAndStart: boolean;
  readonly onAuthFailure: () => void;
  readonly onCompleted: () => void;
}): JSX.Element {
  const pending = createCell<boolean | null>(null);
  const issues = createCell<readonly SetupIssue[]>([]);
  const advancedOpen = createCell(false);
  const tasks = createTaskScope();
  const focusField = useFocusField();

  const submit = (complete: boolean): void => {
    // 回调内也检查门禁，防止键盘提交或尚未重渲染的按钮绕过 disabled。
    if (pending.peek() !== null || props.outcomeUnknown.peek()) {
      return;
    }
    props.notice.set(null);
    const validated = validate(props.draft.peek());
    if (!validated.ok) {
      const errors = validated.error;
      if (errors.some((issue: SetupIssue) => SETUP_FIELD_ADVANCED.includes(issue.field))) {
        advancedOpen.set(true);
      }
      const first = errors[0];
      if (first !== undefined) {
        focusField(setupFieldId(first.field));
      }
      issues.set(errors);
      return;
    }
    const request = validated.value;
    issues.set([]);
    let confirmedTarget: DatabaseTarget | null = null;
    if (complete && props.initializeAndStart) {
      // 使用勾选时的快照核对本次请求，不能从提交草稿重建授权。
      const confirmed = props.draft.peek().confirmedDatabaseTarget;
      if (confirmed === null || !databaseTargetMatches(confirmed, request.database)) {
        props.notice.set([false, "setup-error-confirm-initialize"]);
        focusField(CONFIRM_INITIALIZE_ID);
        return;
      }
      request.confirm_initialize = true;
      confirmedTarget = confirmed;
    }
    pending.set(complete);
    const submittedToken = props.token();
    const initializeAndStart = props.initializeAndStart;
    tasks.spawn(async (context) => {
      const result = await submitFirstRun(
        submittedToken,
        request,
        complete,
        confirmedTarget,
        context.signal,
      );
      if (!context.alive()) {
        return;
      }
      pending.set(null);
      if (result.ok) {
        if (complete) {
          props.draft.update(setupDraftClearPasswords);
          props.notice.set([true, initializeAndStart ? "setup-starting" : "setup-complete"]);
          props.onCompleted();
        } else {
          props.notice.set([true, "setup-valid"]);
        }
        return;
      }
      const code = result.error;
      props.notice.set([false, errorKey(code)]);
      const applied = applySubmissionFailure(props.draft.peek(), code);
      if (applied.draft !== props.draft.peek()) {
        props.draft.set(applied.draft);
      }
      switch (applied.failure) {
        case "outcome_unknown":
          props.outcomeUnknown.set(true);
          props.token.set("");
          break;
        case "authenticate":
          props.onAuthFailure();
          break;
        case "reconfirm_target":
          focusField(CONFIRM_INITIALIZE_ID);
          break;
        case "other":
          break;
        default:
          return unreachable(applied.failure);
      }
    });
  };

  const waiting = createValueMemo(() => pending() !== null);
  // 终态禁用字段和动作，但不能向辅助技术声称仍有请求正在等待。
  const busy = createValueMemo(() => waiting() || props.outcomeUnknown());
  const displayedTarget = createValueMemo(() =>
    props.initializeAndStart ? databaseTarget(props.draft()) : null,
  );
  const confirmationChecked = createValueMemo(() => {
    const target = displayedTarget();
    if (target === null) {
      return false;
    }
    return databaseTargetsEqual(props.draft().confirmedDatabaseTarget, target);
  });
  const confirmationLabel = createValueMemo(() => {
    const target = displayedTarget();
    return target === null
      ? null
      : t("setup-confirm-initialize", {
          host: target.host,
          port: String(target.port),
          name: target.name,
        });
  });

  return (
    <form
      class="admin-setup__form"
      // 两个动作统一走本地校验，避免原生校验跳过按钮或阻止隐藏高级字段的错误定位。
      novalidate={true}
      aria-busy={waiting() ? "true" : "false"}
      onInput={() => {
        if (!busy() && !props.outcomeUnknown()) {
          props.notice.set(null);
        }
      }}
      onSubmit={(event) => {
        event.preventDefault();
        submit(true);
      }}
    >
      <p class="admin-setup__hint">{t("setup-form-help")}</p>
      <SetupErrors issues={issues} advancedOpen={advancedOpen} />
      <For each={SETUP_FORM_GROUPS} keyed={(group) => group.legend}>
        {(group) => (
          <fieldset disabled={busy()}>
            <legend>{t(group().legend)}</legend>
            <Show when={group().legend === "setup-database"}>
              <p class="admin-setup__hint">{t("setup-database-help")}</p>
            </Show>
            <Show when={group().legend === "setup-listener"}>
              <p class="admin-setup__hint">{t("setup-listener-help")}</p>
            </Show>
            <div class="admin-setup__grid">
              <For each={group().fields} keyed={(field) => setupFieldId(field)}>
                {(field) => (
                  <SetupInput field={field()} draft={props.draft} issues={issues} busy={busy()} />
                )}
              </For>
            </div>
            <Show when={group().legend === "setup-basic"}>
              <div class="admin-setup__options">
                <label class="admin-setup__check">
                  <input
                    type="checkbox"
                    checked={props.draft().metricsEnabled}
                    aria-describedby="setup-metrics-help"
                    onChange={(event) => {
                      if (busy()) {
                        return;
                      }
                      props.draft.update((current) => ({
                        ...current,
                        metricsEnabled: event.currentTarget.checked,
                      }));
                      props.notice.set(null);
                    }}
                  />
                  {t("setup-metrics")}
                </label>
                <p id="setup-metrics-help" class="admin-setup__hint">
                  {t("setup-metrics-help")}
                </p>
              </div>
            </Show>
          </fieldset>
        )}
      </For>
      <fieldset disabled={busy()}>
        <legend>{t("setup-schedule-settings")}</legend>
        <label class="admin-setup__check">
          <input
            type="checkbox"
            checked={props.draft().scheduleEnabled}
            aria-describedby="setup-schedule-help"
            onChange={(event) => {
              if (pending.peek() !== null || props.outcomeUnknown.peek()) {
                return;
              }
              props.draft.update((current) => ({
                ...current,
                scheduleEnabled: event.currentTarget.checked,
              }));
              issues.update((currentIssues) =>
                currentIssues.filter((issue) => !SETUP_FIELD_SCHEDULE.includes(issue.field)),
              );
              props.notice.set(null);
            }}
          />
          {t("setup-schedule")}
        </label>
        <p id="setup-schedule-help" class="admin-setup__hint">
          {t("setup-schedule-help")}
        </p>
        <SetupInput field="schedule_timezone" draft={props.draft} issues={issues} busy={busy()} />
        <div class="admin-setup__grid">
          <For each={SETUP_FIELD_SCHEDULE.slice(1)} keyed={(field) => setupFieldId(field)}>
            {(field) => (
              <SetupInput field={field()} draft={props.draft} issues={issues} busy={busy()} />
            )}
          </For>
        </div>
      </fieldset>
      <div class="admin-setup__advanced">
        <button
          class="admin-setup__toggle"
          type="button"
          disabled={busy()}
          aria-expanded={advancedOpen() ? "true" : "false"}
          aria-controls="setup-advanced-fields"
          onClick={() => advancedOpen.update((open) => !open)}
        >
          {advancedOpen() ? t("setup-advanced-hide") : t("setup-advanced-show")}
        </button>
        <p class="admin-setup__hint">{t("setup-advanced-help")}</p>
        <div id="setup-advanced-fields" hidden={!advancedOpen()}>
          <fieldset disabled={busy()}>
            <legend>{t("setup-advanced")}</legend>
            <button
              class="admin-setup__reset"
              type="button"
              disabled={busy()}
              aria-describedby="setup-advanced-reset-help"
              onClick={() => {
                if (pending.peek() !== null || props.outcomeUnknown.peek()) {
                  return;
                }
                props.draft.update(setupDraftResetAdvanced);
                issues.update((currentIssues) =>
                  currentIssues.filter((issue) => !SETUP_FIELD_ADVANCED.includes(issue.field)),
                );
                props.notice.set(null);
              }}
            >
              {t("setup-advanced-reset")}
            </button>
            <p id="setup-advanced-reset-help" class="admin-setup__hint">
              {t("setup-advanced-reset-help")}
            </p>
            <div class="admin-setup__grid">
              <For each={SETUP_FIELD_ADVANCED} keyed={(field) => setupFieldId(field)}>
                {(field) => (
                  <SetupInput field={field()} draft={props.draft} issues={issues} busy={busy()} />
                )}
              </For>
            </div>
          </fieldset>
        </div>
      </div>
      <p id="setup-save-help" class="admin-setup__hint">
        {props.initializeAndStart ? t("setup-initialize-help") : t("setup-save-help")}
      </p>
      <Show when={props.initializeAndStart}>
        <div class="admin-setup__check">
          <input
            id={CONFIRM_INITIALIZE_ID}
            name="confirm_initialize"
            type="checkbox"
            checked={confirmationChecked()}
            disabled={busy() || displayedTarget() === null}
            aria-describedby="setup-save-help"
            onChange={(event) => {
              if (pending.peek() !== null || props.outcomeUnknown.peek()) {
                return;
              }
              props.draft.update((current) =>
                updateConfirmation(current, displayedTarget(), event.currentTarget.checked),
              );
              props.notice.set(null);
            }}
          />
          <label
            for={CONFIRM_INITIALIZE_ID}
            style={{ "white-space": "pre-wrap", "overflow-wrap": "anywhere" }}
          >
            {confirmationLabel() ?? t("setup-confirm-target-unavailable")}
          </label>
        </div>
      </Show>
      <div class="admin-setup__actions">
        <button type="button" disabled={busy()} onClick={() => submit(false)}>
          {pending() === false ? t("setup-validating") : t("setup-validate")}
        </button>
        <button class="admin-btn-primary" type="submit" disabled={busy()}>
          {pending() === true
            ? t("setup-saving")
            : props.initializeAndStart
              ? t("setup-save-start")
              : t("setup-save")}
        </button>
      </div>
    </form>
  );
}

type SubmissionFailure = "outcome_unknown" | "authenticate" | "reconfirm_target" | "other";

function applySubmissionFailure(
  draft: SetupDraft,
  code: string,
): { readonly draft: SetupDraft; readonly failure: SubmissionFailure } {
  switch (code) {
    case "setup_outcome_unknown":
      return { draft: setupDraftClearPasswords(draft), failure: "outcome_unknown" };
    case "setup_auth":
      return { draft, failure: "authenticate" };
    case "setup_confirmation_required":
      // 服务端在写入前已拒绝，清除旧授权并允许重新确认当前目标。
      return {
        draft: { ...draft, confirmedDatabaseTarget: null },
        failure: "reconfirm_target",
      };
    default:
      return { draft, failure: "other" };
  }
}

function updateConfirmation(
  draft: SetupDraft,
  displayedTarget: DatabaseTarget | null,
  checked: boolean,
): SetupDraft {
  // 勾选必须对应画面上的目标。输入已改变但页面还未更新时拒绝沿用旧目标。
  const current = databaseTarget(draft);
  const confirmed =
    displayedTarget !== null && checked && databaseTargetsEqual(current, displayedTarget)
      ? displayedTarget
      : null;
  return { ...draft, confirmedDatabaseTarget: confirmed };
}
