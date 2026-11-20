// 配置表单的本地输入反馈；字段约束来自已渲染控件，服务端仍负责完整候选校验。

import { focusElementById } from "@auction/web-shared/platform/a11y";
import { type Cell, createCell } from "@auction/web-shared/reactive/cell";
import { structuralEquals } from "@auction/web-shared/reactive/equality";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { unreachable } from "@auction/web-shared/result";
import { t, useLocaleSession } from "@auction/web-shared/ui/i18n";
import type { LocaleCode } from "@auction/web-shared/ui/i18n/catalog";
import type { JSX } from "@solidjs/web";
import { createContext, createEffect, createMemo, For, onSettled } from "solid-js";

import { trimRustWhitespace } from "../../api/session-time";
import {
  MAX_DASHBOARD_MAILBOXES,
  type RuntimeConfigForm,
  runtimeConfigFormEditedInput,
  runtimeConfigFormEditedMailboxesExceedLimit,
  runtimeConfigFormEquipmentColors,
  runtimeConfigFormSourceColorLevel,
} from "./model";

export const COLOR_LEVEL_MINIMUM = "0";
export const COLOR_LEVEL_MAXIMUM = "1000";

/** 一条表单字段错误；`path` 与控件 `data-runtime-*` 使用的配置路径一致。 */
export interface FieldIssue {
  readonly path: string;
  readonly kind: FieldIssueKind;
}

type FieldIssueKind =
  | { readonly kind: "mailbox-count" }
  | {
      readonly kind: "color-level";
      readonly number: number;
      readonly isMinimum: boolean;
      readonly issue: NumberIssue;
    }
  | {
      readonly kind: "number";
      readonly labelAtValidation: string;
      readonly minimum: string;
      readonly maximum: string;
      readonly issue: NumberIssue;
    };

/** 校验摘要与控件共用的当前字段错误；由表单页面提供，控件只读取。 */
export type FormIssues = Cell<readonly FieldIssue[]>;

/** 字段错误上下文；缺少提供方是装配错误，读取方会像 Rust 版一样直接失败。 */
export const FormIssuesContext = createContext<FormIssues>();

/** 把配置路径转成字段控件的 DOM id。 */
export function fieldId(path: string): string {
  return `runtime-field-${path.replaceAll(".", "-")}`;
}

/** 整数输入的本地错误类别。 */
export type NumberIssue = "integer" | "range";

/** 数字控件错误文案；范围错误使用控件自身的上下界文本。 */
export function numberIssueMessage(issue: NumberIssue, minimum: string, maximum: string): string {
  switch (issue) {
    case "integer":
      return t("admin-runtime-field-integer");
    case "range":
      return t("admin-runtime-field-range", { minimum, maximum });
    default:
      return unreachable(issue);
  }
}

const I128_MINIMUM = -(2n ** 127n);
const I128_MAXIMUM = 2n ** 127n - 1n;

/** 按 Rust `i128::from_str` 的完整词法读取十进制整数；越界与非法写法都返回 `null`。 */
function parseI128(text: string): bigint | null {
  if (!/^[+-]?[0-9]+$/u.test(text)) {
    return null;
  }
  const value = BigInt(text);
  return value >= I128_MINIMUM && value <= I128_MAXIMUM ? value : null;
}

/** 校验整数输入；上下界自身无法解析时按 Rust 版忽略该边界。 */
export function numberIssue(input: string, minimum: string, maximum: string): NumberIssue | null {
  const value = parseI128(input);
  if (value === null) {
    return "integer";
  }
  const minimumValue = parseI128(minimum);
  if (minimumValue !== null && value < minimumValue) {
    return "range";
  }
  const maximumValue = parseI128(maximum);
  if (maximumValue !== null && value > maximumValue) {
    return "range";
  }
  return null;
}

export function mailboxCountError(): string {
  return t("admin-runtime-mailbox-too-many", { maximum: MAX_DASHBOARD_MAILBOXES });
}

/** 装备标色规则某一等级边界的配置路径。 */
export function colorLevelPath(rowId: number, bound: string): string {
  return `equipment_colors.rules.${rowId}.level.${bound}`;
}

