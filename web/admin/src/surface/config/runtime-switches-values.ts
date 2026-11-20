// 运行时开关卡片的值解析、时刻格式化与暂停期限计算；与卡片视图分离，便于逐函数核对。

import type { RuntimeSwitchListResponse } from "@auction/contracts/dto/admin/runtime-switches";
import { compareText } from "@auction/web-shared/text";
import { t } from "@auction/web-shared/ui/i18n";

import { EXTERNAL_ACCESS_PAUSE_MAX_REASON_BYTES } from "../../api/runtime-switches";

export const PAUSE_HOURS: readonly number[] = [1, 2, 6, 24, 72, 168];

const UINT64_MAX = 18_446_744_073_709_551_615n;
const NANOS_PER_SECOND = 1_000_000_000n;
const TIMESTAMP_MIN_SECONDS = -377_705_023_201n;
const TIMESTAMP_MAX_SECONDS = 253_402_207_200n;

/** 从数据库最新意图读取暂停版本、是否仍暂停与期限；期限以数据库读取时刻判断，不用浏览器时钟。 */
export function pauseState(data: RuntimeSwitchListResponse): {
  readonly revision: number;
  readonly paused: boolean;
  readonly pausedUntil: bigint | null;
} {
  const intent = data.database_intent.find((item) => item.name === "external_access_paused");
  if (intent === undefined) {
    return { revision: 0, paused: false, pausedUntil: null };
  }
  const expiresAt = intent.expires_at === null ? null : parseTimestamp(intent.expires_at);
  const databaseTime = parseTimestamp(data.database_time);
  const setPaused = intent.operation === "set" && intent.value === true;
  const paused =
    setPaused &&
    (intent.expires_at === null ||
      (expiresAt !== null && databaseTime !== null && expiresAt > databaseTime));
  return { revision: intent.revision, paused, pausedUntil: paused ? expiresAt : null };
}

/** 定时暂停到期时唤醒读取的延时；无期限或未暂停时没有定时器。 */
export function pauseRefreshMillis(data: RuntimeSwitchListResponse): number | null {
  const state = pauseState(data);
  if (!state.paused || state.pausedUntil === null) {
    return null;
  }
  const databaseTime = parseTimestamp(data.database_time);
  if (databaseTime === null) {
    return null;
  }
  const remaining = state.pausedUntil - databaseTime;
  if (remaining <= 0n) {
    return 0;
  }
  const millis = remaining / 1_000_000n;
  const residue = remaining % 1_000_000n;
  const total = millis + (residue === 0n ? 0n : 1n);
  return total > UINT64_MAX ? null : Number(total);
}

/** 首次确认时冻结的绝对期限；超出 Timestamp 范围时返回 `null`，调用方不进入确认。 */
export function pauseExpiry(hours: number): bigint | null {
  if (!Number.isSafeInteger(hours)) {
    return null;
  }
  const seconds = BigInt(hours) * 3_600n;
  const nowSeconds = BigInt(Math.floor(Date.now() / 1_000));
  const expires = nowSeconds + seconds;
  return expires < TIMESTAMP_MIN_SECONDS || expires > TIMESTAMP_MAX_SECONDS
    ? null
    : expires * NANOS_PER_SECOND;
}

export function reasonErrorText(): string {
  return t("admin-pause-reason-invalid", {
    maximum: EXTERNAL_ACCESS_PAUSE_MAX_REASON_BYTES,
  });
}

/** 下拉值解析：只接受十进制整数，并限定在卡片提供的暂停时长内；其他值回落为 2 小时。 */
export function parsePauseHours(value: string): number | null {
  if (!/^[+-]?[0-9]+$/u.test(value)) {
    return null;
  }
  const hours = Number(value);
  return Number.isSafeInteger(hours) && PAUSE_HOURS.includes(hours) ? hours : null;
}

export function incrementUint64(value: bigint): bigint {
  return value >= UINT64_MAX ? UINT64_MAX : value + 1n;
}

const TIMESTAMP_PATTERN = /^(.*?)(?:\.(\d{1,9}))?(Z|z|[Uu][Tt][Cc]|[+-]\d{2}:?\d{2})$/u;

