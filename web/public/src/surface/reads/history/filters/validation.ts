// 价格历史筛选器的错误文案、日期边界和金额规范化。

import { err, ok, type Result, unreachable } from "@auction/web-shared/result";
import { utf8ByteLength } from "@auction/web-shared/text";
import { t } from "@auction/web-shared/ui/i18n";

import { historyDateParts, historyDaysInMonth, historyIsValidDate } from "../values";
import type {
  HistoryDateError,
  HistoryFilterError,
  HistoryPriceError,
  HistoryPriceUnit,
} from "./model";

/** 名称查询的词数上限，与 HTTP 层的 `MAX_PUBLIC_HISTORY_NAME_TERMS` 一致。 */
const MAX_NAME_TERMS = 8;

const U64_MAX = 18_446_744_073_709_551_615n;
const MAX_SAFE_CREDITS = 9_007_199_254_740_991n;

/** Rust `str::trim` 的 Unicode 空白集合；JS `trim()` 的集合不同。 */
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

/** 按逗号或 Rust 的 Unicode 空白拆词，并丢弃空片段。 */
function* nameTerms(value: string): Generator<string> {
  let term = "";
  for (const character of value) {
    if (character === "," || isRustWhitespace(character.charCodeAt(0))) {
      if (term !== "") {
        yield term;
      }
      term = "";
    } else {
      term += character;
    }
  }
  if (term !== "") {
    yield term;
  }
}

/** 解析 Rust 无符号十进制整数语法；接收可选 `+`，拒绝负号、空白与溢出。 */
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

function hasControlCharacter(value: string): boolean {
  return /\p{Cc}/u.test(value);
}

/** 名称字段复用 HTTP 的空白/逗号拆词和应用端口字节边界；缩写片段均须命中装备名。 */
export function historyNameValue(value: string): Result<string | null, HistoryFilterError> {
  const trimmed = rustTrim(value);
  if (utf8ByteLength(trimmed) > 1_024) {
    return err("name_too_long");
  }
  let termCount = 0;
  let termTooLong = false;
  let invalid = false;
  for (const term of nameTerms(trimmed)) {
    termCount += 1;
    if (termCount > MAX_NAME_TERMS) {
      return err("name_too_many_terms");
    }
    termTooLong ||= utf8ByteLength(term) > 128;
    invalid ||= !termTooLong && hasControlCharacter(term);
  }
  // 保留总长、词数、单词长度、控制字符的既有错误优先级，不为校验建立词数组。
  if (termTooLong) {
    return err("name_term_too_long");
  }
  if (invalid) {
    return err("name_invalid");
  }
  // 只有分隔符的输入与服务端的空词集合一致，不把它当成有效条件加载全部历史。
  return ok(termCount !== 0 ? trimmed : null);
}

/** 把完整装备名转成价格历史的名称查询。服务端只做“各词都包含”的匹配，没有精确等于，
 * 因此结果可能包含名称更长的同类装备，调用方需自行标出不同名的行。
 * 超出词数上限时保留最长的词（最具区分度）并维持原顺序；没有可用词条或名称
 * 不满足查询约束时返回 `null`，调用方不发请求。 */
export function historyEquipmentNameQuery(equipmentName: string): string | null {
  const terms = Array.from(nameTerms(equipmentName));
  let query: string;
  if (terms.length <= MAX_NAME_TERMS) {
    query = terms.join(" ");
  } else {
    const longest = terms.map((term, index) => ({ index, length: utf8ByteLength(term) }));
    // 稳定排序让等长的词保留靠前者，同一名称始终得到同一查询。
    longest.sort((left, right) => right.length - left.length || left.index - right.index);
    const retained = longest.slice(0, MAX_NAME_TERMS).map((term) => term.index);
    retained.sort((left, right) => left - right);
    query = retained.map((index) => terms[index] ?? "").join(" ");
  }
  const normalized = historyNameValue(query);
  return normalized.ok ? normalized.value : null;
}

