// Public API 的服务端时间校准与同源请求边界。
// 发送、超时取消与错误分类统一由 `@auction/web-shared` 的传输层承担（10 秒取消上限、精确 URL、
// 结构化 `TransportError`）；本模块只保留 Public 特有的服务端时钟采样。

import type {
  TransportError,
  TransportRequestBuilder,
  TransportResponse,
  TransportResponseMetadata,
} from "@auction/web-shared/net/transport";
import { sendGetObserved, sendRequestObserved } from "@auction/web-shared/net/transport";
import type { Result } from "@auction/web-shared/result";
import { responseReportsCurrentBuild } from "@auction/web-shared/ui/build-notice";

// 旧版以响应 Date 头估算服务端时钟，避免客户端本地时钟偏移导致竞价窗口显示错误。
const SERVER_DATE_CENTER_MILLIS = 500;
const SERVER_OFFSET_EMA_NUMERATOR = 3;
const SERVER_OFFSET_EMA_DENOMINATOR = 10;
let SERVER_OFFSET_MILLIS = 0;
let SERVER_OFFSET_KNOWN = false;

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
const IMF_FIXDATE_LENGTH = 29;

// 与 `auction-time` 的 jiff Timestamp 边界一致：-9999-01-02T01:59:59Z 至
// 9999-12-30T22:00:00.999999999Z；毫秒值按 floor 计算。
const TIMESTAMP_MIN_MILLIS = utcBoundMillis(-9999, 1, 2, 1, 59, 59, 0);
const TIMESTAMP_MAX_MILLIS = utcBoundMillis(9999, 12, 30, 22, 0, 0, 999);

/** 响应的 `Date` 能否代表此刻的服务端时钟：只有不可能是缓存重放的响应可以。
 *
 * 可缓存的响应会连同生成时的 `Date` 一起被重放，历史类读取最长保存 5 分钟、已结算内容保存
 * 一天；把它当作当前时间会让时钟偏移整体后退，倒计时与开拍刷新随之变慢，首个样本还会被直接
 * 采用。`no-store` 的响应任何缓存都不得保存（与构建号采信同一条规则）。`private` 的响应共享
 * 缓存不得保存，但浏览器自己可以重放，因此只在请求已绕过浏览器 HTTP 缓存时采信。
 * 仅供测试使用。 */
export function responseDateIsCurrent(
  cacheControl: string | null,
  bypassedBrowserCache: boolean,
): boolean {
  if (responseReportsCurrentBuild(cacheControl)) {
    return true;
  }
  return (
    bypassedBrowserCache &&
    (cacheControl
      ?.split(",")
      .some((directive) => asciiEqualsIgnoreCase(rustTrim(directive), "private")) ??
      false)
  );
}

/** 记录响应中的服务端时间样本；无效或缺失的 Date 头不会影响当前请求。
 * 用于可能由浏览器 HTTP 缓存直接应答的请求，只采信 `no-store` 响应。 */
function observeResponseDate(response: TransportResponseMetadata): void {
  recordResponseDate(response, false);
}

/** 再验证请求（脚本自带 `If-None-Match` 或 `no-cache` 模式）一定到达服务端，
 * 进行中场次的 `private` 目录响应在这里同样是当前样本。 */
function observeRevalidatedDate(response: TransportResponseMetadata): void {
  recordResponseDate(response, true);
}

function recordResponseDate(
  response: TransportResponseMetadata,
  bypassedBrowserCache: boolean,
): void {
  const cacheControl = response.headers.get("cache-control");
  if (responseDateIsCurrent(cacheControl, bypassedBrowserCache)) {
    recordServerDate(response.headers.get("Date"));
  }
}

/** 把一个当前的 `Date` 样本并入时钟偏移：首个样本直接采用，其后按指数滑动平均收敛。 */
function recordServerDate(rawDate: string | null): void {
  if (rawDate === null) {
    return;
  }
  const serverDate = parseImfFixdate(rawDate);
  if (serverDate === null) {
    return;
  }
  const browserNow = Date.now();
  if (
    !Number.isFinite(browserNow) ||
    browserNow < Number.MIN_SAFE_INTEGER ||
    browserNow > Number.MAX_SAFE_INTEGER
  ) {
    return;
  }
  // Rust 的 `browser_now as i64` 先向零截断；浏览器墙钟通常是整数，但保留同一取整规则。
  const sample = serverDate + SERVER_DATE_CENTER_MILLIS - Math.trunc(browserNow);
  if (!Number.isSafeInteger(sample)) {
    return;
  }
  if (!SERVER_OFFSET_KNOWN) {
    SERVER_OFFSET_KNOWN = true;
    SERVER_OFFSET_MILLIS = sample;
    return;
  }
  const delta = sample - SERVER_OFFSET_MILLIS;
  if (!Number.isSafeInteger(delta)) {
    return;
  }
  const scaled = delta * SERVER_OFFSET_EMA_NUMERATOR;
  if (!Number.isSafeInteger(scaled)) {
    return;
  }
  const adjusted = SERVER_OFFSET_MILLIS + Math.trunc(scaled / SERVER_OFFSET_EMA_DENOMINATOR);
  if (!Number.isSafeInteger(adjusted)) {
    return;
  }
  SERVER_OFFSET_MILLIS = adjusted;
}