/** 历史值可以就地标错。摘要只拦截编辑后的输入，完整候选仍由后端校验。 */
export function collectIssues(form: RuntimeConfigForm): readonly FieldIssue[] {
  const issues: FieldIssue[] = [];
  if (runtimeConfigFormEditedMailboxesExceedLimit(form)) {
    issues.push({ path: "dashboard_mailboxes", kind: { kind: "mailbox-count" } });
  }
  runtimeConfigFormEquipmentColors(form).forEach((rule, index) => {
    const bounds = [
      ["min", rule.levelMin],
      ["max", rule.levelMax],
    ] as const;
    for (const [bound, input] of bounds) {
      const source = runtimeConfigFormSourceColorLevel(form, index, bound);
      if (source !== null && String(source) === input) {
        continue;
      }
      const issue = numberIssue(input, COLOR_LEVEL_MINIMUM, COLOR_LEVEL_MAXIMUM);
      if (issue !== null) {
        issues.push({
          path: colorLevelPath(rule.rowId, bound),
          kind: {
            kind: "color-level",
            number: index + 1,
            isMinimum: bound === "min",
            issue,
          },
        });
      }
    }
  });
  visitNumberControls((path, minimum, maximum, label) => {
    const input = runtimeConfigFormEditedInput(form, path);
    if (input === null) {
      return;
    }
    const issue = numberIssue(input, minimum, maximum);
    if (issue !== null) {
      issues.push({
        path,
        kind: {
          kind: "number",
          labelAtValidation: label(),
          minimum,
          maximum,
          issue,
        },
      });
    }
  });
  return issues;
}

/** 沿用已挂载控件的范围和标签；只在确认有错误后读取其完整分组名。 */
function visitNumberControls(
  visit: (path: string, minimum: string, maximum: string, label: () => string) => void,
): void {
  if (typeof document === "undefined") {
    return;
  }
  let fields: NodeListOf<Element>;
  try {
    fields = document.querySelectorAll("[data-runtime-number]");
  } catch {
    return;
  }
  for (const field of fields) {
    if (field.hasAttribute("disabled")) {
      continue;
    }
    const path = field.getAttribute("data-runtime-number");
    if (path === null) {
      continue;
    }
    const minimum = field.getAttribute("min") ?? "";
    const maximum = field.getAttribute("max") ?? "";
    visit(path, minimum, maximum, () => controlLabel(field, path));
  }
}

function currentNumberLabel(path: string): ControlLabelParts | null {
  if (typeof document === "undefined") {
    return null;
  }
  const field = document.getElementById(fieldId(path));
  return field === null ? null : readControlLabel(field, path);
}

/** 将所属分组带入字段名称，避免多个“启用”或“超时”在摘要和变更清单里无法区分。 */
export function controlLabel(field: Element, path: string): string {
  return formatControlLabel(readControlLabel(field, path));
}

interface ControlLabelParts {
  readonly label: string;
  readonly groups: readonly string[];
}

function readControlLabel(field: Element, path: string): ControlLabelParts {
  const label = field.getAttribute("data-runtime-label") ?? path;
  const groups: string[] = [];
  let ancestor = field.parentElement;
  while (ancestor !== null) {
    if (matchesRuntimeGroup(ancestor)) {
      const title = queryGroupTitle(ancestor);
      if (title !== null) {
        groups.push(trimRustWhitespace(title));
      }
    }
    ancestor = ancestor.parentElement;
  }
  return { label, groups };
}

function formatControlLabel(parts: ControlLabelParts): string {
  return parts.groups.reduce(
    (field, context) => t("admin-runtime-field-context", { context, field }),
    parts.label,
  );
}

function matchesRuntimeGroup(element: Element): boolean {
  try {
    return element.matches(".admin-runtime-group, .admin-runtime-section");
  } catch {
    return false;
  }
}

function queryGroupTitle(element: Element): string | null {
  try {
    return element.querySelector(":scope > h3, :scope > summary")?.textContent ?? null;
  } catch {
    return null;
  }
}

export function focusField(path: string): void {
  if (typeof document === "undefined") {
    return;
  }
  const field = document.getElementById(fieldId(path));
  if (field === null) {
    return;
  }
  let ancestor = field.parentElement;
  while (ancestor !== null) {
    if (ancestor.tagName === "DETAILS") {
      ancestor.setAttribute("open", "");
    }
    ancestor = ancestor.parentElement;
  }
  if (field instanceof HTMLElement) {
    field.focus();
    try {
      field.scrollIntoView(true);
    } catch {
      // 滚动失败不影响焦点移动。
    }
  }
}

