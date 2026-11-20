// Realtime 运维页面的筛选器与字段反馈。
// 草稿留在表单内，只有通过校验的筛选才能提交到地址栏。

import { useFocusElementByIdAfterRender } from "@auction/web-shared/platform/a11y";
import type { Cell } from "@auction/web-shared/reactive/cell";
import { createCell } from "@auction/web-shared/reactive/cell";
import { structuralEquals } from "@auction/web-shared/reactive/equality";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { type Accessor, createEffect, For, Show, untrack } from "solid-js";

import {
  type FilterField,
  type FilterValue,
  filterFieldInputId,
  filterFieldNormalize,
  filterValueErrorValues,
  filterValueFromText,
  filterValueInputValue,
  filterValueText,
} from "./filter-field";

/** 事件筛选表单的稳定 ID。 */
export const EVENT_FILTER_FORM_ID = "realtime-event-filters";
/** 失败筛选表单的稳定 ID。 */
export const FAILURE_FILTER_FORM_ID = "realtime-failure-filters";

/** 申请 realtime 事件元数据查询筛选条件。 */
export function RealtimeEventFilters(props: {
  readonly applied:
    | readonly [FilterValue, FilterValue]
    | Accessor<readonly [FilterValue, FilterValue]>;
  readonly invalidApplied: boolean;
  readonly onLoad: (value: readonly [string, string]) => void;
}): JSX.Element {
  const submitted = createCell(false);
  const applied = createValueMemo((): readonly [FilterValue, FilterValue] => {
    const source = props.applied;
    // 兼容仍传访问器的调用点；两种形状都按值进入草稿的依赖。
    return typeof source === "function" ? source() : source;
  });
  const draft = useFilterDraft(applied, submitted);
  const showErrors = createValueMemo(() => props.invalidApplied || submitted());
  const focusField = useFocusElementByIdAfterRender();
  const submit = (event: SubmitEvent): void => {
    event.preventDefault();
    const current = draft();
    const fields = [
      ["event_stream", current[0]],
      ["event_kind", current[1]],
    ] as const;
    const values = validateFields(fields, submitted, focusField);
    if (values !== null) {
      props.onLoad([values[0], values[1]]);
    }
  };
  return (
    <form
      id={EVENT_FILTER_FORM_ID}
      class="admin-realtime-filter"
      autocomplete="off"
      onSubmit={submit}
    >
      <FilterInput
        field="event_stream"
        name="stream"
        label={t("realtime-filter-stream")}
        value={draft()[0]}
        showErrors={showErrors()}
        onInput={(value) => draft.update((current) => [filterValueFromText(value), current[1]])}
      />
      <FilterInput
        field="event_kind"
        name="kind"
        label={t("realtime-filter-kind")}
        value={draft()[1]}
        showErrors={showErrors()}
        onInput={(value) => draft.update((current) => [current[0], filterValueFromText(value)])}
      />
      <button type="submit">{t("realtime-filter-apply")}</button>
    </form>
  );
}

/** 申请 realtime failure 查询筛选条件。 */
export function RealtimeFailureFilters(props: {
  readonly applied:
    | readonly [FilterValue, FilterValue, FilterValue, FilterValue]
    | Accessor<readonly [FilterValue, FilterValue, FilterValue, FilterValue]>;
  readonly invalidApplied: boolean;
  readonly onLoad: (value: readonly [string, string, string]) => void;
}): JSX.Element {
  const submitted = createCell(false);
  const applied = createValueMemo(
    (): readonly [FilterValue, FilterValue, FilterValue, FilterValue] => {
      const source = props.applied;
      return typeof source === "function" ? source() : source;
    },
  );
  const draft = useFilterDraft(applied, submitted);
  const showErrors = createValueMemo(() => props.invalidApplied || submitted());
  const focusField = useFocusElementByIdAfterRender();
  const submit = (event: SubmitEvent): void => {
    event.preventDefault();
    const current = draft();
    const fields = [
      ["failure_stream", current[0]],
      ["failure_consumer", current[1]],
      ["failure_code", current[2]],
      ["failure_state", current[3]],
    ] as const;
    const values = validateFields(fields, submitted, focusField);
    if (values !== null) {
      props.onLoad([values[0], values[1], values[2]]);
    }
  };
  return (
    <details class="admin-realtime-disclosure" open={props.invalidApplied}>
      <summary>{t("realtime-failure-filters")}</summary>
      <form
        id={FAILURE_FILTER_FORM_ID}
        class="admin-realtime-filter"
        autocomplete="off"
        onSubmit={submit}
      >
        <FilterInput
          field="failure_stream"
          name="stream"
          label={t("realtime-filter-stream")}
          value={draft()[0]}
          showErrors={showErrors()}
          onInput={(value) =>
            draft.update((current) => [
              filterValueFromText(value),
              current[1],
              current[2],
              current[3],
            ])
          }
        />
        <FilterInput
          field="failure_consumer"
          name="consumer"
          label={t("realtime-filter-consumer")}
          value={draft()[1]}
          showErrors={showErrors()}
          onInput={(value) =>
            draft.update((current) => [
              current[0],
              filterValueFromText(value),
              current[2],
              current[3],
            ])
          }
        />
        <FilterInput
          field="failure_code"
          name="code"
          label={t("realtime-filter-code")}
          value={draft()[2]}
          showErrors={showErrors()}
          onInput={(value) =>
            draft.update((current) => [
              current[0],
              current[1],
              filterValueFromText(value),
              current[3],
            ])
          }
        />
        <button type="submit">{t("realtime-filter-apply")}</button>
      </form>
    </details>
  );
}

