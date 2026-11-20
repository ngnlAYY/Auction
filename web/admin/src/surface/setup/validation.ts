// 首次部署输入层校验与 DTO 转换；服务端仍独立校验完整配置和日志过滤器语法。

import type { SetupRequest } from "@auction/contracts/setup";
import { err, ok, type Result, unreachable } from "@auction/web-shared/result";
import { utf8ByteLength } from "@auction/web-shared/text";

import { validSafeText } from "../../api/validation";
import {
  type DatabaseTarget,
  SETUP_FIELD_DATABASE_TARGET,
  SETUP_FIELD_LISTENER,
  SETUP_FIELD_WEEKDAYS,
  SETUP_FIELDS,
  type SetupDraft,
  type SetupField,
  setupDraftValue as setupDraftValueRaw,
  setupFieldIsScheduleTime,
  setupFieldIsWeekday,
  setupFieldMaximumBytes,
  setupFieldNumberBounds,
} from "./draft";
import { isAsciiDigitCode, isAsciiDigits, rustTrim, validHost, validOrigin } from "./host";

/** 只保存字段身份和翻译键，不在错误中带入凭据或固定当前语言。 */
export interface SetupIssue {
  readonly field: SetupField;
  readonly key: string;
}

/** 两种提交动作复用同一入口；任何错误都禁止构造候选，数字解析不能静默沿用旧值。 */
export function validate(draft: SetupDraft): Result<SetupRequest, readonly SetupIssue[]> {
  const issues: SetupIssue[] = [];
  for (const field of SETUP_FIELDS) {
    const key = issue(draft, field);
    if (key !== null) {
      issues.push({ field, key });
    }
  }
  if (issues.length > 0) {
    return err(issues);
  }
  const databasePort = number(draft, "database_port");
  if (!databasePort.ok) {
    return err(databasePort.error);
  }
  const publicListenerPort = number(draft, "public_listener_port");
  if (!publicListenerPort.ok) {
    return err(publicListenerPort.error);
  }
  const adminListenerPort = number(draft, "admin_listener_port");
  if (!adminListenerPort.ok) {
    return err(adminListenerPort.error);
  }
  const startPrice = number(draft, "start_price");
  if (!startPrice.ok) {
    return err(startPrice.error);
  }
  const workerConcurrency = number(draft, "worker_concurrency");
  if (!workerConcurrency.ok) {
    return err(workerConcurrency.error);
  }
  const workerPoll = number(draft, "worker_poll");
  if (!workerPoll.ok) {
    return err(workerPoll.error);
  }
  const realtimePoll = number(draft, "realtime_poll");
  if (!realtimePoll.ok) {
    return err(realtimePoll.error);
  }
  return ok({
    database: {
      host: setupDraftValue(draft, "database_host"),
      port: databasePort.value,
      username: setupDraftValue(draft, "database_username"),
      password: setupDraftValue(draft, "database_password"),
      name: setupDraftValue(draft, "database_name"),
    },
    admin_actor: setupDraftValue(draft, "admin_actor"),
    admin_secret: setupDraftValue(draft, "admin_secret"),
    public_origin: setupDraftValue(draft, "public_origin"),
    admin_origin: setupDraftValue(draft, "admin_origin"),
    public_listener_port: publicListenerPort.value,
    admin_listener_port: adminListenerPort.value,
    metrics_enabled: draft.metricsEnabled,
    schedule_enabled: draft.scheduleEnabled,
    schedule: {
      timezone: setupDraftValue(draft, "schedule_timezone"),
      preview_day: setupDraftValue(draft, "schedule_preview_day"),
      preview_time: setupDraftValue(draft, "schedule_preview_time"),
      open_day: setupDraftValue(draft, "schedule_open_day"),
      open_time: setupDraftValue(draft, "schedule_open_time"),
      nominal_end_day: setupDraftValue(draft, "schedule_end_day"),
      nominal_end_time: setupDraftValue(draft, "schedule_end_time"),
    },
    start_price: startPrice.value,
    log_filter: setupDraftValue(draft, "log_filter"),
    worker_concurrency: workerConcurrency.value,
    worker_poll_interval_seconds: workerPoll.value,
    realtime_poll_interval_millis: realtimePoll.value,
    confirm_initialize: false,
  });
}