/** 返回按服务端时钟修正后的当前时间（Unix 毫秒）；校准失败时重读浏览器系统时钟，读数仍不可
 * 表示时返回 Unix 纪元 `0`。 */
export function estimatedNow(): number {
  const browserNow = Date.now();
  if (
    Number.isFinite(browserNow) &&
    browserNow >= Number.MIN_SAFE_INTEGER &&
    browserNow <= Number.MAX_SAFE_INTEGER
  ) {
    const corrected = Math.trunc(browserNow) + SERVER_OFFSET_MILLIS;
    if (
      Number.isSafeInteger(corrected) &&
      corrected >= TIMESTAMP_MIN_MILLIS &&
      corrected <= TIMESTAMP_MAX_MILLIS
    ) {
      return corrected;
    }
  }
  // 与 `Timestamp::now()` 的浏览器分支一致：读数不可表示时退回 Unix 纪元，而不是把无效值交给
  // 页面。校准后的时刻越界时同样重新读一次系统时钟，只有它也不可表示才归零。
  return systemNowMillis();
}

/** 读取一次可直接映射为 jiff Timestamp 的浏览器墙钟毫秒；越界或非有限值返回 Unix 纪元。 */
function systemNowMillis(): number {
  const browserNow = Date.now();
  if (
    !Number.isFinite(browserNow) ||
    browserNow < Number.MIN_SAFE_INTEGER ||
    browserNow > Number.MAX_SAFE_INTEGER
  ) {
    return 0;
  }
  const truncated = Math.trunc(browserNow);
  if (
    !Number.isSafeInteger(truncated) ||
    truncated < TIMESTAMP_MIN_MILLIS ||
    truncated > TIMESTAMP_MAX_MILLIS
  ) {
    return 0;
  }
  return truncated;
}

/** 发送 GET 并在读取 DTO 前采集服务端时钟；所有 Public 读取共用该边界。
 * 结构化错误直接向调用方传播，由调用方用 `transportErrorCode` 折回稳定 transport code。
 * 调用方可在正文读取前观察 HTTP 状态。
 * `signal` 中止时共享传输层同时取消 `fetch` 与正文读取。 */
export function sendGet(
  url: string,
  signal: AbortSignal,
  observeStatus?: (status: number) => void,
): Promise<Result<TransportResponse, TransportError>> {
  if (observeStatus === undefined) {
    return sendGetObserved(url, observeResponseDate, signal);
  }
  return sendGetObserved(
    url,
    (response) => {
      observeStatus(response.status);
      observeResponseDate(response);
    },
    signal,
  );
}

/** 发送 SWR 缓存层的公开 GET：有 ETag 时携带 `If-None-Match`，没有时要求浏览器向服务端再验证。
 * 304 与 200 一样在共享预算内读完响应头，正文与缓存判定由调用方处理。
 *
 * 这两种请求都不会由浏览器 HTTP 缓存直接应答：脚本自带条件头的请求按 Fetch 规范绕过它，过期
 * 或被失效（实时事件、命令结果）的条目都走这条路；无 ETag 的读取发生在缓存未命中或旧条目没有
 * ETag 时，此时必须拿到服务端的当前快照。公开目录响应带 `max-age=2,
 * stale-while-revalidate=5`，默认缓存模式下 7 秒内的第二次读取会原样拿回旧正文，并被 SWR 层
 * 当作新快照保留到 TTL 结束；`no-cache` 模式仍让浏览器用自己的验证器做条件请求，未变化时只
 * 传回 304。 */
export function sendGetConditional(
  url: string,
  etag: string | null,
  observeStatus: (status: number) => void,
  signal: AbortSignal,
): Promise<Result<TransportResponse, TransportError>> {
  return sendRequestObserved(
    () => {
      if (etag === null) {
        return {
          ok: true,
          value: { url, init: { method: "GET", cache: "no-cache" } },
        };
      }
      return {
        ok: true,
        value: { url, init: { method: "GET", headers: { "If-None-Match": etag } } },
      };
    },
    (response) => {
      observeStatus(response.status);
      observeRevalidatedDate(response);
    },
    signal,
  );
}

/** 在响应头到达时校时，并在共享总预算内完整读取命令响应正文。 */
export function sendRequest(
  build: TransportRequestBuilder,
  signal: AbortSignal,
): Promise<Result<TransportResponse, TransportError>> {
  return sendRequestObserved(build, observeResponseDate, signal);
}

