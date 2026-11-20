// Admin 浏览器端的 wire 校验助手：字段存在性、稳定 code、文本/时间边界、原文数值边界与安全整数上限。
// 这里的判断全部按 fail-closed 处理；页面只消费后端安全投影，不借此放宽服务端规则。

import { JSON_MAX_SAFE_INTEGER } from "@auction/web-shared/net/wire";
import { err, ok, type Result } from "@auction/web-shared/result";
import { utf8ByteLength } from "@auction/web-shared/text";

/** JSON 能无损表达的最大整数；所有从 wire 读取的计数、金额与版本都不得超过它。 */
export const MAX_JSON_SAFE_INTEGER: number = JSON_MAX_SAFE_INTEGER;

const U64_MAX = 18_446_744_073_709_551_615n;
const I32_MAX = 2_147_483_647;
const I32_MIN = -2_147_483_648;
// serde_json 默认的非 float_roundtrip 路径用逐次十进制幂换算；这里保留同一张表，
// 避免 `10 ** n` 与十进制字面量的最后一位舍入不同。
const SERDE_POW10 = Array.from({ length: 309 }, (_, exponent) => Number(`1e${exponent}`));
const JSON_VALUE_TOKEN = /"(?:[^"\\]|\\.)*"|-?(?:0|[1-9][0-9]*)(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?/gu;

/**
 * 原文中每个 JSON 数值都要能通过 serde_json 默认的 f64 解析；字符串里的数字不参与。
 * 共享 `parseJson` 只检查 JS Number 是否有限，而 serde_json 的非 float_roundtrip 路径
 * 会因逐次十进制幂换算提前溢出；错误回执必须按同一拒绝边界降级。
 */
export function serdeJsonNumbersFitF64(text: string): boolean {
  for (const match of text.matchAll(JSON_VALUE_TOKEN)) {
    const token = match[0];
    if (token.charCodeAt(0) !== 0x22 && !serdeJsonNumberFitsF64(token)) {
      return false;
    }
  }
  return true;
}

/** 复现 serde_json 的 `parse_integer`/`parse_decimal`/`parse_exponent` 的溢出判定。 */
function serdeJsonNumberFitsF64(raw: string): boolean {
  let index = raw.startsWith("-") ? 1 : 0;
  let significand = 0n;
  let exponent = 0;
  let integerOverflowed = false;
  for (; index < raw.length && isAsciiDigit(raw.charCodeAt(index)); index += 1) {
    if (!integerOverflowed) {
      const next = significand * 10n + BigInt(raw.charCodeAt(index) - 0x30);
      if (next > U64_MAX) {
        // 整数部分溢出后，serde 忽略剩余整数位并把位数折回十进制指数。
        integerOverflowed = true;
      }
      if (!integerOverflowed) {
        significand = next;
      }
    }
    if (integerOverflowed) {
      exponent += 1;
    }
  }
  let hasDecimal = false;
  let fractionOverflowed = false;
  if (raw[index] === ".") {
    hasDecimal = true;
    index += 1;
    while (index < raw.length && isAsciiDigit(raw.charCodeAt(index))) {
      if (!fractionOverflowed) {
        const next = significand * 10n + BigInt(raw.charCodeAt(index) - 0x30);
        if (next > U64_MAX) {
          fractionOverflowed = true;
        } else {
          significand = next;
          exponent -= 1;
        }
      }
      index += 1;
    }
  }
  if (raw[index] === "e" || raw[index] === "E") {
    return serdeExponentFitsF64(raw.slice(index + 1), significand, exponent);
  }
  if (!hasDecimal && !integerOverflowed) {
    // 纯整数在 u64/i64 内时 serde 直接保留为整数，不经过 f64。
    return true;
  }
  return serdeF64Fits(significand, exponent);
}

function serdeExponentFitsF64(
  text: string,
  significand: bigint,
  startingExponent: number,
): boolean {
  let index = 0;
  let positive = true;
  if (text[index] === "+") {
    index += 1;
  } else if (text[index] === "-") {
    positive = false;
    index += 1;
  }
  let exponent = 0;
  for (; index < text.length; index += 1) {
    const digit = text.charCodeAt(index) - 0x30;
    if (exponent > Math.trunc((I32_MAX - digit) / 10)) {
      // serde 在指数越过 i32 时忽略剩余位数：只有非零尾数配正指数才溢出。
      return significand === 0n || !positive;
    }
    exponent = exponent * 10 + digit;
  }
  const finalExponent = positive
    ? Math.min(I32_MAX, startingExponent + exponent)
    : Math.max(I32_MIN, startingExponent - exponent);
  return serdeF64Fits(significand, finalExponent);
}

