// 价格历史筛选值的规范化、查询门槛与校验。
// 世界是“筛选条件”面板里的一项，只有具体世界计入筛选数；Rust 版也把表单里的世界选择计入。

import { structuralEquals } from "@auction/web-shared/reactive/equality";
import { compareText } from "@auction/web-shared/text";

import type { PriceHistoryQuery } from "../../../api/model";
import type { PublicBoardQuery, WorldSwitchOption } from "../../query";
import type {
  HistoryFilterDuplicates,
  HistoryFilterErrors,
  HistoryFilterField,
  HistoryPriceUnit,
} from "./filters/model";
import { historyNameValue, historyPlayerValue } from "./filters/validation";

const U64_MAX = 18_446_744_073_709_551_615n;
const MAX_SAFE_CREDITS = 9_007_199_254_740_991n;

function isAsciiDigit(character: string): boolean {
  const code = character.charCodeAt(0);
  return code >= 0x30 && code <= 0x39;
}

function parseUnsignedDecimal(value: string, maximum: bigint): bigint | null {
  const digits = value.startsWith("+") ? value.slice(1) : value;
  if (digits.length === 0) {
    return null;
  }
  let result = 0n;
  for (const character of digits) {
    const code = character.charCodeAt(0);
    if (code < 0x30 || code > 0x39) {
      return null;
    }
    result = result * 10n + BigInt(code - 0x30);
    if (result > maximum) {
      return null;
    }
  }
  return result;
}

function parseU32Decimal(value: string): number | null {
  const parsed = parseUnsignedDecimal(value, 4_294_967_295n);
  return parsed === null ? null : Number(parsed);
}

/** 只有合法的条件才可规范化；保留非法值供字段错误与请求门禁使用。 */
export function normalizeHistoryQuery(query: PublicBoardQuery): PublicBoardQuery {
  const normalized = { ...query };
  if (query.historyName !== null) {
    const name = historyNameValue(query.historyName);
    normalized.historyName = name.ok ? name.value : query.historyName;
  }
  // 地址栏中的金额已经用 Credits 表示，按 HTTP 的十进制表示校验。
  normalized.historyMinPrice = query.historyMinPrice === "" ? null : query.historyMinPrice;
  normalized.historyMaxPrice = query.historyMaxPrice === "" ? null : query.historyMaxPrice;
  return normalized;
}

/** 历史表单管理的地址键，恢复与写回共用。 */
export const HISTORY_FILTER_KEYS: readonly (readonly [HistoryFilterField, string])[] = [
  ["name", "name"],
  ["world", "world"],
  ["from", "from"],
  ["to", "to"],
  ["minPrice", "min_price"],
  ["maxPrice", "max_price"],
  ["buyer", "buyer"],
  ["seller", "seller"],
];

/** 历史筛选从原始地址恢复；解析器丢弃的非法值与重复键不能扩大查询。 */
export function historyQueryFromUrl(query: PublicBoardQuery, search: string) {
  const params = new URLSearchParams(search);
  const duplicates: HistoryFilterDuplicates = {};
  const fields: Partial<Record<HistoryFilterField, string>> = {};
  for (const [field, key] of HISTORY_FILTER_KEYS) {
    const values = params.getAll(key);
    if (values.length > 1) {
      duplicates[field] = values;
    }
    const first = values[0];
    if (first !== undefined && first !== "") {
      fields[field] = first;
    }
  }
  const world = fields.world === "all" ? null : (fields.world ?? null);
  const normalized = normalizeHistoryQuery({
    ...query,
    historyName: fields.name ?? null,
    historyFrom: fields.from ?? null,
    historyTo: fields.to ?? null,
    historyMinPrice: fields.minPrice ?? null,
    historyMaxPrice: fields.maxPrice ?? null,
    historyBuyer: fields.buyer ?? null,
    historySeller: fields.seller ?? null,
  });
  const restored: PriceHistoryQuery = {
    world,
    name: normalized.historyName,
    from: normalized.historyFrom,
    to: normalized.historyTo,
    minPrice: normalized.historyMinPrice,
    maxPrice: normalized.historyMaxPrice,
    buyer: normalized.historyBuyer,
    seller: normalized.historySeller,
    sort: normalized.historySort,
    page: Math.max(normalized.page, 1),
  };
  const duplicateErrors: HistoryFilterErrors = {};
  for (const [field] of HISTORY_FILTER_KEYS) {
    if (duplicates[field] !== undefined) {
      duplicateErrors[field] = "duplicate";
    }
  }
  return { query: restored, errors: historyQueryErrors(restored, duplicateErrors), duplicates };
}

