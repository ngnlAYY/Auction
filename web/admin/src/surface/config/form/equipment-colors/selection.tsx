// 标色规则的原生复选框组；候选关联只影响展示，已选值由用户明确取消。

import type { Cell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Show } from "solid-js";

import { trimRustWhitespace } from "../../../../api/session-time";
import {
  type RuntimeConfigForm,
  runtimeConfigFormEquipmentColors,
  runtimeConfigFormResolveEquipmentColorSelection,
  runtimeConfigFormSetEquipmentColorField,
} from "../../model";

/** 复选框均有原生名称和可见标签；空过滤集合表示任意，词缀集合仍保留原有空值语义。 */
export function ColorMultiSelect(props: {
  readonly form: Cell<RuntimeConfigForm>;
  readonly index: number;
  readonly field: string;
  readonly label: string;
  readonly options: readonly string[];
  readonly value: string;
  readonly disabled: boolean;
  readonly onDirty: () => void;
  readonly conflictingSingle?: string | null;
  readonly allowAny?: boolean;
  readonly legacyExactType?: string | null;
  readonly materialNames?: readonly string[];
}): JSX.Element {
  const selected = createValueMemo(() => selectedValues(props.value));
  const choices = createValueMemo(() => {
    const result = [...props.options];
    for (const value of selected()) {
      if (!result.some((option) => eqIgnoreAsciiCase(option, value))) {
        result.push(value);
      }
    }
    return result;
  });
  const name = createValueMemo(() => `color-${props.field}-${props.index}`);
  const hintId = createValueMemo(() => `${name()}-hint`);
  const conflictId = createValueMemo(() => `${name()}-conflict`);
  const hasConflict = createValueMemo(() => (props.conflictingSingle ?? null) !== null);
  const allowAny = createValueMemo(() => props.allowAny ?? false);

  return (
    <fieldset
      class="admin-runtime-color-multi"
      disabled={props.disabled}
      aria-invalid={hasConflict() ? "true" : "false"}
      aria-describedby={hasConflict() ? conflictId() : undefined}
    >
      <legend>{props.label}</legend>
      <Show when={hasConflict()}>
        <p class="admin-config-field-error" id={conflictId()}>
          {t("admin-runtime-color-selection-conflict")}
        </p>
        <button
          type="button"
          class="admin-btn-secondary"
          onClick={() => {
            props.form.update((current) =>
              runtimeConfigFormResolveEquipmentColorSelection(
                current,
                props.index,
                props.field,
                true,
              ),
            );
            props.onDirty();
          }}
        >
          {t("admin-runtime-color-keep-single", { value: props.conflictingSingle ?? "" })}
        </button>
        <button
          type="button"
          class="admin-btn-secondary"
          onClick={() => {
            props.form.update((current) =>
              runtimeConfigFormResolveEquipmentColorSelection(
                current,
                props.index,
                props.field,
                false,
              ),
            );
            props.onDirty();
          }}
        >
          {t("admin-runtime-color-keep-multiple")}
        </button>
      </Show>
      <Show when={allowAny()}>
        <button
          type="button"
          disabled={props.disabled || hasConflict() || selected().length === 0}
          onClick={() => {
            updateSelection(props.form, props.index, props.field, null, false);
            props.onDirty();
          }}
        >
          {t("admin-runtime-color-any")}
        </button>
      </Show>
      <div class="admin-runtime-color-multi__options">
        <For each={choices()} keyed={(option) => option}>
          {(option, position) => {
            const materialLabel = createValueMemo(() => {
              const value = option();
              if (
                props.field === "equipment_type" &&
                (props.materialNames ?? []).some((name) => eqIgnoreAsciiCase(name, value))
              ) {
                const legacy = props.legacyExactType ?? null;
                if (legacy !== null && eqIgnoreAsciiCase(legacy, value)) {
                  return t("admin-runtime-color-material-legacy", { material: value });
                }
                return t("admin-runtime-color-material-all", { material: value });
              }
              return value;
            });
            return (
              <label class="admin-runtime-color-multi__option" for={`${name()}-${position()}`}>
                <input
                  id={`${name()}-${position()}`}
                  name={name()}
                  type="checkbox"
                  value={option()}
                  checked={selected().some((value) => eqIgnoreAsciiCase(value, option()))}
                  disabled={hasConflict()}
                  aria-describedby={hintId()}
                  onChange={(event) => {
                    updateSelection(
                      props.form,
                      props.index,
                      props.field,
                      option(),
                      event.currentTarget.checked,
                    );
                    props.onDirty();
                  }}
                />
                <span>{materialLabel()}</span>
              </label>
            );
          }}
        </For>
      </div>
      <small id={hintId()}>
        {allowAny() ? t("admin-runtime-color-filter-hint") : t("admin-runtime-color-multi-hint")}
      </small>
    </fieldset>
  );
}

function updateSelection(
  form: Cell<RuntimeConfigForm>,
  index: number,
  field: string,
  option: string | null,
  checked: boolean,
): void {
  // 每次读取最新规则，避免连续勾选使用渲染时的旧快照覆盖已选项。
  const rule = runtimeConfigFormEquipmentColors(form.peek())[index];
  if (rule === undefined) {
    return;
  }
  let current: string;
  switch (field) {
    case "category":
      current = rule.category.text;
      break;
    case "equipment_type":
      current = rule.equipmentType.text;
      break;
    case "quality":
      current = rule.quality.text;
      break;
    case "prefixes":
      current = rule.prefixes;
      break;
    case "suffixes":
      current = rule.suffixes;
      break;
    default:
      return;
  }
  const selected = selectedValues(current);
  // 词表只是候选，不是历史规则白名单。改分类不得删除另一维选择，尤其不能把精确种类
  // 清空成“任意”；未知、历史及暂不相容的选择都继续可见，直到用户手动取消。
  if (option === null) {
    selected.length = 0;
  } else {
    const kept = selected.filter((value) => !eqIgnoreAsciiCase(value, option));
    selected.length = 0;
    selected.push(...kept);
    if (checked) {
      selected.push(option);
    }
  }
  form.update((latest) =>
    runtimeConfigFormSetEquipmentColorField(latest, index, field, selected.join("\n")),
  );
}

/** 多选值按行读取（`\n` 或 `\r\n`），去掉首尾空白并丢弃空行；更新时再用 `\n` 拼回，逗号不是分隔符。 */
function selectedValues(value: string): string[] {
  const result: string[] = [];
  let start = 0;
  while (start < value.length) {
    const end = value.indexOf("\n", start);
    const raw = end < 0 ? value.slice(start) : value.slice(start, end);
    const line = trimRustWhitespace(raw.endsWith("\r") ? raw.slice(0, -1) : raw);
    if (line !== "") {
      result.push(line);
    }
    if (end < 0) {
      break;
    }
    start = end + 1;
  }
  return result;
}

/** `eq_ignore_ascii_case` 只折叠 ASCII 大小写；非 ASCII 字符必须完全相同。 */
function eqIgnoreAsciiCase(left: string, right: string): boolean {
  if (left.length !== right.length) {
    return false;
  }
  for (let i = 0; i < left.length; i += 1) {
    if (asciiLower(left.charCodeAt(i)) !== asciiLower(right.charCodeAt(i))) {
      return false;
    }
  }
  return true;
}

function asciiLower(code: number): number {
  return code >= 0x41 && code <= 0x5a ? code + 0x20 : code;
}