/** 将名称字段的稳定错误分类转换为本 surface 的文案。 */
export function historyNameError(error: HistoryFilterError | null): string | null {
  if (error === null) {
    return null;
  }
  switch (error) {
    case "name_too_many_terms":
      return t("history-filter-error-name-terms");
    case "name_term_too_long":
      return t("history-filter-error-name-term-length");
    case "name_too_long":
      return t("history-filter-error-name-length");
    case "name_invalid":
      return t("history-filter-error-name-invalid");
    case "from_date_incomplete":
    case "from_date_invalid":
    case "to_date_incomplete":
    case "to_date_invalid":
    case "date_range":
    case "minimum_price_c":
    case "minimum_price_k":
    case "minimum_price_m":
    case "maximum_price_c":
    case "maximum_price_k":
    case "maximum_price_m":
    case "price_range":
      return null;
    default:
      return unreachable(error);
  }
}

/** 将页面错误分类转换为对应的本地化日期错误。 */
export function historyDateError(error: HistoryFilterError | null, from: boolean): string | null {
  if (error === null) {
    return null;
  }
  switch (error) {
    case "from_date_incomplete":
      return from ? t("history-filter-error-date-incomplete") : null;
    case "to_date_incomplete":
      return from ? null : t("history-filter-error-date-incomplete");
    case "from_date_invalid":
      return from ? t("history-filter-error-date-invalid") : null;
    case "to_date_invalid":
      return from ? null : t("history-filter-error-date-invalid");
    case "date_range":
      return from ? t("history-filter-error-date-range") : null;
    case "name_too_many_terms":
    case "name_term_too_long":
    case "name_too_long":
    case "name_invalid":
    case "minimum_price_c":
    case "minimum_price_k":
    case "minimum_price_m":
    case "maximum_price_c":
    case "maximum_price_k":
    case "maximum_price_m":
    case "price_range":
      return null;
    default:
      return unreachable(error);
  }
}

/** 将页面错误分类转换为对应的本地化金额错误。 */
export function historyPriceError(
  error: HistoryFilterError | null,
  minimum: boolean,
): string | null {
  if (error === null) {
    return null;
  }
  if (minimum) {
    switch (error) {
      case "minimum_price_c":
        return t("history-filter-error-price-c");
      case "minimum_price_k":
        return t("history-filter-error-price-k");
      case "minimum_price_m":
        return t("history-filter-error-price-m");
      case "price_range":
        return t("history-filter-error-price-range");
      case "name_too_many_terms":
      case "name_term_too_long":
      case "name_too_long":
      case "name_invalid":
      case "from_date_incomplete":
      case "from_date_invalid":
      case "to_date_incomplete":
      case "to_date_invalid":
      case "date_range":
      case "maximum_price_c":
      case "maximum_price_k":
      case "maximum_price_m":
        return null;
      default:
        return unreachable(error);
    }
  }
  switch (error) {
    case "maximum_price_c":
      return t("history-filter-error-price-c");
    case "maximum_price_k":
      return t("history-filter-error-price-k");
    case "maximum_price_m":
      return t("history-filter-error-price-m");
    case "name_too_many_terms":
    case "name_term_too_long":
    case "name_too_long":
    case "name_invalid":
    case "from_date_incomplete":
    case "from_date_invalid":
    case "to_date_incomplete":
    case "to_date_invalid":
    case "date_range":
    case "minimum_price_c":
    case "minimum_price_k":
    case "minimum_price_m":
    case "price_range":
      return null;
    default:
      return unreachable(error);
  }
}

/** 返回筛选器使用的本地化月份名称。 */
export function historyMonthLabel(month: number): string {
  switch (month) {
    case 1:
      return t("history-filter-month-01");
    case 2:
      return t("history-filter-month-02");
    case 3:
      return t("history-filter-month-03");
    case 4:
      return t("history-filter-month-04");
    case 5:
      return t("history-filter-month-05");
    case 6:
      return t("history-filter-month-06");
    case 7:
      return t("history-filter-month-07");
    case 8:
      return t("history-filter-month-08");
    case 9:
      return t("history-filter-month-09");
    case 10:
      return t("history-filter-month-10");
    case 11:
      return t("history-filter-month-11");
    case 12:
      return t("history-filter-month-12");
    default:
      return month.toString();
  }
}

