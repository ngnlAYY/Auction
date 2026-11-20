// Admin 会话时间解析：严格 RFC 3339 与 HTTP IMF-fixdate。
// 会话时刻保留完整纪元纳秒，截止比较与投影排序不能因提前截断而改变；
// 只有求得服务端剩余时长后才向零截断为毫秒。

/** `Retry-After` 为秒数或 IMF-fixdate；绝对日期以同一响应的 Date 为基准，缺 Date 时无法计算。 */
export function retryAfterMillis(
  retryAfter: string | null,
  serverDate: string | null,
): number | null {
  if (retryAfter === null) {
    return null;
  }
  const trimmed = trimRustWhitespace(retryAfter);
  if (trimmed !== "" && /^[0-9]+$/.test(trimmed)) {
    return saturatingMillis(trimmed);
  }
  const deadline = parseImfFixdateMillis(trimmed);
  // 源码只修剪 `Retry-After`；响应 Date 直接交给 IMF-fixdate 解析。
  const sent = serverDate === null ? null : parseImfFixdateMillis(serverDate);
  if (deadline === null || sent === null) {
    return null;
  }
  return Math.max(deadline - sent, 0);
}

const U64_MAX = 18_446_744_073_709_551_615n;

/** u64 秒乘以 1000 的饱和解析；按 u64 运算，只在源码转 f64 的同一层转成 number。 */
function saturatingMillis(digits: string): number {
  const parsed = BigInt(digits);
  const seconds = parsed > U64_MAX ? U64_MAX : parsed;
  const millis = seconds > U64_MAX / 1_000n ? U64_MAX : seconds * 1_000n;
  return Number(millis);
}

