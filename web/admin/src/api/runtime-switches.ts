// 运行时开关客户端：读取全部开关；写入只开放外部访问暂停。

import {
  type AdminExternalAccessPauseRequest,
  AdminExternalAccessPauseResponseSchema,
  type RuntimeSwitchIntentDto,
  type RuntimeSwitchListResponse,
  RuntimeSwitchListResponseSchema,
} from "@auction/contracts/dto/admin/runtime-switches";
import { apiResponseSchema } from "@auction/contracts/dto/response";
import type { JsonFieldPolicy } from "@auction/web-shared/net/wire";
import { err, ok, type Result } from "@auction/web-shared/result";

import { parseRustUuid, rustUuidIsNil } from "./credentials/decode";
import {
  type AdminApiFailure,
  adminApiFailureClient,
  decodeAdminReceipt,
  invalidResponseError,
  responseError,
} from "./failure";
import type { AdminSessionHost } from "./session";
import { trimRustWhitespace } from "./session-time";
import {
  type PreparedJsonCommand,
  prepareJsonCommandOnce,
  sendGet,
  sendPreparedJsonWrite,
} from "./transport";
import { validSafeText } from "./validation";

const KNOWN_SWITCHES: readonly string[] = [
  "readiness_audit_failure_mode",
  "readiness_cache_enabled",
  "metrics_sli_snapshot_enabled",
  "mooglemail_poll_isolation_enabled",
  "metrics_reject_forwarded_requests",
  "external_access_paused",
  "disposition_read_mode",
];

/** 两个数组里的具型修订号会参与命令围栏；其余自由值可能含浮点数。 */
const RUNTIME_SWITCH_LIST_FIELD_POLICY: JsonFieldPolicy = {
  uniqueKeys: "all",
  integerPaths: [
    ["data", "database_intent", null, "revision"],
    ["data", "admin_applied", null, "revision"],
  ],
};

/** 暂停回执里的 `revision` 会决定下一次命令的期望版本。 */
const EXTERNAL_ACCESS_PAUSE_RECEIPT_FIELD_POLICY: JsonFieldPolicy = {
  uniqueKeys: "all",
  integerPaths: [["data", "accepted", "revision"]],
};

/** 读取全部运行时开关，并核对两个数组都是已知闭集且应用值都有数据库意图。 */
export async function fetchRuntimeSwitches(
  host: AdminSessionHost,
  signal: AbortSignal,
): Promise<Result<RuntimeSwitchListResponse, string>> {
  const response = await sendGet(host, "/api/v1/admin/runtime-switches", signal);
  if (!response.ok) {
    return err(response.error);
  }
  const received = response.value;
  if (!responseOk(received)) {
    return err(await responseError(received, signal, host));
  }
  const payload = decodeAdminReceipt(
    received,
    apiResponseSchema(RuntimeSwitchListResponseSchema),
    RUNTIME_SWITCH_LIST_FIELD_POLICY,
  );
  if (!payload.ok) {
    return err(payload.error);
  }
  const runtime = payload.value.data;
  const databaseNames = runtime.database_intent.map((item) => item.name);
  const appliedNames = runtime.admin_applied.map((item) => item.name);
  if (
    !closedSubset(databaseNames) ||
    !closedSubset(appliedNames) ||
    !appliedNames.every((name) => databaseNames.includes(name))
  ) {
    return err("invalid_response");
  }
  return ok(runtime);
}

function closedSubset(names: readonly string[]): boolean {
  return (
    names.every((name) => KNOWN_SWITCHES.includes(name)) &&
    names.every((name, index) => !names.slice(0, index).includes(name))
  );
}

/**
 * 一次暂停或恢复尝试。结果未知时必须整份原样重试：命令 ID、期望版本、绝对期限与原因
 * 都在首次提交时确定，不能按重试时刻重新计算。
 */
