// 严格 RFC 3339 时刻解析；只接受后端 `Timestamp::parse_from_rfc3339` 的定宽语法。
// 解析结果是纪元纳秒，用于精确比较时刻。

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
  if (month === 2) {
    return isLeapYear(year) ? 29 : 28;
  }
  if (month === 4 || month === 6 || month === 9 || month === 11) {
    return 30;
  }
  return 31;
}

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