/** 深链与提交共用字段校验，只有无错误的查询才能进入读取。 */
export function historyQueryErrors(
  query: PriceHistoryQuery,
  fieldErrors: HistoryFilterErrors = {},
): HistoryFilterErrors {
  const errors = { ...fieldErrors };
  if (query.world !== null && query.world !== "main" && query.world !== "isekai") {
    errors.world ??= "world_invalid";
  }
  const name = historyNameValue(query.name ?? "");
  if (!name.ok) {
    errors.name ??= name.error;
  }
  const from = query.from;
  const to = query.to;
  // 结束日转换成次日 UTC；auction-time 的 UTC 上限落在 9999-12-30，应用端口另拒绝 Unix 纪元。
  if (
    from !== null &&
    (!historyIsValidDate(from) || from === "1970-01-01" || from === "9999-12-31")
  ) {
    errors.from ??= "from_date_invalid";
  }
  if (
    to !== null &&
    (!historyIsValidDate(to) || to === "1969-12-31" || compareText(to, "9999-12-30") >= 0)
  ) {
    errors.to ??= "to_date_invalid";
  }
  if (query.minPrice !== null && historyNormalizedPrice(query.minPrice) === null) {
    errors.minPrice ??= "minimum_price_c";
  }
  if (query.maxPrice !== null && historyNormalizedPrice(query.maxPrice) === null) {
    errors.maxPrice ??= "maximum_price_c";
  }
  for (const field of ["buyer", "seller"] as const) {
    const player = historyPlayerValue(query[field] ?? "");
    if (!player.ok) {
      errors[field] ??= player.error;
    }
  }
  // 只有两端各自通过字段校验，才能用已经规范化的值比较区间。
  if (
    from !== null &&
    to !== null &&
    errors.from === undefined &&
    errors.to === undefined &&
    compareText(from, to) > 0
  ) {
    errors.from = "date_range";
  }
  if (
    errors.minPrice === undefined &&
    errors.maxPrice === undefined &&
    historyPriceRangeReversed(query.minPrice, query.maxPrice)
  ) {
    errors.minPrice = "price_range";
  }
  return errors;
}

/** 把日期拆成表单使用的年月；格式非法时在可编辑的年份栏保留原文。 */
export function historyDateParts(value: string | null): readonly [string, string] {
  if (value === null) {
    return ["", ""];
  }
  if (!historyIsValidDate(value)) {
    return ["", value];
  }
  const parts = value.split("-");
  return [parts[1] ?? "", parts[0] ?? ""];
}

/** 把规范化金额拆成带单位的价格输入；无值用 K，非法金额原样保留并使用 C。 */
export function historyPriceParts(value: string | null): readonly [string, HistoryPriceUnit] {
  if (value === null) {
    return ["", "K"];
  }
  const normalized = historyNormalizedPrice(value);
  if (normalized === null) {
    return [value, "C"];
  }
  const parsed = parseUnsignedDecimal(normalized, U64_MAX);
  if (parsed === null) {
    return ["", "K"];
  }
  const amount = Number(parsed);
  let unit: HistoryPriceUnit;
  let scale: number;
  let digits: number;
  if (amount >= 1_000_000) {
    unit = "M";
    scale = 1_000_000;
    digits = 6;
  } else if (amount >= 1_000) {
    unit = "K";
    scale = 1_000;
    digits = 3;
  } else {
    return [amount.toString(), "C"];
  }
  const whole = Math.trunc(amount / scale);
  const fraction = String(amount % scale)
    .padStart(digits, "0")
    .replace(/0+$/u, "");
  return [fraction === "" ? whole.toString() : `${whole}.${fraction}`, unit];
}

/** 已完成单值校验的金额区间按整数 Credits 比较；URL 恢复和表单提交必须使用同一规则。 */
export function historyPriceRangeReversed(minimum: string | null, maximum: string | null): boolean {
  const minimumValue = minimum === null ? null : parseUnsignedDecimal(minimum, U64_MAX);
  const maximumValue = maximum === null ? null : parseUnsignedDecimal(maximum, U64_MAX);
  return minimumValue !== null && maximumValue !== null && minimumValue > maximumValue;
}

