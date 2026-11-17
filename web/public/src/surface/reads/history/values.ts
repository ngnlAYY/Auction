// 价格历史筛选值的规范化与校验。

import { compareText } from "@auction/web-shared/text";

import type { PriceHistoryQuery } from "../../../api/model";
import type { PublicBoardQuery } from "../../query";
import type { HistoryFilterError, HistoryPriceUnit } from "./filters/model";
import { historyNameValue } from "./filters/validation";

const U64_MAX = 18_446_744_073_709_551_615n;
const MAX_SAFE_CREDITS = 9_007_199_254_740_991n;

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

function rustTrim(value: string): string {
  let start = 0;
  let end = value.length;
  while (start < end && isRustWhitespace(value.charCodeAt(start))) {
    start += 1;
  }
  while (end > start && isRustWhitespace(value.charCodeAt(end - 1))) {
    end -= 1;
  }
  return value.slice(start, end);
}

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

/** 归一化地址栏恢复的筛选条件；非法名称与金额保留，非法日期与倒置区间丢弃。 */
export function normalizeHistoryQuery(query: PublicBoardQuery): PublicBoardQuery {
  const normalized = { ...query };
  if (normalized.historyName !== null) {
    const name = historyNameValue(normalized.historyName);
    // 非法条件保留给字段错误与请求门禁，不能悄悄丢弃后扩大为无此条件的查询。
    normalized.historyName = name.ok ? name.value : normalized.historyName;
  }
  normalized.historyFrom =
    normalized.historyFrom === null || !historyIsValidDate(normalized.historyFrom)
      ? null
      : normalized.historyFrom;
  normalized.historyTo =
    normalized.historyTo === null || !historyIsValidDate(normalized.historyTo)
      ? null
      : normalized.historyTo;
  // 非法金额与倒置区间保留为可修正字段；丢弃条件会扩大查询范围。
  if (normalized.historyMinPrice !== null) {
    normalized.historyMinPrice =
      historyNormalizedPrice(normalized.historyMinPrice) ?? normalized.historyMinPrice;
  }
  if (normalized.historyMaxPrice !== null) {
    normalized.historyMaxPrice =
      historyNormalizedPrice(normalized.historyMaxPrice) ?? normalized.historyMaxPrice;
  }
  if (
    normalized.historyFrom !== null &&
    normalized.historyTo !== null &&
    compareText(normalized.historyFrom, normalized.historyTo) > 0
  ) {
    normalized.historyFrom = null;
    normalized.historyTo = null;
  }
  return normalized;
}

/** URL 名称与金额采用同一字段校验并阻止无效请求；日期沿用既有恢复规则。 */
export function historyQueryError(query: PriceHistoryQuery): HistoryFilterError | null {
  const name = historyNameValue(query.name ?? "");
  if (!name.ok) {
    return name.error;
  }
  if (query.minPrice !== null && historyNormalizedPrice(query.minPrice) === null) {
    return "minimum_price_c";
  }
  if (query.maxPrice !== null && historyNormalizedPrice(query.maxPrice) === null) {
    return "maximum_price_c";
  }
  return historyPriceRangeReversed(query.minPrice, query.maxPrice) ? "price_range" : null;
}

/** 把合法日期拆成表单使用的月份和四位年份；无值或非法日期返回空对。 */
export function historyDateParts(value: string | null): readonly [string, string] {
  if (value === null || !historyIsValidDate(value)) {
    return ["", ""];
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

/** 统计已应用筛选条件数量；世界为 all 或空值不计。 */
export function historyActiveFilterCount(query: PriceHistoryQuery): number {
  const values = [
    query.world === null || query.world === "all" ? null : query.world,
    query.name,
    query.from,
    query.to,
    query.minPrice,
    query.maxPrice,
    query.buyer,
    query.seller,
  ];
  return values.filter((value) => value !== null && value !== "").length;
}

/** 去除 Rust 语义空白后返回非空文本；空文本返回 null。 */
export function optionalFilterValue(value: string): string | null {
  const trimmed = rustTrim(value);
  return trimmed !== "" ? trimmed : null;
}
