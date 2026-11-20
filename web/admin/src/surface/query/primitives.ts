// 各列表页共用的 query 原语：取值、页码、闭集枚举、自由文本、排序、轮询间隔与 UUID 校验。
// 校验失败的地址栏值一律回退到「未设置」，不把畸形文本转发给 API。

import type { QueryPair } from "@auction/web-shared/net/query";
import { utf8ByteLength } from "@auction/web-shared/text";
import { validUuid } from "@auction/web-shared/ui/validation";

import { POLL_INTERVAL_VALUES } from "../poll";
import { type SortDirection, sortDirectionAsQueryValue, sortDirectionParse } from "../sort";

const I64_MAX_TEXT = "9223372036854775807";
const I64_MIN_MAGNITUDE_TEXT = "9223372036854775808";
const U64_MAX_TEXT = "18446744073709551615";

/** 取某个键的首个值。 */
export function pairValue(pairs: readonly QueryPair[], key: string): string | null {
  for (const [pairKey, value] of pairs) {
    if (pairKey === key) {
      return value;
    }
  }
  return null;
}

/** 解析正整数页码；缺失、零、负数、超出 `i64` 或非法字符串都回退第一页。
 * 返回 `bigint` 以保留地址栏中的完整 `i64` 值；是否可请求由各端点的偏移上限决定。 */
export function parsePage(pairs: readonly QueryPair[], key: string): bigint {
  const raw = pairValue(pairs, key);
  const parsed = raw === null ? null : parseI64(raw);
  return parsed !== null && parsed > 0n ? parsed : 1n;
}

/** 页码大于 1 时才写入地址栏；`bigint` 按十进制原样写回，不丢失大页码。 */
export function pushPage(pairs: QueryPair[], key: string, page: bigint): void {
  if (page > 1n) {
    pairs.push([key, page.toString()]);
  }
}

/** 解析闭集枚举值；不在允许集合内（含缺省）时回退空串表示「全部」。 */
export function parseChoice(
  pairs: readonly QueryPair[],
  key: string,
  allowed: readonly string[],
): string {
  const value = pairValue(pairs, key);
  return value !== null && allowed.includes(value) ? value : "";
}

/**
 * 解析自由文本筛选：trim、长度上限（与对应输入框 maxlength 对齐）、剔除控制字符；
 * 不满足任一条件视为未设置。
 */
export function parseText(pairs: readonly QueryPair[], key: string, maximum: number): string {
  const raw = pairValue(pairs, key);
  if (raw === null) {
    return "";
  }
  const value = rustTrim(raw);
  if (value === "" || utf8ByteLength(value) > maximum || hasControlCharacter(value)) {
    return "";
  }
  return value;
}

/** 文本筛选非空时才写入地址栏。 */
export function pushText(pairs: QueryPair[], key: string, value: string): void {
  if (value !== "") {
    pairs.push([key, value]);
  }
}

/** 解析排序列：不在该页允许集合内（含缺省）回退空串，表示保持服务端原始顺序。 */
export function parseSortColumn(pairs: readonly QueryPair[], allowed: readonly string[]): string {
  return parseChoice(pairs, "sort", allowed);
}

/** 解析排序方向；只有排序列存在时方向才有意义，非法值回退升序。 */
export function parseSortDirection(pairs: readonly QueryPair[]): SortDirection {
  return sortDirectionParse(pairValue(pairs, "dir"));
}

/** 解析自动轮询间隔：合法值为 0（关闭）与 10/30/60 秒；缺省回退调用方默认值。 */
export function parsePollSecondsWithDefault(
  pairs: readonly QueryPair[],
  defaultSeconds: number,
): number {
  const raw = pairValue(pairs, "poll");
  if (raw === null) {
    return defaultSeconds;
  }
  const seconds = parseU64(raw);
  if (seconds === null) {
    return defaultSeconds;
  }
  return seconds === 0n || POLL_INTERVAL_VALUES.some((value) => BigInt(value) === seconds)
    ? Number(seconds)
    : defaultSeconds;
}

/** 一般列表页默认关闭自动轮询。 */
export function parsePollSeconds(pairs: readonly QueryPair[]): number {
  return parsePollSecondsWithDefault(pairs, 0);
}