/** 确认只接收可提交的目标字段，避免把无效输入呈现成可授权的目标。 */
export function databaseTarget(draft: SetupDraft): DatabaseTarget | null {
  const [host, port, name] = SETUP_FIELD_DATABASE_TARGET;
  if (host === undefined || port === undefined || name === undefined) {
    return null;
  }
  if (SETUP_FIELD_DATABASE_TARGET.some((field) => issue(draft, field) !== null)) {
    return null;
  }
  const parsedPort = parseU16(setupDraftValue(draft, port));
  if (parsedPort === null) {
    return null;
  }
  return {
    host: setupDraftValue(draft, host),
    port: parsedPort,
    name: setupDraftValue(draft, name),
  };
}

/** 校验单个字段并返回译文键；`null` 表示该字段可以提交。 */
export function issue(draft: SetupDraft, field: SetupField): string | null {
  const value = setupDraftValue(draft, field);
  if (field === "admin_actor") {
    // 与首次设置配置的 128 字节边界及正式登录的冒号约束对齐，原文留给用户修正。
    return !validSafeText(value, 128) || value.includes(":") ? "setup-admin-actor-invalid" : null;
  }
  if (value.length === 0) {
    return "admin-field-required";
  }
  const maximum = setupFieldMaximumBytes(field);
  if (maximum !== null && utf8ByteLength(value) > maximum) {
    return "setup-field-invalid";
  }
  const bounds = setupFieldNumberBounds(field);
  if (bounds !== null) {
    const [minimum, maximumValue, step] = bounds;
    if (!numberInBounds(value, minimum, maximumValue, step)) {
      return "setup-field-invalid";
    }
  }
  if (SETUP_FIELD_LISTENER.includes(field)) {
    const publicPort = parseU16(setupDraftValue(draft, "public_listener_port"));
    const adminPort = parseU16(setupDraftValue(draft, "admin_listener_port"));
    if (publicPort !== null && adminPort !== null && publicPort === adminPort) {
      // 按数值比较，避免前导零绕过两个回环监听器的端口互斥。
      return "setup-listener-conflict";
    }
  }
  if (field === "schedule_open_time") {
    const key = scheduleOrderIssue(draft);
    if (key !== null) {
      return key;
    }
  }
  if (field === "schedule_timezone") {
    return scheduleTimezoneValid(value) ? null : "setup-field-invalid";
  }
  if (setupFieldIsWeekday(field)) {
    return SETUP_FIELD_WEEKDAYS.some(([day]) => day === value) ? null : "setup-field-invalid";
  }
  if (setupFieldIsScheduleTime(field)) {
    return scheduleMinute(value) !== null ? null : "setup-field-invalid";
  }
  switch (field) {
    case "database_host":
      return databaseText(value) && validHost(value) ? null : "setup-field-invalid";
    case "database_username":
    case "database_password":
      return databaseText(value) ? null : "setup-field-invalid";
    case "database_name":
      return databaseText(value) && value !== "." && value !== ".." ? null : "setup-field-invalid";
    case "admin_secret":
      return utf8ByteLength(value) >= 32 && !containsAsciiControl(value)
        ? null
        : "setup-field-invalid";
    case "admin_confirmation":
      return value === setupDraftValue(draft, "admin_secret") ? null : "setup-password-mismatch";
    case "public_origin":
    case "admin_origin":
      if (setupDraftValue(draft, "public_origin") === setupDraftValue(draft, "admin_origin")) {
        return "setup-origins-conflict";
      }
      return validOrigin(value) ? null : "setup-field-invalid";
    case "log_filter":
      return !value.includes("\r") && !value.includes("\n") && !value.includes("\0")
        ? null
        : "setup-field-invalid";
    case "database_port":
    case "public_listener_port":
    case "admin_listener_port":
    case "schedule_preview_day":
    case "schedule_preview_time":
    case "schedule_open_day":
    case "schedule_open_time":
    case "schedule_end_day":
    case "schedule_end_time":
    case "start_price":
    case "worker_concurrency":
    case "worker_poll":
    case "realtime_poll":
      return null;
  }
  return unreachable(field);
}

/** 页面只提示本地周内的分钟顺序；IANA 时区有效性和真实排期由服务端决定。 */
function scheduleOrderIssue(draft: SetupDraft): string | null {
  const preview = scheduleMoment(draft, "schedule_preview_day", "schedule_preview_time");
  const open = scheduleMoment(draft, "schedule_open_day", "schedule_open_time");
  const end = scheduleMoment(draft, "schedule_end_day", "schedule_end_time");
  if (preview === null || open === null || end === null) {
    return null;
  }
  return preview + 120 > open || open >= end ? "setup-schedule-order" : null;
}

