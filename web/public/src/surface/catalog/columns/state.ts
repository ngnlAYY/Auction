// 目录列的固定身份、布局权重与浏览器偏好；不改变目录查询或竞价状态。
//
// 位序属于 v1 本地偏好格式；改动已有位序时必须更换存储版本。

import { getItem, setItem } from "@auction/web-shared/platform/storage";
import { unreachable } from "@auction/web-shared/result";
import { utf8ByteLength } from "@auction/web-shared/text";

/** 目录列的稳定身份；顺序对应当前 v1 本地偏好位集，不能重排。 */
export type CatalogColumn =
  | "code"
  | "item"
  | "group"
  | "average_roll"
  | "level"
  | "attributes"
  | "seller"
  | "price"
  | "next_bid"
  | "leader"
  | "remaining"
  | "status"
  | "actions";

/** 目录的三种列布局；每种布局拥有独立的列顺序与权重。 */
export type CatalogLayout = "live" | "settled" | "preview";

/** 小型 Copy 位集跨目录模式共享；隐藏不适用于当前模式的列时仍保留其偏好。 */
export interface CatalogVisibility {
  /** 内部位集，只在 `catalogVisibility*` 自由函数之间传递。 */
  readonly bits: number;
}

const COLUMN_BITS: Readonly<Record<CatalogColumn, number>> = {
  code: 1 << 0,
  item: 1 << 1,
  group: 1 << 2,
  average_roll: 1 << 3,
  level: 1 << 4,
  attributes: 1 << 5,
  seller: 1 << 6,
  price: 1 << 7,
  next_bid: 1 << 8,
  leader: 1 << 9,
  remaining: 1 << 10,
  status: 1 << 11,
  actions: 1 << 12,
};

const LIVE_COLUMNS: readonly (readonly [CatalogColumn, number])[] = [
  ["code", 5],
  // Item 拿到可容纳常见装备全名的宽度；开拍后新增的 Actions 列不能挤掉它。
  ["item", 27],
  ["group", 4],
  ["average_roll", 9],
  ["level", 5],
  ["attributes", 4],
  ["seller", 7],
  ["price", 9],
  ["next_bid", 8],
  ["leader", 8],
  ["remaining", 7],
  ["status", 7],
  ["actions", 9],
];

const SETTLED_COLUMNS: readonly (readonly [CatalogColumn, number])[] = [
  ["code", 7],
  ["item", 25],
  ["group", 5],
  ["average_roll", 6],
  ["level", 5],
  ["attributes", 5],
  ["seller", 8],
  ["price", 10],
  ["next_bid", 9],
  ["leader", 8],
  ["remaining", 6],
  ["status", 6],
  // 结算后操作列只剩查看入口，只需一个图标的宽度。
  ["actions", 6],
];

const PREVIEW_COLUMNS: readonly (readonly [CatalogColumn, number])[] = [
  ["code", 5],
  ["item", 27],
  ["group", 5],
  ["average_roll", 9],
  ["level", 5],
  ["attributes", 6],
  ["seller", 11],
  ["price", 12],
  ["leader", 6],
  ["remaining", 7],
  ["status", 7],
  ["actions", 6],
];

/** 表头顺序与原有列权重的唯一来源；TableColumns 只归一化当前可见项。 */
export function catalogLayoutColumns(
  layout: CatalogLayout,
): readonly (readonly [CatalogColumn, number])[] {
  switch (layout) {
    case "live":
      return LIVE_COLUMNS;
    case "settled":
      return SETTLED_COLUMNS;
    case "preview":
      return PREVIEW_COLUMNS;
    default:
      return unreachable(layout);
  }
}

function catalogColumnBit(column: CatalogColumn): number {
  return COLUMN_BITS[column];
}

function columnMask(columns: readonly (readonly [CatalogColumn, number])[]): number {
  let mask = 0;
  for (const [column] of columns) {
    mask |= catalogColumnBit(column);
  }
  return mask;
}

const ALL_COLUMNS = columnMask(LIVE_COLUMNS);
const LAYOUT_MASKS: Readonly<Record<CatalogLayout, number>> = {
  live: ALL_COLUMNS,
  settled: columnMask(SETTLED_COLUMNS),
  preview: columnMask(PREVIEW_COLUMNS),
};

function catalogLayoutMask(layout: CatalogLayout): number {
  return LAYOUT_MASKS[layout];
}

const STORAGE_KEY = "hv.public.catalog-columns.v1";

/** 返回默认可见列；装备分组可由筛选器恢复，因此默认不占目录列宽。 */
export function catalogVisibilityDefault(): CatalogVisibility {
  // 装备分组可从筛选器和列设置恢复；默认不占目录列宽，给价格/剩余时间/状态留出空间。
  return { bits: ALL_COLUMNS & ~catalogColumnBit("group") };
}