/** 判断四位年份与两位月日的 `YYYY-MM-DD` 是否为合法公历日期。 */
export function historyIsValidDate(value: string): boolean {
  const parts = value.split("-");
  if (parts.length !== 3) {
    return false;
  }
  const year = parts[0] ?? "";
  const month = parts[1] ?? "";
  const day = parts[2] ?? "";
  if (
    year.length !== 4 ||
    month.length !== 2 ||
    day.length !== 2 ||
    ![...year, ...month, ...day].every(isAsciiDigit)
  ) {
    return false;
  }
  const yearNumber = parseU32Decimal(year);
  const monthNumber = parseU32Decimal(month);
  const dayNumber = parseU32Decimal(day);
  if (yearNumber === null || monthNumber === null || dayNumber === null) {
    return false;
  }
  return (
    yearNumber > 0 &&
    monthNumber >= 1 &&
    monthNumber <= 12 &&
    dayNumber >= 1 &&
    dayNumber <= historyDaysInMonth(yearNumber, monthNumber)
  );
}

/** 把无符号十进制金额规范成安全整数 Credits 字符串；语法非法或溢出返回 null。 */
export function historyNormalizedPrice(value: string): string | null {
  if (!/^(?:0|[1-9][0-9]*)$/u.test(value)) {
    return null;
  }
  const amount = parseUnsignedDecimal(value, U64_MAX);
  return amount !== null && amount <= MAX_SAFE_CREDITS ? amount.toString() : null;
}

/** 返回公历月份天数；二月按 4/100/400 的闰年规则判断。 */
export function historyDaysInMonth(year: number, month: number): number {
  if (month === 2) {
    return year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0) ? 29 : 28;
  }
  if (month === 4 || month === 6 || month === 9 || month === 11) {
    return 30;
  }
  return 31;
}

/** 世界缺省或显式的“全部”都不构成条件；具体世界计一项。 */
function historyWorldFilterActive(world: string | null): boolean {
  return world !== null && world !== "all";
}

/** 世界草稿是否可提交：全部世界、目录中未禁用的世界，或地址栏已经应用的世界。 */
export function historyWorldDraftValid(
  world: string,
  options: readonly WorldSwitchOption[],
  appliedWorld: string | null,
): boolean {
  return (
    world === "all" ||
    world === appliedWorld ||
    options.some((option) => option.value === world && !option.disabled)
  );
}

/** 实际采用的世界草稿：有效草稿保留，目录收窄等原因失效时回到已应用值。 */
export function historyWorldDraftForApply(
  world: string,
  options: readonly WorldSwitchOption[],
  appliedWorld: string | null,
): string {
  return historyWorldDraftValid(world, options, appliedWorld) ? world : (appliedWorld ?? "all");
}

/** 统计本页筛选表单里的已应用条件数量；具体世界与其余字段一样计一项。 */
export function historyActiveFilterCount(query: PriceHistoryQuery): number {
  const values = [
    query.name,
    query.from,
    query.to,
    query.minPrice,
    query.maxPrice,
    query.buyer,
    query.seller,
  ];
  return (
    (historyWorldFilterActive(query.world) ? 1 : 0) +
    values.filter((value) => value !== null && value !== "").length
  );
}

/** 是否具备发起历史读取的条件；非法筛选与重复键必须先修正，具体世界单独选择时也计入筛选数。 */
export function historyHasRequestCondition(query: PriceHistoryQuery): boolean {
  return historyActiveFilterCount(query) > 0;
}

/** 已应用查询中属于本页表单的部分；世界也是表单字段，须随前进后退和跨页进入同步草稿。 */
export function historyFormFilters(query: PriceHistoryQuery): PriceHistoryQuery {
  return { ...query, sort: "", page: 1 };
}

/** 本次提交是否会改变已应用状态；只比较规范化后的筛选与目标页，排序、分组和页长不属于本表单。 */
export function historyApplyChangesQuery(
  applied: PriceHistoryQuery,
  candidate: PriceHistoryQuery,
): boolean {
  return (
    applied.page !== candidate.page ||
    !structuralEquals(historyFormFilters(applied), historyFormFilters(candidate))
  );
}

/** `resetFilters` 是否确实会改变地址栏的已应用状态；页长与透传参数不由本页清除。 */
export function historyResetChangesQuery(
  restored: ReturnType<typeof historyQueryFromUrl>,
  group: string,
): boolean {
  const { query, errors } = restored;
  return (
    historyHasRequestCondition(query) ||
    Object.keys(errors).length !== 0 ||
    query.sort !== "closed_desc" ||
    group !== "flat" ||
    query.page !== 1
  );
}