/** 轮询开启（间隔非 0）时才写入地址栏。 */
export function pushPollSeconds(pairs: QueryPair[], seconds: number): void {
  if (seconds > 0) {
    pairs.push(["poll", seconds.toString()]);
  }
}

/** 排序列非空时才写入地址栏；方向仅降序（非默认）时写入 `dir=desc`。 */
export function pushSort(pairs: QueryPair[], sort: string, direction: SortDirection): void {
  if (sort === "") {
    return;
  }
  pairs.push(["sort", sort]);
  const directionValue = sortDirectionAsQueryValue(direction);
  if (directionValue !== null) {
    pairs.push(["dir", directionValue]);
  }
}

/** 只接受规范 UUID；长度与字符集不符时按未设置处理，避免用地址栏文本扫描。 */
export function parseUuidText(pairs: readonly QueryPair[], key: string): string {
  const value = parseText(pairs, key, 36);
  return validUuid(value) ? value : "";
}

/** 解析 `u64` 的十进制文本；越界或非法时返回 `null`，调用方走缺省值。
 * 先按字符与位数判定上界，超长输入不会构造任意精度整数。 */
function parseU64(value: string): bigint | null {
  const parts = scanDecimal(value);
  if (parts === null || parts.negative) {
    return null;
  }
  const significant = value.length - parts.firstDigit;
  if (significant === 0) {
    return 0n;
  }
  if (significant > 20) {
    return null;
  }
  const magnitude = value.slice(parts.firstDigit);
  if (magnitude.length === 20 && magnitude > U64_MAX_TEXT) {
    return null;
  }
  return BigInt(magnitude);
}

/** 解析 `i64` 的十进制文本；语法或范围不合法时返回 `null`。
 * 与 Rust 的 `i64::from_str` 一致：允许前导 `+`/`-` 与前导零，位数超限先于转换判定。 */
function parseI64(value: string): bigint | null {
  const parts = scanDecimal(value);
  if (parts === null) {
    return null;
  }
  const significant = value.length - parts.firstDigit;
  if (significant === 0) {
    return 0n;
  }
  if (significant > 19) {
    return null;
  }
  const magnitude = value.slice(parts.firstDigit);
  const limit = parts.negative ? I64_MIN_MAGNITUDE_TEXT : I64_MAX_TEXT;
  if (magnitude.length === 19 && magnitude > limit) {
    return null;
  }
  const parsed = BigInt(magnitude);
  return parts.negative ? -parsed : parsed;
}

interface DecimalParts {
  /** 带负号；`u64` 解析据此拒绝负数。 */
  readonly negative: boolean;
  /** 首个非零数字的下标；全零时等于字符串长度。 */
  readonly firstDigit: number;
}

/** 校验十进制整数文本并定位有效数字；只允许可选的 `+`/`-` 与 ASCII 数字。 */
function scanDecimal(value: string): DecimalParts | null {
  if (value.length === 0) {
    return null;
  }
  let index = 0;
  let negative = false;
  const sign = value.charCodeAt(0);
  if (sign === 0x2b || sign === 0x2d) {
    negative = sign === 0x2d;
    index = 1;
  }
  if (index === value.length) {
    return null;
  }
  for (let position = index; position < value.length; position += 1) {
    const code = value.charCodeAt(position);
    if (code < 0x30 || code > 0x39) {
      return null;
    }
  }
  while (index < value.length && value.charCodeAt(index) === 0x30) {
    index += 1;
  }
  return { negative, firstDigit: index };
}

/** Rust `char::is_control` 对应 Unicode Cc：C0、DEL 与 C1。 */
function hasControlCharacter(value: string): boolean {
  for (const character of value) {
    const codePoint = character.codePointAt(0) ?? 0;
    if (codePoint <= 0x1f || (codePoint >= 0x7f && codePoint <= 0x9f)) {
      return true;
    }
  }
  return false;
}

/** Rust `str::trim` 的 Unicode White_Space 集合；不能改用会接受 FEFF 的 JS `trim`。 */
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

function isRustWhitespace(code: number): boolean {
  return (
    (code >= 0x09 && code <= 0x0d) ||
    code === 0x20 ||
    code === 0x85 ||
    code === 0xa0 ||
    code === 0x1680 ||
    (code >= 0x2000 && code <= 0x200a) ||
    code === 0x2028 ||
    code === 0x2029 ||
    code === 0x202f ||
    code === 0x205f ||
    code === 0x3000
  );
}
