// 运行时配置的通用字段控件与分组容器。

import type { Cell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Show } from "solid-js";

import { isHot } from "../hot-fields";
import {
  type RuntimeConfigForm,
  runtimeConfigFormBoolean,
  runtimeConfigFormInput,
  runtimeConfigFormSetBoolean,
  runtimeConfigFormSetInput,
  runtimeConfigFormSourceInput,
} from "../model";
import { fieldId } from "../validation";
import { NumberInput } from "./number";

/** 可折叠的配置分组。 */
export function RuntimeSection(props: {
  readonly title: string;
  readonly open: boolean;
  readonly children: JSX.Element;
}): JSX.Element {
  return (
    <details class="admin-runtime-section" open={props.open}>
      <summary>
        <span>{props.title}</span>
      </summary>
      <div class="admin-runtime-section__body">{props.children}</div>
    </details>
  );
}

/** 分组内的小节标题。 */
export function RuntimeGroup(props: {
  readonly title: string;
  readonly children: JSX.Element;
}): JSX.Element {
  return (
    <section class="admin-runtime-group">
      <h3>{props.title}</h3>
      {props.children}
    </section>
  );
}

/** 布尔开关字段。 */
export function SwitchField(props: {
  readonly form: Cell<RuntimeConfigForm>;
  readonly disabled: boolean;
  readonly path: string;
  readonly label: string;
  readonly onDirty: () => void;
}): JSX.Element {
  // 整份草稿会随每次输入变化；只让当前字段值的变化触发控件重绘。
  const checked = createValueMemo(() => runtimeConfigFormBoolean(props.form(), props.path));
  return (
    <label class="admin-runtime-switch">
      <input
        id={fieldId(props.path)}
        name={props.path}
        data-runtime-label={props.label}
        type="checkbox"
        disabled={props.disabled}
        checked={checked()}
        onChange={(event) => {
          const value = event.currentTarget.checked;
          props.form.update((current) => runtimeConfigFormSetBoolean(current, props.path, value));
          props.onDirty();
        }}
      />
      <span class="admin-runtime-switch__track" aria-hidden="true" />
      <span class="admin-runtime-switch__label">
        {props.label}
        <Show when={isHot(props.path)}>
          <span class="admin-runtime-hot-badge" title={t("admin-runtime-hot-field-title")}>
            {t("admin-runtime-hot-field-badge")}
          </span>
        </Show>
      </span>
    </label>
  );
}

/** 文本字段。 */
export function TextField(props: {
  readonly form: Cell<RuntimeConfigForm>;
  readonly disabled: boolean;
  readonly path: string;
  readonly label: string;
  readonly onDirty: () => void;
}): JSX.Element {
  const value = createValueMemo(() => runtimeConfigFormInput(props.form(), props.path));
  return (
    <label class="admin-runtime-field">
      <span>
        {props.label}
        <Show when={isHot(props.path)}>
          <span class="admin-runtime-hot-badge" title={t("admin-runtime-hot-field-title")}>
            {t("admin-runtime-hot-field-badge")}
          </span>
        </Show>
      </span>
      <input
        id={fieldId(props.path)}
        name={props.path}
        autocomplete="off"
        autocapitalize="none"
        spellcheck="false"
        data-runtime-label={props.label}
        type="text"
        disabled={props.disabled}
        value={value()}
        onInput={(event) => {
          const next = event.currentTarget.value;
          props.form.update((current) => runtimeConfigFormSetInput(current, props.path, next));
          props.onDirty();
        }}
      />
    </label>
  );
}

/** 数值字段；原始值的范围错误由 `NumberInput` 就地标出。 */
export function NumberField(props: {
  readonly form: Cell<RuntimeConfigForm>;
  readonly disabled: boolean;
  readonly path: string;
  readonly label: string;
  readonly minimum: string;
  readonly maximum: string;
  readonly onDirty: () => void;
}): JSX.Element {
  const input = createValueMemo(() => {
    const form = props.form();
    // 由控件报告原始值的范围错误，未编辑的候选字段仍保留原值。
    return {
      value: runtimeConfigFormInput(form, props.path),
      original: runtimeConfigFormSourceInput(form, props.path),
    };
  });
  return (
    <NumberInput
      path={props.path}
      label={props.label}
      minimum={props.minimum}
      maximum={props.maximum}
      original={input().original}
      value={input().value}
      disabled={props.disabled}
      onInput={(value) => {
        props.form.update((current) => runtimeConfigFormSetInput(current, props.path, value));
        props.onDirty();
      }}
    />
  );
}

/** 以换行分隔的列表字段。 */
export function ListField(props: {
  readonly form: Cell<RuntimeConfigForm>;
  readonly disabled: boolean;
  readonly path: string;
  readonly label: string;
  readonly onDirty: () => void;
}): JSX.Element {
  const value = createValueMemo(() => runtimeConfigFormInput(props.form(), props.path));
  return (
    <label class="admin-runtime-field admin-runtime-field--list">
      <span>{props.label}</span>
      <textarea
        id={fieldId(props.path)}
        name={props.path}
        autocomplete="off"
        autocapitalize="none"
        spellcheck="false"
        data-runtime-label={props.label}
        disabled={props.disabled}
        rows="3"
        value={value()}
        onInput={(event) => {
          const next = event.currentTarget.value;
          props.form.update((current) => runtimeConfigFormSetInput(current, props.path, next));
          props.onDirty();
        }}
      />
      <small>{t("admin-runtime-list-hint")}</small>
    </label>
  );
}

/** 下拉选择字段。 */
export function SelectField(props: {
  readonly form: Cell<RuntimeConfigForm>;
  readonly disabled: boolean;
  readonly path: string;
  readonly label: string;
  readonly options: readonly (readonly [value: string, label: string])[];
  readonly onDirty: () => void;
}): JSX.Element {
  const value = createValueMemo(() => runtimeConfigFormInput(props.form(), props.path));
  // Solid 在插入选项后绑定 value；没有匹配项时沿用首次挂载的首项占位。
  const displayedValue = (): string => {
    const current = value();
    const options = props.options;
    return options.some(([option]) => option === current) ? current : (options[0]?.[0] ?? "");
  };
  return (
    <label class="admin-runtime-field">
      <span>{props.label}</span>
      <select
        id={fieldId(props.path)}
        name={props.path}
        autocomplete="off"
        data-runtime-label={props.label}
        disabled={props.disabled}
        value={displayedValue()}
        onChange={(event) => {
          const next = event.currentTarget.value;
          props.form.update((current) => runtimeConfigFormSetInput(current, props.path, next));
          props.onDirty();
        }}
      >
        <For each={props.options} keyed={(option) => option[0]}>
          {(option) => <option value={option()[0]}>{option()[1]}</option>}
        </For>
      </select>
    </label>
  );
}
