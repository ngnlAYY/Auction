// Admin 列表表格的表头排序共享实现。
// 服务端 admin 列表端点不接受排序参数（`api/` 各列表请求与 `auction-contracts` DTO 均无
// sort/order 字段，服务端按固定顺序返回当前页），因此排序只在浏览器内对当前页数据执行；
// 表头 tooltip 明确「仅当前页」局限。排序状态经 `query/` 与筛选/分页一起入 URL，
// 刷新、分享链接与浏览器前进/后退都能恢复。

import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For } from "solid-js";

/**
 * 排序方向。地址栏 `dir` 只区分 `desc` 与其余值（含缺省），非法值一律回退升序。
 */
export type SortDirection = "asc" | "desc";

/** 解析地址栏 `dir` 值。 */
export function sortDirectionParse(value: string | null): SortDirection {
  return value === "desc" ? "desc" : "asc";
}

/** 序列化为地址栏值；升序是默认值，返回 `null` 表示省略该键，保持地址栏干净。 */
export function sortDirectionAsQueryValue(direction: SortDirection): "desc" | null {
  return direction === "desc" ? "desc" : null;
}

/** 点击已排序列时切换方向。 */
export function sortDirectionToggled(direction: SortDirection): SortDirection {
  return direction === "asc" ? "desc" : "asc";
}

/** `aria-sort` 的取值（仅当前排序列使用）。 */
function sortDirectionAria(direction: SortDirection): "ascending" | "descending" {
  return direction === "asc" ? "ascending" : "descending";
}

/** 表头的方向箭头，仅作视觉指示，对辅助技术隐藏。 */
function sortDirectionArrow(direction: SortDirection): "▲" | "▼" {
  return direction === "asc" ? "▲" : "▼";
}

/** 表头点击后的下一排序状态：同列切换方向，切到另一列时从升序开始。 */
export function nextSort(
  active: string,
  direction: SortDirection,
  column: string,
): [string, SortDirection] {
  return active === column ? [column, sortDirectionToggled(direction)] : [column, "asc"];
}

/** 按排序方向应用比较结果：升序保持、降序反转。配合稳定的 `sort` 保证等值行顺序确定。 */
export function directed(ordering: number, direction: SortDirection): number {
  if (ordering === 0) {
    return 0;
  }
  return direction === "asc" ? ordering : -ordering;
}

/**
 * 对当前页行排序：`column` 为空时保持服务端原始顺序；`compare` 对未知列返回
 * `0`，稳定排序因此保留这些行的原始相对顺序。
 */
export function sortRows<T>(
  rows: T[],
  column: string,
  direction: SortDirection,
  compare: (left: T, right: T, column: string) => number,
): void {
  if (column === "") {
    return;
  }
  rows.sort((left, right) => directed(compare(left, right, column), direction));
}

/**
 * 窄屏卡片视图的排序入口：表头被隐藏后，用同一 URL 排序状态驱动列选择与方向切换。
 * 列标签由调用页面提供，空列表示恢复服务端原始顺序。
 */
export function MobileSortControl(props: {
  readonly columns: readonly (readonly [string, string])[];
  readonly sort: string;
  readonly direction: SortDirection;
  readonly onSortChange: (value: [string, SortDirection]) => void;
  /** 显示未指定列时的顺序。提供标签的列表仅在有选定列时允许切换方向。 */
  readonly defaultOrderLabel?: string;
}): JSX.Element {
  const directionDisabled = createValueMemo(
    () => props.sort === "" && props.defaultOrderLabel !== undefined,
  );
  const defaultOption = createValueMemo(() => props.defaultOrderLabel ?? t("value-empty"));
  const directionHint = createValueMemo(() => {
    if (directionDisabled()) {
      return defaultOption();
    }
    return props.direction === "asc"
      ? t("admin-sort-ascending-hint")
      : t("admin-sort-descending-hint");
  });
  return (
    <div class="admin-mobile-sort">
      <label class="admin-mobile-sort__field">
        <select
          value={props.sort}
          aria-label={t("admin-sort-inactive-hint")}
          onChange={(event) => {
            const next = event.currentTarget.value;
            const nextDirection = next === props.sort ? props.direction : "asc";
            props.onSortChange([next, nextDirection]);
          }}
        >
          <option value="" selected={props.sort === ""}>
            {defaultOption()}
          </option>
          <For each={props.columns} keyed={false}>
            {(column) => (
              <option value={column()[0]} selected={props.sort === column()[0]}>
                {column()[1]}
              </option>
            )}
          </For>
        </select>
      </label>
      <button
        class="admin-btn-secondary admin-mobile-sort__direction"
        type="button"
        disabled={directionDisabled()}
        title={directionHint()}
        aria-label={directionHint()}
        onClick={() => {
          if (!directionDisabled()) {
            props.onSortChange([props.sort, sortDirectionToggled(props.direction)]);
          }
        }}
      >
        <span aria-hidden="true">
          {directionDisabled() ? "↕" : sortDirectionArrow(props.direction)}
        </span>
      </button>
    </div>
  );
}

/**
 * 可排序表头单元格；已排序列始终标出方向，旧调用方可以保留未排序列的标记。
 * 按钮 title 提示排序仅对当前页生效。
 */
export function SortableTh(props: {
  /** 本列的稳定列标识（与 `query/` 中该页允许集合一致）。 */
  readonly column: string;
  readonly label: string;
  /** 当前排序列；空字符串表示保持服务端原始顺序。 */
  readonly active: string;
  /** 当前排序方向（仅 `active` 为本列时有意义）。 */
  readonly direction: SortDirection;
  /** 点击后回调下一排序状态（列标识 + 方向）。 */
  readonly onSort: (value: [string, SortDirection]) => void;
  /** 附加在 `th` 上的样式类。 */
  readonly class?: string;
  /** 采用可排序表格的 ARIA 约定，在未选中的列上省略 `aria-sort`。 */
  readonly omitInactiveAriaSort?: boolean;
}): JSX.Element {
  const isActive = createValueMemo(() => props.active === props.column);
  const ariaSort = createValueMemo((): "ascending" | "descending" | "none" | undefined => {
    if (isActive()) {
      return sortDirectionAria(props.direction);
    }
    return props.omitInactiveAriaSort === true ? undefined : "none";
  });
  const hint = createValueMemo(() => {
    if (!isActive()) {
      return t("admin-sort-inactive-hint");
    }
    return props.direction === "asc"
      ? t("admin-sort-ascending-hint")
      : t("admin-sort-descending-hint");
  });
  return (
    <th scope="col" class={props.class ?? ""} aria-sort={ariaSort()}>
      <button
        type="button"
        class="admin-sort-trigger"
        title={hint()}
        onClick={() => {
          props.onSort(nextSort(props.active, props.direction, props.column));
        }}
      >
        {props.label}
        <span class="admin-sort-arrow" aria-hidden="true">
          {isActive() ? sortDirectionArrow(props.direction) : "↕"}
        </span>
      </button>
    </th>
  );
}