/** 状态选择沿用立即应用的交互，保留非法历史值并允许从正常选项中修正。 */
export function RealtimeFailureStateFilter(props: {
  readonly value: FilterValue;
  readonly onChange: (value: string) => void;
}): JSX.Element {
  const field: FilterField = "failure_state";
  return (
    <div class="admin-realtime-status">
      <QueryFilterSelect
        id={filterFieldInputId(field)}
        name="failure_state"
        label={t("realtime-filter-state")}
        value={props.value}
        choices={[
          ["", t("realtime-filter-all"), false],
          ["pending", t("realtime-state-pending"), true],
          ["retry", t("realtime-state-retry"), true],
          ["dead_letter", t("realtime-state-dead-letter"), false],
          ["completed", t("realtime-state-completed"), false],
        ]}
        onChange={(value) => {
          const normalized = filterFieldNormalize(field, value);
          if (normalized !== null) {
            props.onChange(normalized);
          }
        }}
      />
    </div>
  );
}

function FilterInput(props: {
  readonly field: FilterField;
  readonly name: string;
  readonly label: string;
  readonly value: FilterValue;
  readonly showErrors: boolean;
  readonly onInput: (value: string) => void;
}): JSX.Element {
  const invalidDraft = createValueMemo(() => {
    if (!props.showErrors) {
      return false;
    }
    const text = filterValueText(props.value);
    return text !== null && filterFieldNormalize(props.field, text) === null;
  });
  return (
    <QueryFilterInput
      id={filterFieldInputId(props.field)}
      name={props.name}
      label={props.label}
      onInput={props.onInput}
      inputType="search"
      value={filterValueInputValue(props.value)}
      errorValues={filterValueErrorValues(props.value, invalidDraft())}
    />
  );
}

/** 单行或本地时间筛选框，关联字段错误与转义后的地址原值。 */
export function QueryFilterInput(props: {
  readonly id: string;
  readonly name: string;
  readonly label: string;
  readonly inputType: string;
  readonly value: string;
  readonly errorValues: readonly string[];
  readonly onInput: (value: string) => void;
}): JSX.Element {
  const errorId = createValueMemo(() => `${props.id}-error`);
  const labelId = createValueMemo(() => `${props.id}-label`);
  const invalid = createValueMemo(() => props.errorValues.length > 0);
  return (
    <label for={props.id}>
      <span id={labelId()}>{props.label}</span>
      <input
        id={props.id}
        name={props.name}
        value={props.value}
        type={props.inputType}
        autocapitalize="none"
        spellcheck="false"
        enterkeyhint="search"
        aria-labelledby={labelId()}
        aria-invalid={invalid() ? "true" : "false"}
        aria-describedby={invalid() ? errorId() : undefined}
        onInput={(event) => props.onInput(event.currentTarget.value)}
      />
      <Show when={invalid()}>
        <FilterError
          id={errorId()}
          originals={props.errorValues}
          isLocalTime={props.inputType === "datetime-local"}
        />
      </Show>
    </label>
  );
}

