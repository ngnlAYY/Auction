// 按人工操作合并 Admin 会话续期请求。
// 续期只由已确认会话与最新输入驱动；限流等待可以超过普通合并间隔，但仍须早于会话截止。

import type { AdminSession } from "../../api/session";
import { sessionLifetimeMillis } from "./session/deadlines";

/** 注销按钮选择器；输入监听用它把注销点击排除在续期活动之外。 */
export const LOGOUT_BUTTON_CLASS = "admin-shell__logout";

const MAX_RENEWAL_INTERVAL_MS = 60_000;
const MIN_RENEWAL_INTERVAL_MS = 5_000;

/** 最近一次登录或续期确定的合并间隔与单调时钟基准。 */
export interface RenewalTiming {
  readonly sinceMillis: number;
  readonly intervalMillis: number;
}

/** 从服务端剩余时长与本次接收基准建立续期节奏。 */
export function renewalTimingFromSession(session: AdminSession): RenewalTiming {
  return {
    sinceMillis: session.receivedAtMillis,
    intervalMillis: renewalIntervalMillis(sessionLifetimeMillis(session)),
  };
}

/** 合并间隔取最早截止时间的三分之一，并限制在 5 到 60 秒之间。 */
function renewalIntervalMillis(lifetimeMillis: number): number {
  return Math.min(Math.max(lifetimeMillis / 3, MIN_RENEWAL_INTERVAL_MS), MAX_RENEWAL_INTERVAL_MS);
}

/** 会话在单调时钟上的预计到期时刻；不依赖本机墙钟校准。 */
export function estimatedExpirationMillis(session: AdminSession): number {
  return session.receivedAtMillis + sessionLifetimeMillis(session);
}

/**
 * 在截止前安排一次续期。
 * 没有输入时不续期；限流未解除时等到最早重试时间。普通输入必须能在最新活动后
 * 一个合并窗口内到期，避免陈旧输入跨越截止时间触发续期。
 */
export function activityDelay(
  timing: RenewalTiming | null,
  latestInput: number | null,
  now: number,
  expiresAt: number,
  retryNotBefore: number,
): number | null {
  if (latestInput === null) {
    return null;
  }
  const deferredByRateLimit = latestInput < retryNotBefore;
  const scheduled = deferredByRateLimit
    ? retryNotBefore
    : timing === null
      ? now
      : timing.sinceMillis + timing.intervalMillis;
  // Rust `f64::max` 在任一参数为 NaN 时取另一个；`Math.max` 会把 NaN 传播到截止比较。
  const delay = Math.ceil(f64Max(scheduled - now, 0));
  const dueAt = now + delay;
  // 限流可能要求等待超过 60 秒。已排队的新输入只要赶得上会话截止就仍可派发。
  if (
    dueAt < expiresAt &&
    (deferredByRateLimit || dueAt - latestInput <= MAX_RENEWAL_INTERVAL_MS)
  ) {
    return saturatingU32(delay);
  }
  return null;
}

/** Rust `f64::max`：任一参数为 NaN 时取另一个。 */
function f64Max(left: number, right: number): number {
  if (Number.isNaN(left)) {
    return right;
  }
  if (Number.isNaN(right)) {
    return left;
  }
  return Math.max(left, right);
}

/** Rust `f64 as u32`：负值归零，超出范围饱和，NaN 归零。 */
function saturatingU32(value: number): number {
  if (Number.isNaN(value) || value <= 0) {
    return 0;
  }
  if (value >= 0xffff_ffff) {
    return 0xffff_ffff;
  }
  return Math.trunc(value);
}