export interface ExternalAccessPauseAttempt {
  readonly commandId: string;
  readonly expectedRevision: number;
  readonly paused: boolean;
  readonly expiresAt: string | null;
  readonly reason: string;
}

/** 与服务端 RuntimeSwitchCommand 共用的 UTF-8 字节边界。 */
export const EXTERNAL_ACCESS_PAUSE_MAX_REASON_BYTES = 1024;

/** 校验将提交的单行原因。表单与发送前检查使用同一口径。 */
export function validExternalAccessPauseReason(value: string): boolean {
  return validSafeText(value, EXTERNAL_ACCESS_PAUSE_MAX_REASON_BYTES);
}

/** 宽松 RFC 3339 时刻的 UTC 秒与纳秒；与 `auction_time::Timestamp` 的反序列化一致。 */
interface RustTimestamp {
  readonly epochSecond: bigint;
  readonly nanosecond: number;
}

// `auction_time::Timestamp` 的可表示范围（UTC 秒）。
const TIMESTAMP_MIN_SECONDS = -377_705_023_201n;
const TIMESTAMP_MAX_SECONDS = 253_402_207_200n;
const RUST_WHITESPACE =
  /^[\t\n\v\f\r \u0085\u00a0\u1680\u2000-\u200a\u2028\u2029\u202f\u205f\u3000]$/u;

interface NumberToken {
  readonly rest: string;
  readonly value: number;
}

/** 把宽松 RFC 3339 期限规范化为 `Timestamp` 序列化使用的 UTC 文本；非法时为 `null`。 */
function canonicalExternalAccessPauseExpiry(value: string | null): string | null {
  if (value === null || typeof value !== "string") {
    return null;
  }
  const parsed = parseRelaxedTimestamp(value);
  return parsed === null ? null : formatTimestamp(parsed);
}

/** 按 `auction-time` 的宽松 RFC 3339 语法读取时刻；范围与字段校验在最后统一执行。 */
function parseRelaxedTimestamp(text: string): RustTimestamp | null {
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
    return null;
  }
  const year = negativeYear ? -yearToken.value : yearToken.value;
  if (year < -9_999 || year > 9_999) {
    return null;
  }

  rest = trimStartRustWhitespace(yearToken.rest);
  if (!rest.startsWith("-")) {
    return null;
  }
  const monthToken = readNumber(trimStartRustWhitespace(rest.slice(1)), 1, 2);
  if (monthToken === null || monthToken.value < 1 || monthToken.value > 12) {
    return null;
  }

  rest = trimStartRustWhitespace(monthToken.rest);
  if (!rest.startsWith("-")) {
    return null;
  }
  const dayToken = readNumber(trimStartRustWhitespace(rest.slice(1)), 1, 2);
  if (
    dayToken === null ||
    dayToken.value < 1 ||
    dayToken.value > daysInMonth(year, monthToken.value)
  ) {
    return null;
  }

  const delimiter = dayToken.rest.charAt(0);
  if (delimiter !== "T" && delimiter !== "t" && delimiter !== " ") {
    return null;
  }
  rest = dayToken.rest.slice(1);

  const hourToken = readNumber(trimStartRustWhitespace(rest), 1, 2);
  if (hourToken === null || hourToken.value > 23) {
    return null;
  }
  rest = trimStartRustWhitespace(hourToken.rest);
  if (!rest.startsWith(":")) {
    return null;
  }
  const minuteToken = readNumber(trimStartRustWhitespace(rest.slice(1)), 1, 2);
  if (minuteToken === null || minuteToken.value > 59) {
    return null;
  }
  rest = trimStartRustWhitespace(minuteToken.rest);
  if (!rest.startsWith(":")) {
    return null;
  }
  const secondToken = readNumber(trimStartRustWhitespace(rest.slice(1)), 1, 2);
  if (secondToken === null || secondToken.value > 59) {
    return null;
  }
  rest = secondToken.rest;

  let nanosecond = 0;
  if (rest.startsWith(".")) {
    const fractionText = rest.slice(1);
    const fraction = readNumber(fractionText, 1, 9);
    if (fraction === null) {
      return null;
    }
    const digits = fractionText.length - fraction.rest.length;
    nanosecond = fraction.value * 10 ** (9 - digits);
    rest = fraction.rest;
    while (rest.length > 0 && isAsciiDigit(rest.charAt(0))) {
      rest = rest.slice(1);
    }
  }

  const offset = parseTimezoneOffset(trimStartRustWhitespace(rest));
  if (offset === null || trimStartRustWhitespace(offset.rest) !== "") {
    return null;
  }

  const localSeconds =
    daysFromCivil(year, monthToken.value, dayToken.value) * 86_400 +
    hourToken.value * 3_600 +
    minuteToken.value * 60 +
    secondToken.value;
  const utcSeconds = BigInt(localSeconds - offset.seconds);
  if (utcSeconds < TIMESTAMP_MIN_SECONDS || utcSeconds > TIMESTAMP_MAX_SECONDS) {
    return null;
  }
  return { epochSecond: utcSeconds, nanosecond };
}