/** 读取本地列偏好；存储缺失或内容损坏时回到默认可见列。 */
export function catalogVisibilityLoad(): CatalogVisibility {
  const stored = getItem(STORAGE_KEY);
  return stored === null
    ? catalogVisibilityDefault()
    : (catalogVisibilityFromStored(stored) ?? catalogVisibilityDefault());
}

/** 严格接受本版本写出的非零十进制位集；空白、未知位和全隐藏均视为损坏。 */
export function catalogVisibilityFromStored(value: string): CatalogVisibility | null {
  if (
    value.length === 0 ||
    utf8ByteLength(value) > 4 ||
    value.startsWith("0") ||
    !asciiDigits(value)
  ) {
    return null;
  }
  const bits = Number(value);
  return (bits & ~ALL_COLUMNS) === 0 ? { bits } : null;
}

function asciiDigits(value: string): boolean {
  for (let index = 0; index < value.length; index += 1) {
    const code = value.charCodeAt(index);
    if (code < 0x30 || code > 0x39) {
      return false;
    }
  }
  return true;
}

/** 模式切换不得出现空表；只在派生视图临时补上装备列，不覆盖已保存偏好。 */
export function catalogVisibilityForLayout(
  visibility: CatalogVisibility,
  layout: CatalogLayout,
): CatalogVisibility {
  if (catalogVisibilityVisibleCount(visibility, layout) === 0) {
    return catalogVisibilityWithBits(visibility.bits | catalogColumnBit("item"), visibility);
  }
  return visibility;
}

/** 判断列是否在给定位集中可见；按布局归一化由调用方负责。 */
export function catalogVisibilityContains(
  visibility: CatalogVisibility,
  column: CatalogColumn,
): boolean {
  return (visibility.bits & catalogColumnBit(column)) !== 0;
}

/** 按布局顺序返回当前可见列的宽度；空布局会临时补上装备列。 */
export function catalogVisibilityWidths(
  visibility: CatalogVisibility,
  layout: CatalogLayout,
): number[] {
  const visible = catalogVisibilityForLayout(visibility, layout);
  const widths: number[] = [];
  for (const [column, width] of catalogLayoutColumns(layout)) {
    if (catalogVisibilityContains(visible, column)) {
      widths.push(width);
    }
  }
  return widths;
}

/** 统计当前布局内的可见列数；其他布局保留的偏好位不计入。 */
export function catalogVisibilityVisibleCount(
  visibility: CatalogVisibility,
  layout: CatalogLayout,
): number {
  let bits = visibility.bits & catalogLayoutMask(layout);
  let count = 0;
  while (bits !== 0) {
    bits &= bits - 1;
    count += 1;
  }
  return count;
}

/** 原生 disabled 之外再守住最后一列；用户明确修改时才把有效布局写回偏好。 */
export function catalogVisibilityWithColumn(
  visibility: CatalogVisibility,
  layout: CatalogLayout,
  column: CatalogColumn,
  shown: boolean,
): CatalogVisibility {
  const current = catalogVisibilityForLayout(visibility, layout);
  const bit = catalogColumnBit(column);
  if (
    (catalogLayoutMask(layout) & bit) === 0 ||
    catalogVisibilityContains(current, column) === shown ||
    (!shown && catalogVisibilityVisibleCount(current, layout) === 1)
  ) {
    return visibility;
  }
  return catalogVisibilityWithBits(shown ? current.bits | bit : current.bits & ~bit, visibility);
}

/** “显示全部列”只打开当前布局包含的字段；其他布局的偏好原样保留。 */
export function catalogVisibilityWithAllColumns(
  visibility: CatalogVisibility,
  layout: CatalogLayout,
): CatalogVisibility {
  return catalogVisibilityWithBits(visibility.bits | catalogLayoutMask(layout), visibility);
}

/** “全选”取消时只保留装备列（目录至少保留一列）；其他布局专属列的偏好原样保留。 */
export function catalogVisibilityWithOnlyItem(
  visibility: CatalogVisibility,
  layout: CatalogLayout,
): CatalogVisibility {
  return catalogVisibilityWithBits(
    (visibility.bits & ~catalogLayoutMask(layout)) | catalogColumnBit("item"),
    visibility,
  );
}

/** 存储受限或写入失败时仍保留本次 Signal 修改，不影响当前页面使用。 */
export function catalogVisibilitySave(visibility: CatalogVisibility): void {
  // Rust 忽略写入结果；偏好未能持久化时当前页面仍使用内存中的位集。
  setItem(STORAGE_KEY, visibility.bits.toString());
}

/** 位集相同时复用原值，让调用方的引用比较等价于 Rust 的 `PartialEq`。 */
function catalogVisibilityWithBits(bits: number, original: CatalogVisibility): CatalogVisibility {
  return bits === original.bits ? original : { bits };
}