/** 按 Rust `str::trim` 的空白集合去掉首尾空白；登录校验与 `Retry-After` 共用。 */
export function trimRustWhitespace(value: string): string {
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

const TIMESTAMP_MIN_MILLIS = utcBoundMillis(-9999, 1, 2, 1, 59, 59, 0);
const TIMESTAMP_MAX_MILLIS = utcBoundMillis(9999, 12, 30, 22, 0, 0, 999);

// 后端 `auction-time` 的可表示范围：-9999-01-02T01:59:59Z 到
// 9999-12-30T22:00:00.999999999Z；秒值上限允许最后一秒内的小数部分。
const MIN_TIMESTAMP_SECONDS = -377_705_023_201n;
const MAX_TIMESTAMP_SECONDS = 253_402_207_200n;

/** 严格 RFC 3339 解析，返回纪元纳秒；字段定宽、偏移必须是 `Z` 或 `±hh:mm`。 */
export function parseStrictRfc3339(text: string): bigint | null {
  if (text.length < 19) {
    return null;
  }
  const yearToken = text.slice(0, 4);
  if (!/^[0-9]{4}$/.test(yearToken)) {
    return null;
  }
  const year = Number(yearToken);
  if (text.charAt(4) !== "-" || text.charAt(7) !== "-") {
    return null;
  }
  const month = asciiTwoDigits(text, 5);
  const day = asciiTwoDigits(text, 8);
  if (month === null || day === null || month < 1 || month > 12) {
    return null;
  }
  if (day < 1 || day > daysInMonth(year, month)) {
    return null;
  }
  const separator = text.charAt(10);
  if (separator !== "T" && separator !== "t" && separator !== " ") {
    return null;
  }
  const hour = asciiTwoDigits(text, 11);
  const minute = asciiTwoDigits(text, 14);
  const second = asciiTwoDigits(text, 17);
  if (
    hour === null ||
    minute === null ||
    second === null ||
    text.charAt(13) !== ":" ||
    text.charAt(16) !== ":" ||
    hour > 23 ||
    minute > 59 ||
    second > 60
  ) {
    return null;
  }
  if (second === 60) {
    return null;
  }

  let index = 19;
  let nanosecond = 0n;
  if (text.charAt(index) === ".") {
    index += 1;
    let digits = 0;
    let value = 0;
    while (digits < 9 && index < text.length) {
      const digit = asciiDigit(text, index);
      if (digit === null) {
        break;
      }
      value = value * 10 + digit;
      digits += 1;
      index += 1;
    }
    if (digits === 0) {
      return null;
    }
    while (index < text.length && asciiDigit(text, index) !== null) {
      index += 1;
    }
    nanosecond = BigInt(value) * 10n ** BigInt(9 - digits);
  }

  let offsetSeconds = 0;
  const first = text.charAt(index);
  if (first === "Z" || first === "z") {
    index += 1;
  } else {
    const negative = first === "-" || first === "\u2212";
    if (!negative && first !== "+") {
      return null;
    }
    const offsetHour = asciiTwoDigits(text, index + 1);
    if (offsetHour === null || text.charAt(index + 3) !== ":") {
      return null;
    }
    const offsetMinute = asciiTwoDigits(text, index + 4);
    if (offsetMinute === null || offsetMinute > 59) {
      return null;
    }
    offsetSeconds = offsetHour * 3_600 + offsetMinute * 60;
    if (offsetSeconds >= 86_400) {
      return null;
    }
    if (negative) {
      offsetSeconds = -offsetSeconds;
    }
    index += 6;
  }
  if (index !== text.length) {
    return null;
  }

  const localSeconds =
    daysFromCivil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second - offsetSeconds;
  const utcSeconds = BigInt(localSeconds);
  if (utcSeconds < MIN_TIMESTAMP_SECONDS || utcSeconds > MAX_TIMESTAMP_SECONDS) {
    return null;
  }
  return utcSeconds * 1_000_000_000n + nanosecond;
}

const IMF_WEEKDAYS: readonly string[] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const IMF_MONTHS: readonly string[] = [
  "Jan",
  "Feb",
  "Mar",
  "Apr",
  "May",
  "Jun",
  "Jul",
  "Aug",
  "Sep",
  "Oct",
  "Nov",
  "Dec",
];

/** 严格 IMF-fixdate（`Sun, 06 Nov 1994 08:49:37 GMT`）解析为 UTC 毫秒。 */
function parseImfFixdateMillis(text: string): number | null {
  if (text.length !== 29 || !isAscii(text)) {
    return null;
  }
  if (text[3] !== "," || text[4] !== " " || text[7] !== " " || text[11] !== " ") {
    return null;
  }
  if (text[16] !== " " || text[19] !== ":" || text[22] !== ":" || text[25] !== " ") {
    return null;
  }
  if (text.slice(26) !== "GMT") {
    return null;
  }
  const weekday = IMF_WEEKDAYS.indexOf(text.slice(0, 3));
  const month = IMF_MONTHS.indexOf(text.slice(8, 11));
  const day = parseDigits(text, 5, 2);
  const year = parseDigits(text, 12, 4);
  const hour = parseDigits(text, 17, 2);
  const minute = parseDigits(text, 20, 2);
  const second = parseDigits(text, 23, 2);
  if (
    weekday < 0 ||
    month < 0 ||
    day === null ||
    year === null ||
    hour === null ||
    minute === null ||
    second === null ||
    day < 1 ||
    day > daysInMonth(year, month + 1) ||
    hour > 23 ||
    minute > 59 ||
    second > 59
  ) {
    return null;
  }
  const millis = utcBoundMillis(year, month + 1, day, hour, minute, second, 0);
  if (millis < TIMESTAMP_MIN_MILLIS || millis > TIMESTAMP_MAX_MILLIS) {
    return null;
  }
  const actualWeekday = new Date(millis).getUTCDay();
  const expectedWeekday = weekday === 6 ? 0 : weekday + 1;
  if (actualWeekday !== expectedWeekday) {
    return null;
  }
  return millis;
}

function parseDigits(text: string, start: number, length: number): number | null {
  if (start + length > text.length) {
    return null;
  }
  let value = 0;
  for (let index = start; index < start + length; index += 1) {
    const code = text.charCodeAt(index);
    if (code < 0x30 || code > 0x39) {
      return null;
    }
    value = value * 10 + (code - 0x30);
  }
  return value;
}

function isAscii(text: string): boolean {
  for (let index = 0; index < text.length; index += 1) {
    if (text.charCodeAt(index) > 0x7f) {
      return false;
    }
  }
  return true;
}

function asciiDigit(text: string, index: number): number | null {
  const code = text.charCodeAt(index);
  return code >= 0x30 && code <= 0x39 ? code - 0x30 : null;
}

function asciiTwoDigits(text: string, index: number): number | null {
  const high = asciiDigit(text, index);
  const low = asciiDigit(text, index + 1);
  return high === null || low === null ? null : high * 10 + low;
}

function isLeapYear(year: number): boolean {
  return year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0);
}

function daysInMonth(year: number, month: number): number {
  switch (month) {
    case 2:
      return isLeapYear(year) ? 29 : 28;
    case 4:
    case 6:
    case 9:
    case 11:
      return 30;
    default:
      return 31;
  }
}

function daysFromCivil(year: number, month: number, day: number): number {
  const adjustedYear = year - (month <= 2 ? 1 : 0);
  const era = Math.floor(adjustedYear / 400);
  const yearOfEra = adjustedYear - era * 400;
  const monthIndex = month + (month > 2 ? -3 : 9);
  const dayOfYear = Math.floor((153 * monthIndex + 2) / 5) + day - 1;
  const dayOfEra =
    yearOfEra * 365 + Math.floor(yearOfEra / 4) - Math.floor(yearOfEra / 100) + dayOfYear;
  return era * 146097 + dayOfEra - 719468;
}

function utcBoundMillis(
  year: number,
  month: number,
  day: number,
  hour: number,
  minute: number,
  second: number,
  millisecond: number,
): number {
  return (
    daysFromCivil(year, month, day) * 86_400_000 +
    hour * 3_600_000 +
    minute * 60_000 +
    second * 1_000 +
    millisecond
  );
}
