// 后端 `auction_time::Timestamp` 的线上表示：校验宽松 RFC 3339，并保留收到时的原始字符串。

import * as v from "valibot";

// 与 jiff::Timestamp::MIN/MAX 对应的 Unix 秒；边界已预留任何合法偏移仍可落回民用日期范围。
const MAX_TIMESTAMP_SECONDS = 253_402_207_200;
const MIN_TIMESTAMP_SECONDS = -377_705_023_201;

const RUST_WHITESPACE =
  /^[\t\n\v\f\r \u0085\u00a0\u1680\u2000-\u200a\u2028\u2029\u202f\u205f\u3000]$/;

type NumberToken = {
  readonly rest: string;
  readonly value: number;
};

function isAsciiDigit(character: string): boolean {
  return character >= "0" && character <= "9";
}

function isRustWhitespace(character: string): boolean {
  return RUST_WHITESPACE.test(character);
}

function trimStartRustWhitespace(text: string): string {
  let index = 0;
  while (index < text.length && isRustWhitespace(text.charAt(index))) {
    index += 1;
  }
  return text.slice(index);
}

function equalsAsciiIgnoreCase(text: string, expected: string): boolean {
  if (text.length !== expected.length) {
    return false;
  }
  for (let index = 0; index < text.length; index += 1) {
    let actual = text.charCodeAt(index);
    if (actual >= 65 && actual <= 90) {
      actual += 32;
    }
    if (actual !== expected.charCodeAt(index)) {
      return false;
    }
  }
  return true;
}

function readNumber(text: string, min: number, max: number): NumberToken | null {
  if (text.length < min) {
    return null;
  }
  let value = 0;
  let consumed = 0;
  const limit = Math.min(text.length, max);
  while (consumed < limit) {
    const character = text.charAt(consumed);
    if (!isAsciiDigit(character)) {
      if (consumed < min) {
        return null;
      }
      break;
    }
    const digit = character.charCodeAt(0) - 48;
    if (value > (Number.MAX_SAFE_INTEGER - digit) / 10) {
      return null;
    }
    value = value * 10 + digit;
    consumed += 1;
  }
  if (consumed < min) {
    return null;
  }
  return { rest: text.slice(consumed), value };
}

function literal(text: string, expected: string): string | null {
  return text.startsWith(expected) ? text.slice(expected.length) : null;
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

function parseTwoDigitNumber(text: string): NumberToken | null {
  if (text.length < 2) {
    return null;
  }
  const digits = text.slice(0, 2);
  if (!isAsciiDigit(digits.charAt(0)) || !isAsciiDigit(digits.charAt(1))) {
    return null;
  }
  return { rest: text.slice(2), value: Number(digits) };
}

function parseTimezoneOffset(text: string): NumberToken | null {
  if (text.startsWith("Z") || text.startsWith("z")) {
    return { rest: text.slice(1), value: 0 };
  }
  if (equalsAsciiIgnoreCase(text.slice(0, 3), "utc")) {
    return { rest: text.slice(3), value: 0 };
  }

  const sign = text.charAt(0);
  const negative = sign === "-" || sign === "\u2212";
  if (sign !== "+" && !negative) {
    return null;
  }
  const hours = parseTwoDigitNumber(text.slice(1));
  if (hours === null) {
    return null;
  }

  let rest = hours.rest;
  while (rest.length > 0 && (rest.startsWith(":") || isRustWhitespace(rest.charAt(0)))) {
    rest = rest.slice(1);
  }
  const minutes = parseTwoDigitNumber(rest);
  if (minutes === null || minutes.value > 59) {
    return null;
  }

  const seconds = hours.value * 3_600 + minutes.value * 60;
  if (seconds >= 86_400) {
    return null;
  }
  return { rest: minutes.rest, value: negative ? -seconds : seconds };
}

function isRelaxedRfc3339Timestamp(text: string): boolean {
  let rest = trimStartRustWhitespace(text);

  let negativeYear = false;
  let signedYear = false;
  if (rest.startsWith("-")) {
    negativeYear = true;
    signedYear = true;
    rest = rest.slice(1);
  } else if (rest.startsWith("+")) {
    signedYear = true;
    rest = rest.slice(1);
  }
  const yearToken = readNumber(rest, 1, signedYear ? Number.POSITIVE_INFINITY : 4);
  if (yearToken === null) {
    return false;
  }
  const year = negativeYear ? -yearToken.value : yearToken.value;
  if (year < -9_999 || year > 9_999) {
    return false;
  }

  rest = literal(trimStartRustWhitespace(yearToken.rest), "-") ?? "";
  if (rest === "") {
    return false;
  }
  const monthToken = readNumber(trimStartRustWhitespace(rest), 1, 2);
  if (monthToken === null || monthToken.value < 1 || monthToken.value > 12) {
    return false;
  }

  rest = literal(trimStartRustWhitespace(monthToken.rest), "-") ?? "";
  if (rest === "") {
    return false;
  }
  const dayToken = readNumber(trimStartRustWhitespace(rest), 1, 2);
  if (
    dayToken === null ||
    dayToken.value < 1 ||
    dayToken.value > daysInMonth(year, monthToken.value)
  ) {
    return false;
  }

  const delimiter = dayToken.rest.charAt(0);
  if (delimiter !== "T" && delimiter !== "t" && delimiter !== " ") {
    return false;
  }
  rest = dayToken.rest.slice(1);

  const hourToken = readNumber(trimStartRustWhitespace(rest), 1, 2);
  if (hourToken === null || hourToken.value > 23) {
    return false;
  }

  rest = literal(trimStartRustWhitespace(hourToken.rest), ":") ?? "";
  if (rest === "") {
    return false;
  }
  const minuteToken = readNumber(trimStartRustWhitespace(rest), 1, 2);
  if (minuteToken === null || minuteToken.value > 59) {
    return false;
  }

  rest = literal(trimStartRustWhitespace(minuteToken.rest), ":") ?? "";
  if (rest === "") {
    return false;
  }
  const secondToken = readNumber(trimStartRustWhitespace(rest), 1, 2);
  if (secondToken === null || secondToken.value > 60) {
    return false;
  }
  const leapSecond = secondToken.value === 60;
  const second = leapSecond ? 59 : secondToken.value;
  rest = secondToken.rest;

  if (rest.startsWith(".")) {
    const fraction = readNumber(rest.slice(1), 1, 9);
    if (fraction === null) {
      return false;
    }
    rest = fraction.rest;
    while (rest.length > 0 && isAsciiDigit(rest.charAt(0))) {
      rest = rest.slice(1);
    }
  }

  const offset = parseTimezoneOffset(trimStartRustWhitespace(rest));
  if (offset === null || trimStartRustWhitespace(offset.rest) !== "") {
    return false;
  }
  if (leapSecond) {
    return false;
  }

  const localSeconds =
    daysFromCivil(year, monthToken.value, dayToken.value) * 86_400 +
    hourToken.value * 3_600 +
    minuteToken.value * 60 +
    second;
  const utcSeconds = localSeconds - offset.value;
  if (utcSeconds < MIN_TIMESTAMP_SECONDS || utcSeconds > MAX_TIMESTAMP_SECONDS) {
    return false;
  }
  return true;
}

/** 后端 `auction_time::Timestamp`：接受宽松 RFC 3339，输出保持原始字符串。 */
export const timestamp = v.pipe(
  v.string(),
  v.check(isRelaxedRfc3339Timestamp, "必须是 RFC 3339 时刻"),
);