/** 宽松 RFC 3339 时刻转成 UTC 纪元纳秒；只用于比较和格式化服务端时刻。 */
function parseTimestamp(text: string): bigint | null {
  const match = TIMESTAMP_PATTERN.exec(text);
  if (match === null) {
    return null;
  }
  const base = match[1] ?? "";
  const fraction = match[2] ?? "";
  const zone = match[3] ?? "";
  const milliseconds = Date.parse(`${base}${zone.toUpperCase() === "UTC" ? "Z" : zone}`);
  if (!Number.isFinite(milliseconds)) {
    return null;
  }
  return BigInt(milliseconds) * 1_000_000n + BigInt(fraction.padEnd(9, "0"));
}

/** `Timestamp::to_rfc3339` / `to_rfc3339_opts(AutoSi, true)` 的 UTC 文本。 */
export function formatTimestamp(value: bigint, useZ: boolean): string | null {
  const parts = floorTimestampParts(value);
  const epochSecond = Number(parts.seconds);
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
    `${fractionText(parts.nanosecond)}${useZ ? "Z" : "+00:00"}`
  );
}

function floorTimestampParts(value: bigint): {
  readonly seconds: bigint;
  readonly nanosecond: bigint;
} {
  let seconds = value / NANOS_PER_SECOND;
  let nanosecond = value % NANOS_PER_SECOND;
  if (nanosecond < 0n) {
    seconds -= 1n;
    nanosecond += NANOS_PER_SECOND;
  }
  return { seconds, nanosecond };
}

function fractionText(nanosecond: bigint): string {
  if (nanosecond === 0n) {
    return "";
  }
  if (nanosecond % 1_000_000n === 0n) {
    return `.${String(nanosecond / 1_000_000n).padStart(3, "0")}`;
  }
  if (nanosecond % 1_000n === 0n) {
    return `.${String(nanosecond / 1_000n).padStart(6, "0")}`;
  }
  return `.${String(nanosecond).padStart(9, "0")}`;
}

function padTwo(value: number): string {
  return String(value).padStart(2, "0");
}

export function displayTimestamp(text: string | null): string {
  if (text === null) {
    return "—";
  }
  const parsed = parseTimestamp(text);
  const formatted = parsed === null ? null : formatTimestamp(parsed, false);
  return formatted ?? "—";
}

/** serde_json 默认以 BTreeMap 保存对象，`Value::to_string()` 的键按 Unicode 码点顺序输出。 */
export function rustJsonText(value: unknown): string {
  if (Array.isArray(value)) {
    return `[${value.map(rustJsonText).join(",")}]`;
  }
  if (isJsonObject(value)) {
    const fields = Object.keys(value)
      .sort(compareText)
      .map((key) => `${JSON.stringify(key)}:${rustJsonText(value[key])}`);
    return `{${fields.join(",")}}`;
  }
  const text = JSON.stringify(value);
  return typeof text === "string" ? text : "null";
}

function isJsonObject(value: unknown): value is Readonly<Record<string, unknown>> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Rust `{:.1}` 的有限值显示：总是保留一位小数，恰好半时取偶（与格式化器的舍入一致）。 */
export function formatOneDecimal(value: number): string {
  if (Number.isNaN(value)) {
    return "NaN";
  }
  if (value === Number.POSITIVE_INFINITY) {
    return "inf";
  }
  if (value === Number.NEGATIVE_INFINITY) {
    return "-inf";
  }
  const negative = value < 0 || Object.is(value, -0);
  const magnitude = Math.abs(value);
  const view = new DataView(new ArrayBuffer(8));
  view.setFloat64(0, magnitude, false);
  const bits = view.getBigUint64(0, false);
  const exponentBits = Number((bits >> 52n) & 0x7ffn);
  const fractionBits = bits & 0xffff_ffff_ffff_fn;
  const significand = exponentBits === 0 ? fractionBits : (1n << 52n) | fractionBits;
  const exponent = exponentBits === 0 ? -1074 : exponentBits - 1023 - 52;
  let numerator = significand * 10n;
  let denominator = 1n;
  if (exponent >= 0) {
    numerator <<= BigInt(exponent);
  } else {
    denominator <<= BigInt(-exponent);
  }
  let rounded = numerator / denominator;
  const remainder = numerator % denominator;
  const doubled = remainder * 2n;
  if (doubled > denominator || (doubled === denominator && rounded % 2n === 1n)) {
    rounded += 1n;
  }
  const digits = rounded.toString().padStart(2, "0");
  return `${negative ? "-" : ""}${digits.slice(0, -1)}.${digits.slice(-1)}`;
}