/** 解析 IMF-fixdate；形状、取值范围或星期与日期不符时返回 `null`。 */
function parseImfFixdate(text: string): number | null {
  if (text.length !== IMF_FIXDATE_LENGTH) {
    return null;
  }
  // Rust 先按字节切片；只有 ASCII 时 UTF-16 位置才与字节位置一致。
  for (let index = 0; index < IMF_FIXDATE_LENGTH; index += 1) {
    if (text.charCodeAt(index) > 0x7f) {
      return null;
    }
  }
  const part = (start: number, end: number): string => text.slice(start, end);
  const number = (start: number, end: number): number | null => {
    const digits = part(start, end);
    for (let index = 0; index < digits.length; index += 1) {
      const code = digits.charCodeAt(index);
      if (code < 0x30 || code > 0x39) {
        return null;
      }
    }
    return Number(digits);
  };
  const separators =
    part(3, 5) === ", " &&
    part(7, 8) === " " &&
    part(11, 12) === " " &&
    part(16, 17) === " " &&
    part(19, 20) === ":" &&
    part(22, 23) === ":" &&
    part(25, 29) === " GMT";
  if (!separators) {
    return null;
  }
  const weekdayName = part(0, 3);
  const weekday = IMF_WEEKDAYS.indexOf(weekdayName);
  if (weekday < 0) {
    return null;
  }
  const monthName = part(8, 11);
  const monthIndex = IMF_MONTHS.indexOf(monthName);
  if (monthIndex < 0) {
    return null;
  }
  const year = number(12, 16);
  const day = number(5, 7);
  const hour = number(17, 19);
  const minute = number(20, 22);
  const second = number(23, 25);
  if (year === null || day === null || hour === null || minute === null || second === null) {
    return null;
  }
  const parsed = utcCivilMillis(year, monthIndex + 1, day, hour, minute, second, 0);
  if (parsed === null) {
    return null;
  }
  if (parsed < TIMESTAMP_MIN_MILLIS || parsed > TIMESTAMP_MAX_MILLIS) {
    return null;
  }
  const date = new Date(parsed);
  const actualWeekday = (date.getUTCDay() + 6) % 7;
  return actualWeekday === weekday ? parsed : null;
}

/** 校验年月日时分秒毫秒并返回 UTC 毫秒；日期或时间无效时返回 `null`。 */
function utcCivilMillis(
  year: number,
  month: number,
  day: number,
  hour: number,
  minute: number,
  second: number,
  millisecond: number,
): number | null {
  if (
    month < 1 ||
    month > 12 ||
    hour < 0 ||
    hour > 23 ||
    minute < 0 ||
    minute > 59 ||
    second < 0 ||
    second > 59 ||
    millisecond < 0 ||
    millisecond > 999
  ) {
    return null;
  }
  const date = new Date(0);
  date.setUTCFullYear(year, month - 1, day);
  date.setUTCHours(hour, minute, second, millisecond);
  if (
    date.getUTCFullYear() !== year ||
    date.getUTCMonth() !== month - 1 ||
    date.getUTCDate() !== day ||
    date.getUTCHours() !== hour ||
    date.getUTCMinutes() !== minute ||
    date.getUTCSeconds() !== second ||
    date.getUTCMilliseconds() !== millisecond
  ) {
    return null;
  }
  const millis = date.getTime();
  return Number.isSafeInteger(millis) ? millis : null;
}

/** 已知合法的 Timestamp 边界；用于表达 jiff 的可表示范围，并拒绝越界的 IMF 日期。 */
function utcBoundMillis(
  year: number,
  month: number,
  day: number,
  hour: number,
  minute: number,
  second: number,
  millisecond: number,
): number {
  const date = new Date(0);
  date.setUTCFullYear(year, month - 1, day);
  date.setUTCHours(hour, minute, second, millisecond);
  return date.getTime();
}

function rustTrim(value: string): string {
  let start = 0;
  while (start < value.length && isRustWhitespace(value.charCodeAt(start))) {
    start += 1;
  }
  let end = value.length;
  while (end > start && isRustWhitespace(value.charCodeAt(end - 1))) {
    end -= 1;
  }
  return value.slice(start, end);
}

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

function asciiEqualsIgnoreCase(left: string, right: string): boolean {
  if (left.length !== right.length) {
    return false;
  }
  for (let index = 0; index < left.length; index += 1) {
    let leftCode = left.charCodeAt(index);
    let rightCode = right.charCodeAt(index);
    if (leftCode >= 65 && leftCode <= 90) {
      leftCode += 32;
    }
    if (rightCode >= 65 && rightCode <= 90) {
      rightCode += 32;
    }
    if (leftCode !== rightCode) {
      return false;
    }
  }
  return true;
}