function serdeF64Fits(significand: bigint, exponent: number): boolean {
  let value = Number(significand);
  let power = exponent;
  while (true) {
    const magnitude = Math.abs(power);
    if (magnitude <= 308) {
      const pow = SERDE_POW10[magnitude];
      if (pow === undefined) {
        return false;
      }
      if (power >= 0) {
        value *= pow;
        return Number.isFinite(value);
      }
      value /= pow;
      return true;
    }
    if (value === 0) {
      return true;
    }
    if (power >= 0) {
      return false;
    }
    value /= 1e308;
    power += 308;
  }
}

function isAsciiDigit(code: number): boolean {
  return code >= 0x30 && code <= 0x39;
}

/** 成功 envelope 的稳定 `code` 闭集；投递路由与论坛同步状态共用。 */
export function validSuccessCode(code: string): boolean {
  return code === "ok" || code === "created" || code === "accepted";
}

/**
 * 表示可以缺失、但一旦出现就不能是 JSON `null` 的旧 wire 字段。
 * Rust 用包装类型在 serde 解码时区分两者；TS 的同一语义由 `@auction/contracts/wire` 的
 * `optionalField` 在 schema 里表达，解码结果以 `null` 表示缺省。
 */
export type OptionalField<T> = T | null;

/** 与 `auction_time::Timestamp` 的 UTC 边界一致，供墙上时间文本的取值范围检查使用。 */
const MIN_TIMESTAMP_SECONDS = -377_705_023_201;
const MAX_TIMESTAMP_SECONDS = 253_402_207_200;

/** 本地日期时间文本追加 `Z` 后的严格 RFC 3339 形状；秒必须出现。 */
const WALL_TIMESTAMP_PATTERN =
  /^([0-9]{4})-([0-9]{2})-([0-9]{2})[Tt ]([0-9]{2}):([0-9]{2}):([0-9]{2})(?:\.([0-9]+))?[Zz]$/;

/** 墙上时间的年、月、日、时、分。 */
type LocalFields = readonly [number, number, number, number, number];

/** 某个时刻的 UTC 墙上字段与该分钟的秒数。 */
type LocalClock = readonly [LocalFields, number];

/** 作为 UTC 文本解析出的本地墙上时间。 */
interface WallTimestamp {
  readonly nanosecond: number;
  readonly epochSecond: number;
}

/** 对齐旧浏览器 `Date.parse` 的时间接受边界。 */
export function parseAdminTime(value: string): number | null {
  if (value.length === 0) {
    return null;
  }
  const timestamp = Date.parse(value);
  return Number.isFinite(timestamp) ? timestamp : null;
}

/** 时间文本必须能被 `parseAdminTime` 解析。 */
export function validAdminTime(value: string): boolean {
  return parseAdminTime(value) !== null;
}

/** 非空且首尾没有 Rust `str::trim` 会去掉的空白。 */
export function validTrimmedText(value: string): boolean {
  return value.length > 0 && value === rustTrim(value);
}

/** 可空文本：缺省合法，出现时与 `validTrimmedText` 同一边界。 */
export function validOptionalText(value: string | null): boolean {
  return value === null || validTrimmedText(value);
}

/** 可空时间：缺省合法，出现时与 `validAdminTime` 同一边界。 */
export function validOptionalTime(value: string | null): boolean {
  return value === null || validAdminTime(value);
}

/** 计数、金额等非负 JSON 安全整数。 */
export function validCount(value: number): boolean {
  return Number.isSafeInteger(value) && value >= 0 && value <= MAX_JSON_SAFE_INTEGER;
}

/** 非空、首尾无空白、UTF-8 长度不超过上限且不含控制字符的文本。 */
export function validSafeText(value: string, maximumBytes: number): boolean {
  return validTrimmedText(value) && utf8ByteLength(value) <= maximumBytes && !/\p{Cc}/u.test(value);
}

/** 本地钟点在当前时区中无法唯一对应 UTC 的原因。 */
export type LocalTimeError = "invalid" | "nonexistent" | "ambiguous";

/**
 * 只接受在浏览器时区中有唯一 UTC 对应的本地日期时间。
 * 返回值是与 `to_rfc3339_opts(SecondsFormat::AutoSi, true)` 相同的 UTC 文本：小数秒按
 * 0/3/6/9 位自动选择，纳秒精度不丢失。
 */