function scheduleMoment(
  draft: SetupDraft,
  dayField: SetupField,
  timeField: SetupField,
): number | null {
  const day = SETUP_FIELD_WEEKDAYS.findIndex(
    ([value]) => value === setupDraftValue(draft, dayField),
  );
  if (day < 0) {
    return null;
  }
  const minute = scheduleMinute(setupDraftValue(draft, timeField));
  if (minute === null) {
    return null;
  }
  return day * 24 * 60 + minute;
}

/** 规范的 `HH:MM`（各两位、补零）是当天的第几分钟；其他写法一律不接受。 */
function scheduleMinute(value: string): number | null {
  if (value.length !== 5 || value.charCodeAt(2) !== 0x3a) {
    return null;
  }
  const hour = digitPair(value.charCodeAt(0), value.charCodeAt(1));
  const minute = digitPair(value.charCodeAt(3), value.charCodeAt(4));
  if (hour === null || minute === null || hour >= 24 || minute >= 60) {
    return null;
  }
  return hour * 60 + minute;
}

function digitPair(tens: number, ones: number): number | null {
  if (!isAsciiDigitCode(tens) || !isAsciiDigitCode(ones)) {
    return null;
  }
  return (tens - 0x30) * 10 + (ones - 0x30);
}

function databaseText(value: string): boolean {
  return rustTrim(value).length > 0 && !/\p{Cc}/u.test(value);
}

/** 孤立代理项在 Rust WASM 边界会变成 U+FFFD；校验与 DTO 使用同一份标量文本。 */
function scalarText(value: string): string {
  let result = "";
  for (let index = 0; index < value.length; index += 1) {
    const code = value.charCodeAt(index);
    if (code >= 0xd800 && code <= 0xdbff) {
      const next = value.charCodeAt(index + 1);
      if (next >= 0xdc00 && next <= 0xdfff) {
        result += value.charAt(index) + value.charAt(index + 1);
        index += 1;
      } else {
        result += "\uFFFD";
      }
    } else if (code >= 0xdc00 && code <= 0xdfff) {
      result += "\uFFFD";
    } else {
      result += value.charAt(index);
    }
  }
  return result;
}

function setupDraftValue(draft: SetupDraft, field: SetupField): string {
  return scalarText(setupDraftValueRaw(draft, field));
}

function number(draft: SetupDraft, field: SetupField): Result<number, readonly SetupIssue[]> {
  const parsed = parseInteger(setupDraftValue(draft, field));
  return parsed === null ? err([{ field, key: "setup-field-invalid" }] as const) : ok(parsed);
}

/** 数字化字段已经通过无符号十进制检查；这里保留 Rust `FromStr` 的失败分支。 */
function parseInteger(value: string): number | null {
  const digits = value.startsWith("+") ? value.slice(1) : value;
  if (!isAsciiDigits(digits)) {
    return null;
  }
  const parsed = Number(digits);
  return Number.isSafeInteger(parsed) ? parsed : null;
}

/** 与 `u16::from_str` 一致：可带一个前导 `+`，允许前导零，超出范围失败。 */
function parseU16(value: string): number | null {
  const digits = value.startsWith("+") ? value.slice(1) : value;
  if (!isAsciiDigits(digits)) {
    return null;
  }
  const significant = digits.replace(/^0+/, "") || "0";
  if (significant.length > 5) {
    return null;
  }
  const parsed = Number(significant);
  return parsed <= 65_535 ? parsed : null;
}

/** 与 `u64` 解析一致：只接受 ASCII 数字，前导零不计入溢出，再检查上下界与步长。 */
function numberInBounds(value: string, minimum: number, maximum: number, step: number): boolean {
  if (!isAsciiDigits(value)) {
    return false;
  }
  const significant = value.replace(/^0+/, "") || "0";
  if (significant.length > 20) {
    return false;
  }
  const parsed = BigInt(significant);
  return parsed >= BigInt(minimum) && parsed <= BigInt(maximum) && parsed % BigInt(step) === 0n;
}

function scheduleTimezoneValid(value: string): boolean {
  for (let index = 0; index < value.length; index += 1) {
    const code = value.charCodeAt(index);
    const alphanumeric =
      (code >= 0x30 && code <= 0x39) ||
      (code >= 0x41 && code <= 0x5a) ||
      (code >= 0x61 && code <= 0x7a);
    if (!alphanumeric && code !== 0x2f && code !== 0x5f && code !== 0x2d && code !== 0x2b) {
      return false;
    }
  }
  return true;
}

function containsAsciiControl(value: string): boolean {
  for (let index = 0; index < value.length; index += 1) {
    const code = value.charCodeAt(index);
    if (code <= 0x1f || code === 0x7f) {
      return true;
    }
  }
  return false;
}