/** 年月未改变时保留当前 URL 的精确日期；改变年月才生成整月的闭区间端点。 */
export function historyDateValue(
  month: string,
  year: string,
  end: boolean,
  applied: string | null,
): Result<string | null, HistoryDateError> {
  const trimmedMonth = rustTrim(month);
  const trimmedYear = rustTrim(year);
  if (trimmedMonth === "" && trimmedYear === "") {
    return ok(null);
  }
  if (trimmedMonth === "" || trimmedYear === "") {
    return err("incomplete");
  }
  if (trimmedYear.length !== 4 || ![...trimmedYear].every(isAsciiDigit)) {
    return err("invalid");
  }
  const yearNumber = parseU32Decimal(trimmedYear);
  const monthNumber = parseU32Decimal(trimmedMonth);
  if (
    yearNumber === null ||
    monthNumber === null ||
    yearNumber === 0 ||
    monthNumber < 1 ||
    monthNumber > 12
  ) {
    return err("invalid");
  }
  if (applied !== null && historyIsValidDate(applied)) {
    const [appliedMonth, appliedYear] = historyDateParts(applied);
    if (
      parseU32Decimal(appliedMonth) === monthNumber &&
      parseU32Decimal(appliedYear) === yearNumber
    ) {
      return ok(applied);
    }
  }
  const day = end ? historyDaysInMonth(yearNumber, monthNumber) : 1;
  return ok(
    `${yearNumber.toString().padStart(4, "0")}-${monthNumber.toString().padStart(2, "0")}-${day.toString().padStart(2, "0")}`,
  );
}

/** 将带单位的页面金额输入规范化为 API 使用的整数 Credits。 */
export function historyPriceValue(
  value: string,
  unit: HistoryPriceUnit,
): Result<string | null, HistoryPriceError> {
  const trimmed = rustTrim(value);
  if (trimmed === "") {
    return ok(null);
  }
  const precision = unit === "C" ? 0 : unit === "K" ? 3 : 6;
  let whole: string;
  let fraction: string;
  if (unit === "C") {
    if (![...trimmed].every(isAsciiDigit)) {
      return err("invalid");
    }
    whole = trimmed;
    fraction = "";
  } else {
    const parts = trimmed.split(".");
    const candidateWhole = parts[0] ?? "";
    const candidateFraction = parts[1] ?? "";
    if (
      parts.length > 2 ||
      candidateWhole === "" ||
      ![...candidateWhole].every(isAsciiDigit) ||
      (candidateFraction === "" && trimmed.includes(".")) ||
      candidateFraction.length > precision ||
      ![...candidateFraction].every(isAsciiDigit)
    ) {
      return err("invalid");
    }
    whole = candidateWhole;
    fraction = candidateFraction;
  }
  const wholeValue = parseUnsignedDecimal(whole, U64_MAX);
  if (wholeValue === null) {
    return err("invalid");
  }
  // 仅对整数补足单位小数位；全程精确整数运算，不能用浮点或截断非整千 Credits。
  const scale = 10n ** BigInt(precision);
  const base = wholeValue * scale;
  if (base > U64_MAX) {
    return err("invalid");
  }
  const fractionValue = fraction === "" ? 0n : BigInt(fraction);
  const fractionScale = 10n ** BigInt(precision - fraction.length);
  const fractional = fractionValue * fractionScale;
  if (fractional > U64_MAX) {
    return err("invalid");
  }
  const credits = base + fractional;
  if (credits > U64_MAX || credits > MAX_SAFE_CREDITS) {
    return err("invalid");
  }
  return ok(credits.toString());
}

function isAsciiDigit(character: string): boolean {
  const code = character.charCodeAt(0);
  return code >= 0x30 && code <= 0x39;
}
