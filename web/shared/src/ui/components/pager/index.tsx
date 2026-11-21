// Public 与 Admin 长列表共用分页器。
// 文案一律由调用方经 props 传入，本 crate 不持有任何 i18n key；DOM class 保持
// `ui-pager*` 契约不变。调用方显式提供每页数量选项时才启用数字页码布局。
// 页码导航位于数量和跳页之前；数字组独立换行，响应式样式不复制或重挂载表单。

import type { JSX } from "@solidjs/web";
import { createEffect, For, Show, untrack } from "solid-js";

import { createCell } from "../../../reactive/cell";
import { createValueMemo } from "../../../reactive/memo";
import { PageSizePicker } from "./size";

/** 分页器当前页、总量口径、文案回调与可选数字页码配置。 */
export interface PagerProps {
  readonly page: number;
  readonly pageSize: number;
  /** 后端返回的总条数；`null`/省略表示只有 `hasMore` 游标语义。 */
  readonly total?: number | null;
  /** 游标语义下是否还有下一页；仅在 `total` 为 `null` 时参与计算。 */
  readonly hasMore?: boolean;
  /** 可访问页码上限；不改变权威 total，只限制后端 offset 边界内的导航。 */
  readonly maxPage?: number | null;
  /** `nav` 的 `aria-label`。 */
  readonly label: string;
  readonly previousLabel: string;
  /** 页码状态模板回调：`([当前页, 总页数]) -> 已翻译文案`。 */
  readonly status: (value: readonly [number, number]) => string;
  readonly nextLabel: string;
  readonly jumpLabel: string;
  readonly jumpButtonLabel: string;
  /** 返回 false 表示翻页未提交，恢复当前已应用的输入值。 */
  readonly onPageChange: (page: number) => unknown;
  /** 页码纠偏可替换历史记录，返回 false 时允许重试，省略时沿用 `onPageChange`。 */
  readonly onPageClamp?: ((page: number) => unknown) | null;
  /** 调用方提供写入失败提示与纠偏重试按钮文案。 */
  readonly clampFailure?: { readonly message: string; readonly retryLabel: string };
  /** 非空时启用数字分页与原生数量选择器；选项和文案归调用方。 */
  readonly pageSizeOptions?: readonly number[];
  readonly pageSizeLabel?: string;
  readonly pageSizeOptionLabel?: ((size: number) => string) | null;
  /** 可选业务数量上限；默认无限制，保持 Public 既有语义。 */
  readonly maxPageSize?: number | null;
  /** 显式启用正整数自定义输入，不改变默认布局。 */
  readonly allowCustomPageSize?: boolean;
  readonly pageSizeCustomLabel?: string;
  readonly pageSizeApplyLabel?: string;
  readonly pageSizeInvalidLabel?: string;
  /** 仅有权威 total 时由调用方提供总数文案，不把当前页长度当总数。 */
  readonly totalSummary?: string;
  /**
   * 数字页码布局左端的条目范围文案；参数为 `[起始序号, 结束序号, 总数]`。
   * 提供后替代 `totalSummary`，避免同一工具条重复显示总数。
   */
  readonly rangeSummary?: ((value: readonly [number, number, number]) => string) | null;
  readonly onPageSizeChange?: (size: number) => unknown;
}

/**
 * 展示长列表分页，并把目标页交给调用页面更新 URL 或查询状态。
 * - `total` 有值：后端返回总数的稳定分页；
 * - `total === null` + `hasMore`：游标语义的兼容投影，可跳转范围限制到当前已知下一页。
 * 页码与总量变化时尝试纠偏。可选页受总量约束，显示值以已提交的页码为准。
 */