export function parseLocalDatetime(value: string): Result<string, LocalTimeError> {
  return resolveLocalDatetime(value, (utcMillis) => {
    const date = new Date(utcMillis);
    if (!Number.isFinite(date.getTime())) {
      return null;
    }
    return [
      [date.getFullYear(), date.getMonth() + 1, date.getDate(), date.getHours(), date.getMinutes()],
      date.getSeconds(),
    ];
  });
}

/** 把本地日期时间文本解析为唯一的 UTC 时刻；时区偏移不足一天，逐分钟反查。 */
function resolveLocalDatetime(
  value: string,
  localAt: (utcMillis: number) => LocalClock | null,
): Result<string, LocalTimeError> {
  // 用 UTC 文本表示本地字段以严格校验日期；真正的 UTC 时刻由反向核对确定。
  const wall = parseWallTimestamp(value);
  if (wall === null) {
    return err("invalid");
  }
  const [target, targetSecond] = localFields(wall.epochSecond);
  const wallMinuteMillis = (wall.epochSecond - targetSecond) * 1_000;
  let utc: number | null = null;
  // 时区偏移不足一天。逐分钟反查，并补正含秒的旧偏移，不把时间移到缺口之后或择一回拨时刻。
  for (let minutes = -1_440; minutes <= 1_440; minutes += 1) {
    const probe = wallMinuteMillis + minutes * 60_000;
    const local = localAt(probe);
    if (local === null) {
      continue;
    }
    const [fields, second] = local;
    if (!sameFields(fields, target)) {
      continue;
    }
    const candidate = probe + (targetSecond - second) * 1_000;
    if (targetSecond !== second) {
      const corrected = localAt(candidate);
      if (
        corrected === null ||
        corrected[1] !== targetSecond ||
        !sameFields(corrected[0], target)
      ) {
        continue;
      }
    }
    if (utc !== null && utc !== candidate) {
      return err("ambiguous");
    }
    utc = candidate;
  }
  if (utc === null) {
    return err("nonexistent");
  }
  const formatted = formatTimestampRfc3339(utc, wall.nanosecond);
  return formatted === null ? err("nonexistent") : ok(formatted);
}

/** 把 `Z` 结尾的 RFC 3339 墙上时间解析成字段与纪元秒；越出后端时间范围时为 `null`。 */
function parseWallTimestamp(value: string): WallTimestamp | null {
  const wall = utf8ByteLength(value) === 16 ? `${value}:00Z` : `${value}Z`;
  const match = WALL_TIMESTAMP_PATTERN.exec(wall);
  if (match === null) {
    return null;
  }
  const [
    ,
    yearText = "",
    monthText = "",
    dayText = "",
    hourText = "",
    minuteText = "",
    secondText = "",
    fraction = "",
  ] = match;
  const year = Number(yearText);
  const month = Number(monthText);
  const day = Number(dayText);
  const hour = Number(hourText);
  const minute = Number(minuteText);
  const second = Number(secondText);
  if (
    month < 1 ||
    month > 12 ||
    day < 1 ||
    day > daysInMonth(year, month) ||
    hour > 23 ||
    minute > 59 ||
    second > 59
  ) {
    return null;
  }
  const epochSecond =
    daysFromCivil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second;
  const nanosecond = fractionNanoseconds(fraction);
  // 上下界本身是 jiff 的最大/最小时刻；边界秒内仍可带任意合法纳秒。
  if (epochSecond < MIN_TIMESTAMP_SECONDS || epochSecond > MAX_TIMESTAMP_SECONDS) {
    return null;
  }
  return { nanosecond, epochSecond };
}

/** 小数秒按前九位解释为纳秒；更多位与后端解析一样被忽略。 */
function fractionNanoseconds(digits: string): number {
  if (digits.length === 0) {
    return 0;
  }
  return Number(digits.slice(0, 9).padEnd(9, "0"));
}

/** 取时刻的 UTC 墙上字段（年、月、日、时、分）与该分钟的秒数。 */
function localFields(epochSecond: number): LocalClock {
  const date = new Date(epochSecond * 1_000);
  return [
    [
      date.getUTCFullYear(),
      date.getUTCMonth() + 1,
      date.getUTCDate(),
      date.getUTCHours(),
      date.getUTCMinutes(),
    ],
    date.getUTCSeconds(),
  ];
}