/** 闭集筛选显示无效地址，并等待用户从允许的选项中选择。 */
export function QueryFilterSelect(props: {
  readonly id: string;
  readonly name: string;
  readonly label: string;
  readonly value: FilterValue;
  readonly choices: readonly QueryFilterChoice[];
  readonly onChange: (value: string) => void;
}): JSX.Element {
  const selected = createValueMemo(() => filterValueText(props.value));
  const errorValues = createValueMemo(() => filterValueErrorValues(props.value, false));
  const invalid = createValueMemo(() => errorValues().length > 0);
  const errorId = createValueMemo(() => `${props.id}-error`);
  const labelId = createValueMemo(() => `${props.id}-label`);
  return (
    <label for={props.id}>
      <span id={labelId()}>{props.label}</span>
      <select
        id={props.id}
        name={props.name}
        aria-labelledby={labelId()}
        aria-invalid={invalid() ? "true" : "false"}
        aria-describedby={invalid() ? errorId() : undefined}
        onChange={(event) => props.onChange(event.currentTarget.value)}
      >
        <Show when={invalid()}>
          <option
            value={INVALID_OPTION}
            disabled
            ref={(element) => {
              element.selected = true;
            }}
          >
            {t("value-empty")}
          </option>
        </Show>
        <For each={props.choices} keyed={(choice) => choice[0]}>
          {(choice) => (
            <option
              value={choice()[0]}
              selected={selected() === choice()[0]}
              disabled={choice()[2]}
            >
              {choice()[1]}
            </option>
          )}
        </For>
      </select>
      <Show when={invalid()}>
        <FilterError id={errorId()} originals={errorValues()} isLocalTime={false} />
      </Show>
    </label>
  );
}

/** 闭集筛选的选项：稳定取值、显示文案与是否禁用。 */
export type QueryFilterChoice = readonly [choice: string, label: string, disabled: boolean];

const INVALID_OPTION = "invalid";

function FilterError(props: {
  readonly id: string;
  readonly originals: readonly string[];
  readonly isLocalTime: boolean;
}): JSX.Element {
  const message = createValueMemo(() =>
    props.isLocalTime && props.originals.length === 1
      ? t("admin-audit-invalid-time")
      : t("admin-error-invalid-request"),
  );
  const originals = createValueMemo(() =>
    props.originals.map((value, index) => ({
      index,
      escaped: rustDebugString(value),
      controls: controlCodes(value),
    })),
  );
  return (
    <span id={props.id} class="action-feedback action-feedback--error" role="alert">
      {message()}
      <For each={originals()} keyed={(original) => original.index}>
        {(original) => (
          <span>
            {" "}
            <code>
              <For each={escapedParts(original().escaped)} keyed={false}>
                {(part, partIndex) => (
                  <>
                    <Show when={partIndex > 0}>
                      <wbr />
                    </Show>
                    {part()}
                  </>
                )}
              </For>
            </code>
            <Show when={original().controls !== ""}>
              <strong>{` (${original().controls})`}</strong>
            </Show>
          </span>
        )}
      </For>
    </span>
  );
}

function escapedParts(escaped: string): string[] {
  const parts: string[] = [];
  let run = 0;
  let start = 0;
  let index = 0;
  // 只补连续 ASCII 的折行机会，连接各段仍得到完整的转义值。
  for (const character of escaped) {
    index += character.length;
    const code = character.codePointAt(0) ?? 0;
    if (code < 0x21 || code > 0x7e) {
      run = 0;
      continue;
    }
    run += 1;
    if (run < 16) {
      continue;
    }
    run = 0;
    parts.push(escaped.slice(start, index));
    start = index;
  }
  if (start < escaped.length) {
    parts.push(escaped.slice(start));
  }
  return parts;
}

function validateFields(
  fields: readonly [readonly [FilterField, FilterValue], readonly [FilterField, FilterValue]],
  submitted: Cell<boolean>,
  focusField: (elementId: string) => void,
): readonly [string, string] | null;
function validateFields(
  fields: readonly [
    readonly [FilterField, FilterValue],
    readonly [FilterField, FilterValue],
    readonly [FilterField, FilterValue],
    readonly [FilterField, FilterValue],
  ],
  submitted: Cell<boolean>,
  focusField: (elementId: string) => void,
): readonly [string, string, string, string] | null;
function validateFields(
  fields: readonly (readonly [FilterField, FilterValue])[],
  submitted: Cell<boolean>,
  focusField: (elementId: string) => void,
): readonly string[] | null {
  const values: string[] = [];
  for (const [field, value] of fields) {
    const text = filterValueText(value);
    const normalized = text === null ? null : filterFieldNormalize(field, text);
    if (normalized === null) {
      submitted.set(true);
      focusField(filterFieldInputId(field));
      return null;
    }
    values.push(normalized);
  }
  submitted.set(false);
  return values;
}

function useFilterDraft<T>(applied: Accessor<T>, submitted: Cell<boolean>): Cell<T> {
  const draft = createCell<T>(untrack(applied));
  // 只有已应用条件改变才替换输入；同条件翻页或轮询不改变这个 memo。
  createEffect(applied, (next) => {
    if (!structuralEquals(draft.peek(), next)) {
      draft.set(next);
    }
    submitted.set(false);
  });
  return draft;
}

const RUST_DEBUG_ESCAPED_CATEGORY = /[\p{Cc}\p{Cf}\p{Cs}\p{Co}\p{Cn}\p{Zl}\p{Zp}]/u;
const RUST_DEBUG_SPACE_SEPARATOR = /\p{Zs}/u;
const RUST_DEBUG_GRAPHEME_EXTENDED = /\p{Grapheme_Extend}/u;
const CONTROL_CHARACTER = /^\p{Cc}$/u;