export function Pager(props: PagerProps): JSX.Element {
  const pageSize = createValueMemo(() =>
    Number.isSafeInteger(props.pageSize) && props.pageSize > 0 ? props.pageSize : 1,
  );
  const appliedPage = createValueMemo(() =>
    Number.isSafeInteger(props.page) && props.page > 0 ? props.page : 1,
  );
  const totalPages = createValueMemo(() => {
    const rawTotal = props.total ?? null;
    const total =
      rawTotal === null ? null : Number.isSafeInteger(rawTotal) && rawTotal >= 0 ? rawTotal : 0;
    const raw =
      total === null
        ? props.hasMore === true
          ? saturatingAdd(appliedPage(), 1)
          : appliedPage()
        : total <= 0
          ? 1
          : 1 + Math.floor((total - 1) / pageSize());
    const rawMaximum = props.maxPage ?? Number.MAX_SAFE_INTEGER;
    const maximum = Number.isSafeInteger(rawMaximum) && rawMaximum > 0 ? rawMaximum : 1;
    return Math.min(Math.max(raw, 1), maximum);
  });
  const validPage = createValueMemo(() => Math.min(appliedPage(), totalPages()));
  const jumpPage = createCell(untrack(appliedPage).toString());
  const clampRejected = createCell(false);
  let jumpInput: HTMLInputElement | undefined;

  const showJumpPage = (page: number) => {
    const value = page.toString();
    jumpPage.set(value);
    if (jumpInput !== undefined) jumpInput.value = value;
  };
  createEffect(appliedPage, showJumpPage);

  const changePage = (page: number, write = props.onPageChange): boolean => {
    if (write(page) === false) {
      // 地址未写入时回显已提交页码，原生控件也可能持有尚未生效的输入。
      showJumpPage(untrack(appliedPage));
      return false;
    }
    return true;
  };

  // 总数只决定可用页与纠偏目标。写入失败后由用户重试，避免反复改写同一地址。
  const clampDependency = createValueMemo(() => [props.page, validPage()] as const);
  const correctPage = ([page, target]: readonly [number, number]) =>
    clampRejected.set(
      page !== target &&
        !untrack(() => changePage(target, props.onPageClamp ?? props.onPageChange)),
    );
  createEffect(clampDependency, correctPage);
  const retryClamp = () => {
    if (clampRejected.peek()) correctPage(clampDependency());
  };

  const jumpTarget = createValueMemo(() => {
    const clean = rustTrim(jumpPage());
    if (!asciiDigits(clean)) {
      return null;
    }
    const target = Number(clean);
    return Number.isSafeInteger(target) && target >= 1 && target <= totalPages() ? target : null;
  });
  const numbered = createValueMemo(() => (props.pageSizeOptions?.length ?? 0) > 0);
  // 只有已提交的页仍在范围内，才给它显示条目序号。
  const itemRange = createValueMemo((): readonly [number, number, number] | null => {
    const total = props.total ?? null;
    if (total === null) {
      return null;
    }
    if (total <= 0) {
      return [0, 0, 0];
    }
    const current = appliedPage();
    if (current !== validPage()) {
      return null;
    }
    const start = saturatingAdd(saturatingMultiply(current - 1, pageSize()), 1);
    const end = Math.min(saturatingMultiply(current, pageSize()), total);
    return [start, end, total];
  });
  const rangeRendered = createValueMemo(
    () => numbered() && itemRange() !== null && props.rangeSummary != null,
  );
  const rangeText = createValueMemo(() => {
    const range = itemRange();
    const template = props.rangeSummary;
    return range === null || template == null ? null : template(range);
  });
  const pageItems = createValueMemo(() => pageEntries(validPage(), totalPages()));

  const controls = (
    <nav class={numbered() ? "ui-pager ui-pager--numbered" : "ui-pager"} aria-label={props.label}>
      <Show when={numbered() && rangeText() !== null}>
        <span class="ui-pager__range">{rangeText()}</span>
      </Show>
      <div class="ui-pager__controls">
        <button
          type="button"
          disabled={validPage() <= 1}
          onClick={() => changePage(validPage() - 1)}
          aria-label={props.previousLabel}
        >
          <Show when={numbered()} fallback={props.previousLabel}>
            <span aria-hidden="true">‹</span>
          </Show>
        </button>
        <Show when={numbered()}>
          <div class="ui-pager__pages">
            <For each={pageItems()} keyed={(item) => item.key}>
              {(item) => (
                <Show
                  when={item().target}
                  fallback={
                    <span class="ui-pager__ellipsis" aria-hidden="true">
                      {"…"}
                    </span>
                  }
                >
                  {(target) => (
                    <button
                      type="button"
                      class="ui-pager__page"
                      aria-label={props.status([target(), totalPages()])}
                      aria-current={target() === appliedPage() ? "page" : "false"}
                      onClick={() => {
                        if (target() !== appliedPage()) {
                          changePage(target());
                        }
                      }}
                    >
                      {target()}
                    </button>
                  )}
                </Show>
              )}
            </For>
          </div>
        </Show>
        <span class={numbered() ? "ui-pager__sr-only" : "ui-pager__status"} aria-live="polite">
          {props.status([appliedPage(), totalPages()])}
        </span>
        <button
          type="button"
          disabled={validPage() >= totalPages()}
          onClick={() => changePage(saturatingAdd(validPage(), 1))}
          aria-label={props.nextLabel}
        >
          <Show when={numbered()} fallback={props.nextLabel}>
            <span aria-hidden="true">›</span>
          </Show>
        </button>
      </div>
      <Show when={numbered()}>
        <Show
          when={
            !rangeRendered() && (props.total ?? null) !== null && (props.totalSummary ?? "") !== ""
          }
        >
          <span class="ui-pager__total">{props.totalSummary}</span>
        </Show>
        <PageSizePicker
          pageSize={pageSize()}
          options={props.pageSizeOptions ?? []}
          label={props.pageSizeLabel ?? ""}
          optionLabel={props.pageSizeOptionLabel}
          allowCustom={props.allowCustomPageSize === true}
          maxPageSize={props.maxPageSize}
          customLabel={props.pageSizeCustomLabel ?? ""}
          applyLabel={props.pageSizeApplyLabel ?? ""}
          invalidLabel={props.pageSizeInvalidLabel ?? ""}
          onChange={props.onPageSizeChange}
        />
      </Show>
      <form
        class="ui-pager__jump"
        novalidate
        onSubmit={(event) => {
          event.preventDefault();
          const target = jumpTarget();
          if (target !== null) {
            changePage(target);
          }
        }}
      >
        <label class="ui-pager__label">
          <span>{props.jumpLabel}</span>
          <input
            class="ui-pager__input"
            type="number"
            name="page"
            min="1"
            max={totalPages().toString()}
            step="1"
            inputmode="numeric"
            autocomplete="off"
            enterkeyhint="go"
            ref={(element) => {
              jumpInput = element;
            }}
            value={jumpPage()}
            onInput={(event) => jumpPage.set(event.currentTarget.value)}
          />
        </label>
        <button type="submit" disabled={jumpTarget() === null}>
          {props.jumpButtonLabel}
        </button>
      </form>
    </nav>
  );
  return (
    <>
      {controls}
      <Show when={props.clampFailure}>
        {(failure) => (
          <div class="ui-pager__error" role="status" aria-live="polite">
            <Show when={clampRejected() && props.page !== validPage()}>
              <span>{failure().message}</span>{" "}
              <button type="button" onClick={retryClamp}>
                {failure().retryLabel}
              </button>
            </Show>
          </div>
        )}
      </Show>
    </>
  );
}