/** 五个墙上字段逐一相等。 */
function sameFields(left: LocalFields, right: LocalFields): boolean {
  return (
    left[0] === right[0] &&
    left[1] === right[1] &&
    left[2] === right[2] &&
    left[3] === right[3] &&
    left[4] === right[4]
  );
}

/** 纪元毫秒与纳秒写成 AutoSi 精度的 RFC 3339 UTC 文本；越界时为 `null`。 */
function formatTimestampRfc3339(epochMillis: number, nanosecond: number): string | null {
  const epochSecond = Math.trunc(epochMillis / 1_000);
  if (
    !Number.isSafeInteger(epochSecond) ||
    epochSecond < MIN_TIMESTAMP_SECONDS ||
    epochSecond > MAX_TIMESTAMP_SECONDS
  ) {
    return null;
  }
  const date = new Date(epochSecond * 1_000);
  if (!Number.isFinite(date.getTime())) {
    return null;
  }
  return (
    `${padYear(date.getUTCFullYear())}-${padTwo(date.getUTCMonth() + 1)}` +
    `-${padTwo(date.getUTCDate())}T${padTwo(date.getUTCHours())}` +
    `:${padTwo(date.getUTCMinutes())}:${padTwo(date.getUTCSeconds())}${fractionText(nanosecond)}Z`
  );
}

/** 小数秒的 AutoSi 文本：0、3、6 或 9 位。 */
function fractionText(nanosecond: number): string {
  if (nanosecond === 0) {
    return "";
  }
  if (nanosecond % 1_000_000 === 0) {
    return `.${String(nanosecond / 1_000_000).padStart(3, "0")}`;
  }
  if (nanosecond % 1_000 === 0) {
    return `.${String(nanosecond / 1_000).padStart(6, "0")}`;
  }
  return `.${String(nanosecond).padStart(9, "0")}`;
}

/**
 * API 的 RFC3339 时间 → 浏览器 `datetime-local` 的本地分钟文本（`YYYY-MM-DDTHH:MM`，不带
 * 时区）。预填输入框与提交换算必须共用同一时区规则；带偏移的文本会被 `datetime-local`
 * 净化成空值。
 */
export function datetimeLocalValue(value: string): string | null {
  const date = new Date(value);
  if (!Number.isFinite(date.getTime())) {
    return null;
  }
  return (
    `${padLocalYear(date.getFullYear())}-${padTwo(date.getMonth() + 1)}-${padTwo(date.getDate())}` +
    `T${padTwo(date.getHours())}:${padTwo(date.getMinutes())}`
  );
}

function padTwo(value: number): string {
  return String(value).padStart(2, "0");
}

/** 年份至少四位；负年份与后端一样带符号。 */
function padYear(year: number): string {
  return year < 0 ? `-${String(-year).padStart(4, "0")}` : String(year).padStart(4, "0");
}

/** 本地分钟文本的年份按含符号总宽度补零；RFC 3339 年份另有格式，不复用这里的负年份宽度。 */
function padLocalYear(year: number): string {
  return year < 0 ? `-${String(-year).padStart(3, "0")}` : String(year).padStart(4, "0");
}

function isLeapYear(year: number): boolean {
  return year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0);
}

function daysInMonth(year: number, month: number): number {
  if (month === 2) {
    return isLeapYear(year) ? 29 : 28;
  }
  if (month === 4 || month === 6 || month === 9 || month === 11) {
    return 30;
  }
  return 31;
}

/** 公历日期到 1970-01-01 起算的天数；与后端日期运算一致，不受宿主 Date 的两位年份规则影响。 */
function daysFromCivil(year: number, month: number, day: number): number {
  const adjustedYear = year - (month <= 2 ? 1 : 0);
  const era = Math.floor(adjustedYear / 400);
  const yearOfEra = adjustedYear - era * 400;
  const adjustedMonth = month + (month > 2 ? -3 : 9);
  const dayOfYear = Math.floor((153 * adjustedMonth + 2) / 5) + day - 1;
  const dayOfEra =
    yearOfEra * 365 + Math.floor(yearOfEra / 4) - Math.floor(yearOfEra / 100) + dayOfYear;
  return era * 146_097 + dayOfEra - 719_468;
}

/** Rust `str::trim` 的空白集合；JS 的 `trim` 与之不同，不能替换。 */
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
    code === 0x09 ||
    code === 0x0a ||
    code === 0x0b ||
    code === 0x0c ||
    code === 0x0d ||
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