export function focusSummary(): void {
  focusElementById("runtime-errors");
}

/** 字段错误摘要；渲染后聚焦摘要节点，链接触发字段聚焦。 */
export function ErrorSummary(props: { readonly issues: readonly FieldIssue[] }): JSX.Element {
  const locale = useLocaleSession();
  const numberLabels = createCell<NumberLabelSnapshot | null>(null);
  // 语言变化后等待控件与祖先标题更新，再从 DOM 读取实际名称。
  const dependency = createValueMemo(() => ({
    issues: props.issues,
    language: locale.current(),
  }));
  createEffect(dependency, (snapshot) => {
    const entries = [];
    for (const issue of snapshot.issues) {
      if (issue.kind.kind !== "number") {
        continue;
      }
      entries.push({ issue, label: currentNumberLabel(issue.path) });
    }
    const next = { language: snapshot.language, entries };
    if (!structuralEquals(numberLabels.peek(), next)) {
      numberLabels.set(next);
    }
  });
  const numbers = createMemo<ReadonlyMap<string, NumberCacheEntry>>(
    (previous) => {
      const snapshot = numberLabels();
      // 等当前语言的 DOM 快照到齐再格式化，避免新译文拼接旧字段名。
      if (snapshot === null || snapshot.language !== locale.current()) {
        return previous ?? new Map();
      }
      const formatted = new Map<string, NumberCacheEntry>();
      for (const { issue, label } of snapshot.entries) {
        formatted.set(issue.path, {
          kind: issue.kind,
          text: fieldIssueText(issue, label === null ? null : formatControlLabel(label)),
        });
      }
      return formatted;
    },
    { equals: structuralEquals },
  );
  const rows = createValueMemo((): readonly ErrorRow[] => {
    const cachedNumbers = numbers();
    return props.issues.map((issue) => {
      const cached = cachedNumbers.get(issue.path);
      if (cached !== undefined && structuralEquals(cached.kind, issue.kind)) {
        return { path: issue.path, message: cached.text };
      }
      return { path: issue.path, message: fieldIssueText(issue, null) };
    });
  });
  onSettled(() => {
    focusSummary();
  });
  return (
    <section
      id="runtime-errors"
      class="admin-config-errors"
      tabindex="-1"
      role="alert"
      aria-labelledby="runtime-errors-title"
    >
      <h3 id="runtime-errors-title">{t("admin-runtime-field-errors")}</h3>
      <ul>
        <For each={rows()} keyed={(row) => row.path}>
          {(row) => (
            <li>
              <a
                href={`#${fieldId(row().path)}`}
                onClick={(event) => {
                  event.preventDefault();
                  focusField(row().path);
                }}
              >
                {row().message}
              </a>
            </li>
          )}
        </For>
      </ul>
    </section>
  );
}

interface NumberLabelSnapshot {
  readonly language: LocaleCode;
  readonly entries: readonly {
    readonly issue: FieldIssue;
    readonly label: ControlLabelParts | null;
  }[];
}

interface NumberCacheEntry {
  readonly kind: FieldIssueKind;
  readonly text: string;
}

interface ErrorRow {
  readonly path: string;
  readonly message: string;
}

function fieldIssueText(issue: FieldIssue, controlLabelText: string | null): string {
  let label: string;
  let message: string;
  switch (issue.kind.kind) {
    case "mailbox-count":
      label = t("admin-runtime-section-mailboxes");
      message = mailboxCountError();
      break;
    case "color-level": {
      const field = issue.kind.isMinimum
        ? t("admin-runtime-color-level-min")
        : t("admin-runtime-color-level-max");
      const context = t("admin-runtime-color-rule", { number: issue.kind.number });
      label = t("admin-runtime-field-context", { context, field });
      message = numberIssueMessage(issue.kind.issue, COLOR_LEVEL_MINIMUM, COLOR_LEVEL_MAXIMUM);
      break;
    }
    case "number":
      label = controlLabelText ?? issue.kind.labelAtValidation;
      message = numberIssueMessage(issue.kind.issue, issue.kind.minimum, issue.kind.maximum);
      break;
    default:
      return unreachable(issue.kind);
  }
  return `${label}: ${message}`;
}
