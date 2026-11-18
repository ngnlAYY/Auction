// 分页数量预设与正整数输入；只提交可表示的合法整数，不截断或静默修正用户输入。

import type { JSX } from "@solidjs/web";
import { createEffect, For, onSettled, Show } from "solid-js";

import { createCell } from "../../../reactive/cell";
import { createValueMemo } from "../../../reactive/memo";

// 同一页面可能同时挂载多个分页控件，各输入只能关联自己的提示。
let nextHintId = 1;

/** 每页数量选择器的当前值、候选值、文案与提交回调。 */
export interface PageSizePickerProps {
  readonly pageSize: number;
  readonly options: readonly number[];
  readonly label: string;
  readonly optionLabel?: ((size: number) => string) | null | undefined;
  readonly allowCustom: boolean;
  readonly maxPageSize?: number | null | undefined;
  readonly customLabel: string;
  readonly applyLabel: string;
  readonly invalidLabel: string;
  /** 返回 false 时恢复浏览器已经改变的控件值。 */
  readonly onChange?: ((size: number) => unknown) | undefined;
}

/** 数量文案与可选上限由 surface 注入；显式启用才显示自定义输入。 */
export function PageSizePicker(props: PageSizePickerProps): JSX.Element {
  const input = createCell(props.pageSize.toString());
  const touched = createCell(false);
  let inputElement: HTMLInputElement | undefined;
  const hintId = `page-size-hint-${nextHintId}`;
  nextHintId += 1;
  const pageSize = createValueMemo(() => props.pageSize);

  createEffect(pageSize, (size) => {
    input.set(size.toString());
    touched.set(false);
  });

  // 不用浮点转换或 type=number：科学计数法、符号、空白和小数都不能成为数量。
  const target = createValueMemo(() => {
    const value = input();
    if (!asciiDigits(value)) {
      return null;
    }
    const size = Number(value);
    if (
      !Number.isSafeInteger(size) ||
      size <= 0 ||
      (props.maxPageSize != null && size > props.maxPageSize)
    ) {
      return null;
    }
    return size;
  });
  const invalid = createValueMemo(() => touched() && target() === null);
  const customSelected = createValueMemo(() => !props.options.includes(props.pageSize));

  return (
    <>
      <label class="ui-pager__size">
        <span class="ui-pager__sr-only">{props.label}</span>
        <select
          class="ui-pager__select"
          name="page_size"
          autocomplete="off"
          value={props.pageSize.toString()}
          onChange={(event) => {
            const select = event.currentTarget;
            const value = select.value;
            const size = Number(value);
            if (
              !asciiDigits(value) ||
              !Number.isSafeInteger(size) ||
              size <= 0 ||
              (props.maxPageSize != null && size > props.maxPageSize) ||
              !props.options.includes(size) ||
              props.onChange?.(size) === false
            ) {
              // 原生 change 已经改变选中项；写入被拒时 props 不变，不会触发重新渲染。
              select.value = props.pageSize.toString();
            }
          }}
        >
          <For each={props.options} keyed={(size) => size}>
            {(size) => (
              <option
                value={size().toString()}
                selected={size() === props.pageSize}
                disabled={size() <= 0 || (props.maxPageSize != null && size() > props.maxPageSize)}
              >
                {optionLabel(props.optionLabel, size())}
              </option>
            )}
          </For>
          <Show when={customSelected()}>
            <option value={props.pageSize.toString()} selected={true}>
              {optionLabel(props.optionLabel, props.pageSize)}
            </option>
          </Show>
        </select>
      </label>
      <Show when={props.allowCustom}>
        <form
          class="ui-pager__custom"
          novalidate
          onSubmit={(event) => {
            event.preventDefault();
            touched.set(true);
            const size = target();
            if (size !== null) {
              if (props.onChange?.(size) === false) {
                const applied = props.pageSize.toString();
                input.set(applied);
                touched.set(false);
                if (inputElement !== undefined) {
                  inputElement.value = applied;
                }
              }
            } else {
              onSettled(() => {
                inputElement?.focus();
              });
            }
          }}
        >
          <span id={hintId} class="ui-pager__sr-only">
            {props.invalidLabel}
          </span>
          <label class="ui-pager__label">
            <span>{props.customLabel}</span>
            <input
              class="ui-pager__input"
              type="text"
              name="custom_page_size"
              inputmode="numeric"
              pattern="[0-9]*[1-9][0-9]*"
              required
              autocomplete="off"
              enterkeyhint="done"
              ref={(element) => {
                inputElement = element;
              }}
              aria-describedby={hintId}
              aria-invalid={invalid() ? "true" : "false"}
              title={props.invalidLabel}
              value={input()}
              onInput={(event) => input.set(event.currentTarget.value)}
              onBlur={() => touched.set(true)}
            />
          </label>
          <button type="submit">{props.applyLabel}</button>
          <Show when={invalid()}>
            <span class="ui-pager__error" role="status">
              {props.invalidLabel}
            </span>
          </Show>
        </form>
      </Show>
    </>
  );
}

function optionLabel(
  callback: ((size: number) => string) | null | undefined,
  size: number,
): string {
  return callback?.(size) ?? size.toString();
}

function asciiDigits(value: string): boolean {
  if (value.length === 0) {
    return false;
  }
  for (let index = 0; index < value.length; index += 1) {
    const code = value.charCodeAt(index);
    if (code < 48 || code > 57) {
      return false;
    }
  }
  return true;
}
