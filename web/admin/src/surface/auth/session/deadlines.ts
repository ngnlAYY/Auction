// 根据服务端剩余时长与单调时钟安排会话复核。

import type { AdminSession } from "../../../api/session";

const SESSION_CHECK_MARGIN_MS = 1_000;
const MIN_SESSION_CHECK_DELAY_MS = 2_000;

/** 最早截止时间与本次接收服务端时刻之间的毫秒数。 */
export function sessionLifetimeMillis(session: AdminSession): number {
  // 完整纳秒相减后再向零截断；先截断两边会改变毫秒差与投影排序。
  return Number((sessionDeadline(session) - session.serverTime) / 1_000_000n);
}

function sessionDeadline(session: AdminSession): bigint {
  if (session.expiresAt < session.absoluteExpiresAt) {
    return session.expiresAt;
  }
  return session.absoluteExpiresAt;
}

/** 同一代际的截止只向后更新；截止相同时用较晚的服务端观察时刻校准。 */
export function isNewerProjection(candidate: AdminSession, current: AdminSession): boolean {
  const candidateDeadline = sessionDeadline(candidate);
  const currentDeadline = sessionDeadline(current);
  if (candidateDeadline !== currentDeadline) {
    return candidateDeadline > currentDeadline;
  }
  // 同一毫秒内的新投影仍按完整纳秒比较，不能被判为相等。
  return candidate.serverTime > current.serverTime;
}

/** 在截止前安排一次会话复核；剩余时间过短时等到截止之后。 */
export function sessionCheckDelay(
  lifetimeMillis: number,
  receivedAtMillis: number,
  nowMillis: number,
): number {
  const remaining = lifetimeMillis - f64Max(nowMillis - receivedAtMillis, 0);
  const beforeDeadline = remaining - SESSION_CHECK_MARGIN_MS;
  // 截止将至时等到截止之后，避免成功 GET 返回短剩余时长后再立即查一次。
  const delay =
    beforeDeadline >= MIN_SESSION_CHECK_DELAY_MS
      ? beforeDeadline
      : f64Max(remaining + SESSION_CHECK_MARGIN_MS, MIN_SESSION_CHECK_DELAY_MS);
  const bounded = f64Min(Math.ceil(delay), 2_147_483_647);
  return bounded <= 0 ? 0 : Math.trunc(bounded);
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

/** Rust `f64::min`：任一参数为 NaN 时取另一个。 */
function f64Min(left: number, right: number): number {
  if (Number.isNaN(left)) {
    return right;
  }
  if (Number.isNaN(right)) {
    return left;
  }
  return Math.min(left, right);
}