interface PageEntry {
  readonly key: string;
  readonly target: number | null;
}

function pageEntries(page: number, totalPages: number): PageEntry[] {
  return numberedPages(page, totalPages).map((target, index) => ({
    key: target === null ? `ellipsis-${index}` : `page-${target}`,
    target,
  }));
}

/** 最多生成七个页码/省略项；固定栈缓冲不分配临时 Vec，省略号仍不可点击。 */
function numberedPages(page: number, totalPages: number): (number | null)[] {
  let pages: (number | null)[];
  if (totalPages <= 7) {
    pages = [1, 2, 3, 4, 5, 6, 7];
  } else if (page <= 4) {
    pages = [1, 2, 3, 4, 5, null, totalPages];
  } else if (page >= totalPages - 3) {
    pages = [1, null, totalPages - 4, totalPages - 3, totalPages - 2, totalPages - 1, totalPages];
  } else {
    pages = [1, null, page - 1, page, page + 1, null, totalPages];
  }
  return pages.slice(0, Math.min(Math.max(totalPages, 0), 7));
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

function saturatingAdd(value: number, addend: number): number {
  const sum = value + addend;
  return Number.isSafeInteger(sum) ? sum : Number.MAX_SAFE_INTEGER;
}

function saturatingMultiply(value: number, multiplier: number): number {
  const product = value * multiplier;
  return Number.isSafeInteger(product) ? product : Number.MAX_SAFE_INTEGER;
}

function rustTrim(value: string): string {
  let start = 0;
  while (start < value.length && isRustWhitespace(value.charCodeAt(start))) {
    start += 1;
  }
  let end = value.length;
  while (end > start && isRustWhitespace(value.charCodeAt(end - 1))) {
    end -= 1;
  }
  return value.slice(start, end);
}

function isRustWhitespace(code: number): boolean {
  return (
    (code >= 0x0009 && code <= 0x000d) ||
    code === 0x0020 ||
    code === 0x0085 ||
    code === 0x00a0 ||
    code === 0x1680 ||
    (code >= 0x2000 && code <= 0x200a) ||
    code === 0x2028 ||
    code === 0x2029 ||
    code === 0x202f ||
    code === 0x205f ||
    code === 0x3000
  );
}