function parseTimezoneOffset(
  text: string,
): { readonly rest: string; readonly seconds: number } | null {
  if (text.startsWith("Z") || text.startsWith("z")) {
    return { rest: text.slice(1), seconds: 0 };
  }
  if (equalsAsciiIgnoreCase(text.slice(0, 3), "utc")) {
    return { rest: text.slice(3), seconds: 0 };
  }
  const sign = text.charAt(0);
  const negative = sign === "-" || sign === "\u2212";
  if (sign !== "+" && !negative) {
    return null;
  }
  const hours = readTwoDigitNumber(text.slice(1));
  if (hours === null) {
    return null;
  }
  let rest = hours.rest;
  while (rest.length > 0 && (rest.startsWith(":") || isRustWhitespace(rest.charAt(0)))) {
    rest = rest.slice(1);
  }
  const minutes = readTwoDigitNumber(rest);
  if (minutes === null || minutes.value > 59) {
    return null;
  }
  const seconds = hours.value * 3_600 + minutes.value * 60;
  if (seconds >= 86_400) {
    return null;
  }
  return { rest: minutes.rest, seconds: negative ? -seconds : seconds };
}

function readTwoDigitNumber(text: string): NumberToken | null {
  if (text.length < 2) {
    return null;
  }
  const first = text.charAt(0);
  const second = text.charAt(1);
  if (!isAsciiDigit(first) || !isAsciiDigit(second)) {
    return null;
  }
  return { rest: text.slice(2), value: Number(first) * 10 + Number(second) };
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

function trimStartRustWhitespace(text: string): string {
  let index = 0;
  while (index < text.length && isRustWhitespace(text.charAt(index))) {
    index += 1;
  }
  return text.slice(index);
}

function isRustWhitespace(character: string): boolean {
  return RUST_WHITESPACE.test(character);
}

function isAsciiDigit(character: string): boolean {
  return character >= "0" && character <= "9";
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

/** 把 UTC 秒与纳秒写成 `Timestamp` 的 `SecondsFormat::AutoSi` + `Z` 形式。 */
function formatTimestamp(value: RustTimestamp): string | null {
  const epochSecond = Number(value.epochSecond);
  if (!Number.isSafeInteger(epochSecond)) {
    return null;
  }
  const date = new Date(epochSecond * 1_000);
  if (!Number.isFinite(date.getTime())) {
    return null;
  }
  const year = date.getUTCFullYear();
  if (year < -9_999 || year > 9_999) {
    return null;
  }
  const yearText = year < 0 ? `-${String(-year).padStart(4, "0")}` : String(year).padStart(4, "0");
  return (
    `${yearText}-${padTwo(date.getUTCMonth() + 1)}-${padTwo(date.getUTCDate())}` +
    `T${padTwo(date.getUTCHours())}:${padTwo(date.getUTCMinutes())}:${padTwo(date.getUTCSeconds())}` +
    `${fractionText(value.nanosecond)}Z`
  );
}

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

function padTwo(value: number): string {
  return String(value).padStart(2, "0");
}

/** 已冻结的暂停尝试与首次执行时生成的正文。 */
export interface PreparedExternalAccessPause {
  readonly attempt: ExternalAccessPauseAttempt;
  readonly body: PreparedJsonCommand;
}

/**
 * 冻结暂停或恢复输入；执行体第一次运行时才结合 command ID 生成并缓存正文。
 * 原因在正文里使用 Rust `str::trim` 后的值，重试不再读取表单。
 */
export function prepareExternalAccessPauseOnce(
  attempt: ExternalAccessPauseAttempt,
): (commandId: string) => Result<PreparedExternalAccessPause, AdminApiFailure> {
  // Rust 的 `Option<Timestamp>` 反序列化会校验并规范化为 UTC 文本；这里在冻结时完成同一转换。
  const expiresAt = canonicalExternalAccessPauseExpiry(attempt.expiresAt);
  const validExpiresAt = attempt.expiresAt === null || expiresAt !== null;
  const frozen = Object.freeze({
    commandId: attempt.commandId,
    expectedRevision: attempt.expectedRevision,
    paused: attempt.paused,
    expiresAt,
    reason: attempt.reason,
  });
  const prepare = prepareJsonCommandOnce((commandId) => {
    const command: AdminExternalAccessPauseRequest = {
      command_id: commandId,
      expected_revision: frozen.expectedRevision,
      paused: frozen.paused,
      expires_at: frozen.expiresAt,
      reason: trimRustWhitespace(frozen.reason),
    };
    return command;
  });
  return (commandId) => {
    const canonicalCommandId = parseRustUuid(frozen.commandId);
    const reason = trimRustWhitespace(frozen.reason);
    if (
      !validExpiresAt ||
      canonicalCommandId === null ||
      rustUuidIsNil(canonicalCommandId) ||
      canonicalCommandId !== commandId ||
      !Number.isSafeInteger(frozen.expectedRevision) ||
      frozen.expectedRevision < 0 ||
      frozen.paused !== (frozen.expiresAt !== null) ||
      !validExternalAccessPauseReason(reason)
    ) {
      return err(adminApiFailureClient("invalid_request"));
    }
    const prepared = prepare(commandId);
    if (!prepared.ok) {
      return err(prepared.error);
    }
    return ok(Object.freeze({ attempt: frozen, body: prepared.value }));
  };
}

/** 提交外部访问暂停或恢复；只接受与命令一致的已接受事实。 */
export async function submitExternalAccessPause(
  host: AdminSessionHost,
  prepared: PreparedExternalAccessPause,
  signal: AbortSignal,
): Promise<Result<RuntimeSwitchIntentDto, AdminApiFailure>> {
  const response = await sendPreparedJsonWrite(
    host,
    "/api/v1/admin/runtime-switches/external-access-paused",
    "POST",
    prepared.body,
    signal,
  );
  if (!response.ok) {
    return err(response.error);
  }
  const payload = decodeAdminReceipt(
    response.value,
    apiResponseSchema(AdminExternalAccessPauseResponseSchema),
    EXTERNAL_ACCESS_PAUSE_RECEIPT_FIELD_POLICY,
  );
  if (!payload.ok) {
    return err(adminApiFailureClient(payload.error));
  }
  const accepted = payload.value.data.accepted;
  const expectedOperation = prepared.attempt.paused ? "set" : "clear";
  if (
    accepted.name !== "external_access_paused" ||
    accepted.operation !== expectedOperation ||
    accepted.revision < 1
  ) {
    return err(adminApiFailureClient(invalidResponseError("external access pause")));
  }
  return ok(accepted);
}

function responseOk(response: { readonly status: number }): boolean {
  return response.status >= 200 && response.status <= 299;
}
