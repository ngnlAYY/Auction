// 配置数值控件的回填、离焦反馈与纠错时机。

import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { createContext, createEffect, Show, useContext } from "solid-js";

import type { FieldAnnouncer } from "../field-announcement";
import { isHot } from "../hot-fields";
import {
  FormIssuesContext,
  fieldId,
  type NumberIssue,
  numberIssue,
  numberIssueMessage,
} from "../validation";

/** 由 RuntimeConfigFields 向字段控件提供的播报句柄，对应 Rust 版的 `FieldAnnouncer` 上下文。 */
export const FieldAnnouncerContext = createContext<FieldAnnouncer>();

interface NumberEdit {
  readonly issue: NumberIssue | null;
  /** 错误已在界面上展示；输入只延续或清除它，新错误等离焦或提交。 */
  readonly reported: boolean;
  /** 当前错误已播报或已排入离焦播报；种类变化后需要重新播报。 */
  readonly announced: boolean;
}

/**
 * 数字输入控件：原始值可以就地标错，编辑后的错误只在离焦或提交时展示。
 */
export function NumberInput(props: {
  readonly path: string;
  readonly label: string;
  readonly minimum: string;
  readonly maximum: string;
  readonly original: string | null;
  readonly value: string;
  readonly disabled: boolean;
  readonly onInput: (value: string) => void;
}): JSX.Element {
  const issues = useContext(FormIssuesContext);
  const announcer = useContext(FieldAnnouncerContext);
  const edit = createCell<NumberEdit | null>(null);
  const issue = createValueMemo(() => numberIssue(props.value, props.minimum, props.maximum));
  const historicError = createValueMemo(() => issue() !== null && props.original === props.value);
  const inSummary = createValueMemo(() => issues().some((issue) => issue.path === props.path));
  const message = createValueMemo(() => {
    const currentIssue = issue();
    if (currentIssue === null) {
      return null;
    }
    const pending = edit();
    const show =
      (pending === null ? historicError() : pending.reported || historicError()) || inSummary();
    return show ? numberIssueMessage(currentIssue, props.minimum, props.maximum) : null;
  });
  const id = createValueMemo(() => fieldId(props.path));
  const errorId = createValueMemo(() => `${id()}-error`);
  const hot = createValueMemo(() => isHot(props.path));
  let inputElement: HTMLInputElement | null = null;

  // 像 Rust 版一样只在 DOM 值与草稿不同时写回，保留数字输入尚未完成的编辑。
  createEffect(
    () => props.value,
    (value) => {
      if (inputElement !== null && inputElement.value !== value) {
        inputElement.value = value;
      }
    },
    { defer: true },
  );

  return (
    <label class="admin-runtime-field">
      <span>
        {props.label}
        <Show when={hot()}>
          <span class="admin-runtime-hot-badge" title={t("admin-runtime-hot-field-title")}>
            {t("admin-runtime-hot-field-badge")}
          </span>
        </Show>
      </span>
      <input
        id={id()}
        name={props.path}
        autocomplete="off"
        inputmode="numeric"
        data-runtime-number={props.path}
        data-runtime-label={props.label}
        aria-invalid={message() !== null ? "true" : "false"}
        aria-describedby={message() !== null ? errorId() : undefined}
        type="number"
        disabled={props.disabled}
        min={props.minimum}
        max={props.maximum}
        step="1"
        ref={(element) => {
          inputElement = element;
          if (element.value !== props.value) {
            element.value = props.value;
          }
        }}
        onInput={(event) => {
          const value = event.currentTarget.value;
          const nextIssue = numberIssue(value, props.minimum, props.maximum);
          // 输入只延续或清除已展示的错误；新错误在离焦或提交后展示。
          const pending = edit.peek();
          // 等待离焦播报期间错误种类变化：只撤销旧播报，已展示的错误延续到新错误。
          const changed = pending?.announced === true && pending.issue !== nextIssue;
          if (changed) {
            announcer.clearField(id());
          }
          const reported =
            (pending === null ? historicError() : pending.reported) ||
            issues.peek().some((issue) => issue.path === props.path);
          edit.set({
            issue: nextIssue,
            reported: reported && nextIssue !== null,
            announced: nextIssue !== null && !changed && pending?.announced === true,
          });
          if (reported && nextIssue === null) {
            announcer.clearField(id());
          }
          props.onInput(value);
        }}
        onBlur={() => {
          const pendingEdit = edit.peek();
          if (
            pendingEdit !== null &&
            pendingEdit.issue !== null &&
            !pendingEdit.announced &&
            !issues.peek().some((issue) => issue.path === props.path)
          ) {
            const pendingIssue = pendingEdit.issue;
            edit.set({ issue: pendingIssue, reported: true, announced: true });
            // 焦点播报的等待期内语言可能切换；保留字段错误，在真正播报前翻译。
            announcer.announceLocalizedOnBlur(id(), () =>
              numberIssueMessage(pendingIssue, props.minimum, props.maximum),
            );
          }
        }}
      />
      <Show when={message()}>
        {(text) => (
          <small id={errorId()} class="admin-config-field-error">
            {text()}
          </small>
        )}
      </Show>
    </label>
  );
}
