// 首次部署的字段控件和可定位错误摘要；标签、帮助与错误全部使用当前语言。

import type { Cell } from "@auction/web-shared/reactive/cell";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { t, useLocaleSession } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { createEffect, For, Show } from "solid-js";

import { FieldError, useFocusField } from "../validation";
import {
  SETUP_FIELD_ADVANCED,
  SETUP_FIELD_WEEKDAYS,
  type SetupDraft,
  type SetupField,
  setupDraftGenerateAdminSecret,
  setupDraftSet,
  setupDraftValue,
  setupFieldAutocomplete,
  setupFieldId,
  setupFieldInputType,
  setupFieldIsScheduleTime,
  setupFieldIsWeekday,
  setupFieldMaximumBytes,
  setupFieldNumberBounds,
  setupFieldRelated,
} from "./draft";
import { issue, type SetupIssue } from "./validation";

/** 单个 Setup 字段控件；仅在 DOM 值变化时写回，避免打断数字输入尚未提交的编辑。 */
export function SetupInput(props: {
  readonly field: SetupField;
  readonly draft: Cell<SetupDraft>;
  readonly issues: Cell<readonly SetupIssue[]>;
  readonly busy: boolean;
}): JSX.Element {
  const locale = useLocaleSession();
  // `current()` 返回生效语言码，但也会收到在途目标与加载错误的变化通知。
  const effectiveLocale = createValueMemo(() => locale.current());
  const id = createValueMemo(() => setupFieldId(props.field));
  const helpId = createValueMemo(() => `${id()}-help`);
  const errorId = createValueMemo(() => `${id()}-error`);
  const revealSecret = createCell(false);
  const actorEdited = createCell(false);
  const actorBlurIssue = createCell<string | null>(null);
  let inputElement: HTMLInputElement | null = null;
  let selectElement: HTMLSelectElement | null = null;
  const submittedIssue = createValueMemo(
    () => props.issues().find((issue) => issue.field === props.field)?.key ?? null,
  );
  const error = createValueMemo(() => {
    const key = submittedIssue() ?? actorBlurIssue();
    return key === null ? null : t(key);
  });
  const invalid = createValueMemo(() => error() !== null);
  const description = createValueMemo(() => (invalid() ? `${helpId()} ${errorId()}` : helpId()));
  const bounds = createValueMemo(() => setupFieldNumberBounds(props.field));
  const minimum = createValueMemo(() => {
    const value = bounds();
    return value === null ? null : String(value[0]);
  });
  const maximum = createValueMemo(() => {
    const value = bounds();
    return value === null ? null : String(value[1]);
  });
  const step = createValueMemo(() => {
    if (setupFieldIsScheduleTime(props.field)) {
      return "60";
    }
    const value = bounds();
    return value === null ? null : String(value[2]);
  });
  const maxlength = createValueMemo(() => {
    const value = setupFieldMaximumBytes(props.field);
    return value === null ? null : String(value);
  });
  const help = () => (
    <p id={helpId()} class="admin-setup__hint">
      {t(`${id()}-help`)}
    </p>
  );
  const syncValue = (element: HTMLInputElement | HTMLSelectElement | null, next: string) => {
    if (element !== null && element.value !== next) {
      element.value = next;
    }
  };
  const syncDraftValue = () => {
    const next = setupDraftValue(props.draft.peek(), props.field);
    syncValue(inputElement, next);
    syncValue(selectElement, next);
  };
  createEffect(
    () => {
      // Dioxus 把控件值当作易变属性：任何触发重渲染的状态都要重新比较 DOM 与草稿。
      // 派生 memo 会按值去重，不能只依赖它，否则 DOM 被浏览器改写后不会纠正。
      props.draft();
      props.issues();
      revealSecret();
      actorBlurIssue();
      // 译文生效后重渲染标签与帮助文本，按 Rust 行为重新比较 DOM 与草稿。
      effectiveLocale();
      return props.busy;
    },
    () => {
      syncDraftValue();
    },
  );
  const attachInput = (element: HTMLInputElement) => {
    inputElement = element;
    syncValue(element, setupDraftValue(props.draft.peek(), props.field));
  };
  const attachSelect = (element: HTMLSelectElement) => {
    selectElement = element;
    syncValue(element, setupDraftValue(props.draft.peek(), props.field));
  };

  return (
    <div class="admin-setup__field">
      <label for={id()}>{t(id())}</label>
      <Show
        when={setupFieldIsWeekday(props.field)}
        fallback={
          <input
            id={id()}
            name={id()}
            type={
              props.field === "admin_secret" && revealSecret()
                ? "text"
                : setupFieldInputType(props.field)
            }
            inputmode={
              bounds() !== null
                ? "numeric"
                : setupFieldInputType(props.field) === "url"
                  ? "url"
                  : "text"
            }
            autocomplete={setupFieldAutocomplete(props.field)}
            autocapitalize="none"
            autocorrect="off"
            spellcheck="false"
            required={true}
            disabled={props.busy}
            min={minimum() ?? undefined}
            max={maximum() ?? undefined}
            step={step() ?? undefined}
            maxlength={maxlength() ?? undefined}
            aria-invalid={invalid() ? "true" : "false"}
            aria-describedby={description()}
            ref={attachInput}
            onInput={(event) => {
              if (props.busy) {
                return;
              }
              const value = event.currentTarget.value;
              const field = props.field;
              const current = props.draft.peek();
              if (field === "admin_actor" && setupDraftValue(current, field) !== value) {
                actorEdited.set(true);
              }
              const next = setupDraftSet(current, field, value);
              props.draft.set(next);
              // 输入被浏览器规范化成旧值时，也要把控件显示纠正回草稿。
              syncValue(event.currentTarget, setupDraftValue(next, field));
              if (field === "admin_actor") {
                if (issue(next, field) !== null) {
                  return;
                }
                actorBlurIssue.set(null);
              }
              props.issues.update((currentIssues) =>
                currentIssues.filter((issue) => !setupFieldRelated(field, issue.field)),
              );
            }}
            onBlur={() => {
              if (!props.busy && props.field === "admin_actor" && actorEdited()) {
                actorBlurIssue.set(issue(props.draft.peek(), props.field));
              }
            }}
          />
        }
      >
        <select
          id={id()}
          name={id()}
          autocomplete="off"
          required={true}
          disabled={props.busy}
          aria-invalid={invalid() ? "true" : "false"}
          aria-describedby={description()}
          ref={attachSelect}
          onInput={(event) => {
            if (props.busy) {
              return;
            }
            const next = setupDraftSet(props.draft.peek(), props.field, event.currentTarget.value);
            props.draft.set(next);
            syncValue(event.currentTarget, setupDraftValue(next, props.field));
            props.issues.update((currentIssues) =>
              currentIssues.filter((issue) => !setupFieldRelated(props.field, issue.field)),
            );
          }}
        >
          <For each={SETUP_FIELD_WEEKDAYS} keyed={false}>
            {(weekday) => <option value={weekday()[0]}>{t(weekday()[1])}</option>}
          </For>
        </select>
      </Show>
      <Show when={props.field === "admin_secret"}>
        <div class="admin-setup__actions">
          <button
            type="button"
            disabled={props.busy}
            onClick={() => {
              if (props.busy) {
                return;
              }
              props.draft.update(setupDraftGenerateAdminSecret);
              props.issues.update((currentIssues) =>
                currentIssues.filter((issue) => !setupFieldRelated(props.field, issue.field)),
              );
              revealSecret.set(true);
            }}
          >
            {t("setup-admin-secret-generate")}
          </button>
          <button
            type="button"
            disabled={props.busy}
            aria-controls={id()}
            aria-pressed={revealSecret() ? "true" : "false"}
            onClick={() => {
              if (!props.busy) {
                revealSecret.update((revealed) => !revealed);
              }
            }}
          >
            {revealSecret() ? t("setup-admin-secret-hide") : t("setup-admin-secret-show")}
          </button>
        </div>
      </Show>
      <Show
        when={props.field === "admin_actor"}
        fallback={
          <>
            {help()}
            <FieldError id={errorId()} message={error()} />
          </>
        }
      >
        <div>
          {help()}
          <div aria-live={submittedIssue() !== null ? "off" : "polite"}>
            <Show when={error() !== null}>
              <p id={errorId()} class="admin-field-error">
                {error()}
              </p>
            </Show>
          </div>
        </div>
      </Show>
    </div>
  );
}

/** 可定位的错误摘要；点击进阶字段条目会展开进阶区域，所有条目都会聚焦对应控件。 */
export function SetupErrors(props: {
  readonly issues: Cell<readonly SetupIssue[]>;
  readonly advancedOpen: Cell<boolean>;
}): JSX.Element {
  const focusField = useFocusField();
  return (
    <Show when={props.issues().length > 0}>
      <section class="admin-setup__errors" role="alert" aria-labelledby="setup-errors-title">
        <h2 id="setup-errors-title">{t("setup-errors-title")}</h2>
        <ul>
          <For each={props.issues()} keyed={(issue) => setupFieldId(issue.field)}>
            {(issue) => (
              <li>
                <a
                  href={`#${setupFieldId(issue().field)}`}
                  onClick={(event) => {
                    event.preventDefault();
                    const field = issue().field;
                    if (SETUP_FIELD_ADVANCED.includes(field)) {
                      props.advancedOpen.set(true);
                    }
                    focusField(setupFieldId(field));
                  }}
                >
                  {t(setupFieldId(issue().field))}
                </a>
              </li>
            )}
          </For>
        </ul>
      </section>
    </Show>
  );
}