type CodePointRange = number | readonly [number, number];

// Rust 1.100.0 的 `Debug` 判断与运行时 Unicode 属性的差异；按该工具链逐码点比对生成。
// 前面的表覆盖最新 Unicode 已赋予码位、旧运行时仍判为不可打印的范围。
const RUST_DEBUG_PRINTABLE_OVERRIDES: readonly CodePointRange[] = [
  0x558,
  [0x58b, 0x58c],
  0x208f,
  [0x209d, 0x209f],
  [0x20c2, 0x20c4],
  [0x2e60, 0x2e63],
  0xa7dd,
  0xa7e2,
  [0xab6c, 0xab6d],
  [0xff9e, 0xff9f],
  [0x107bb, 0x107bf],
  [0x10ec9, 0x10eca],
  [0x10ed9, 0x10eee],
  0x11b0a,
  0x11df1,
  0x1246f,
  [0x12475, 0x1247f],
  [0x12550, 0x12686],
  [0x18cd6, 0x18cda],
  [0x18d1f, 0x18d20],
  [0x18e00, 0x19191],
  [0x191a0, 0x191d2],
  [0x1b123, 0x1b128],
  0x1b168,
  [0x1ced2, 0x1ced4],
  [0x1cedd, 0x1cedf],
  [0x1cef1, 0x1cefd],
  [0x1d1eb, 0x1d1ff],
  [0x1d253, 0x1d25a],
  [0x1d25d, 0x1d25e],
  [0x1d260, 0x1d27f],
  0x1d6a6,
  [0x1db00, 0x1db1c],
  [0x1df1f, 0x1df24],
  [0x1df2b, 0x1df81],
  [0x1df90, 0x1df96],
  [0x1dfcd, 0x1dfff],
  0x1f1ae,
  0x1f6d9,
  [0x1f7da, 0x1f7db],
  [0x1f7f1, 0x1f7ff],
  [0x1fa8b, 0x1fa8d],
  0x1facc,
  0x1fadd,
  0x1faeb,
  [0x1faf9, 0x1fafa],
  0x2b81e,
  [0x3d000, 0x3fc3f],
];

// 该工具链仍转义、而运行时 Unicode 属性判为可打印的码位。
const RUST_DEBUG_ESCAPED_OVERRIDES: readonly CodePointRange[] = [[0x115f, 0x1160], 0x3164, 0xffa0];

/** Rust `Debug` 对字符串的转义：引号包裹，特殊字符与不可打印字符按 `escape_debug` 输出。 */
function rustDebugString(value: string): string {
  let escaped = '"';
  for (const character of value) {
    escaped += escapeDebugCharacter(character);
  }
  return `${escaped}"`;
}

function escapeDebugCharacter(character: string): string {
  switch (character) {
    case "\0":
      return "\\0";
    case "\t":
      return "\\t";
    case "\r":
      return "\\r";
    case "\n":
      return "\\n";
    case "\\":
      return "\\\\";
    case '"':
      return '\\"';
    default:
      break;
  }
  if (!isRustDebugEscaped(character)) {
    return character;
  }
  return `\\u{${(character.codePointAt(0) ?? 0).toString(16)}}`;
}

function isRustDebugEscaped(character: string): boolean {
  if (character === " ") {
    return false;
  }
  const code = character.codePointAt(0) ?? 0;
  if (inCodePointRanges(code, RUST_DEBUG_ESCAPED_OVERRIDES)) {
    return true;
  }
  if (inCodePointRanges(code, RUST_DEBUG_PRINTABLE_OVERRIDES)) {
    return false;
  }
  return (
    RUST_DEBUG_ESCAPED_CATEGORY.test(character) ||
    RUST_DEBUG_SPACE_SEPARATOR.test(character) ||
    RUST_DEBUG_GRAPHEME_EXTENDED.test(character)
  );
}

function inCodePointRanges(code: number, ranges: readonly CodePointRange[]): boolean {
  for (const range of ranges) {
    if (typeof range === "number") {
      if (code === range) {
        return true;
      }
      continue;
    }
    if (code >= range[0] && code <= range[1]) {
      return true;
    }
  }
  return false;
}

function controlCodes(value: string): string {
  const codes: string[] = [];
  for (const character of value) {
    if (!CONTROL_CHARACTER.test(character)) {
      continue;
    }
    const code = character.codePointAt(0) ?? 0;
    codes.push(`U+${code.toString(16).toUpperCase().padStart(4, "0")}`);
  }
  return codes.join(", ");
}
