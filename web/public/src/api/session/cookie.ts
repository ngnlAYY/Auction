// Public Cookie 写入的原生锁与常量规模同步提示；提示不持有 token，也不决定认证或服务器期限。

import type { PublicSessionCurrentResponse as PublicSession } from "@auction/contracts/dto/public/account";
import { DEFAULT_REQUEST_TIMEOUT_MILLIS } from "@auction/web-shared/net/transport";
import { withBrowserLock } from "@auction/web-shared/net/transport/browser-lock";
import type { Result } from "@auction/web-shared/result";
import { err, unreachable } from "@auction/web-shared/result";
import { notifyPublicSessionChange } from "./revision";

const LOCK_NAME = "hv.public.session-cookie";
const SYNC_INTERVAL_MILLIS = 24 * 60 * 60 * 1_000;

interface SyncedCookie {
  userId: string;
  expiresAt: string | null;
  absoluteExpiresAt: string | null;
  synchronizedAt: number;
}

let syncedCookie: SyncedCookie | null = null;

function lockErrorCode(error: "unavailable" | "timeout"): string {
  switch (error) {
    case "unavailable":
      return "session_lock_unavailable";
    case "timeout":
      return "session_lock_timeout";
    default:
      return unreachable(error);
  }
}

/**
 * 登录、退出、偏好命令和浏览器持久期同步共用同一原生独占锁。
 *
 * 浏览器的锁只提供闭包作用域，因此这里把调用体放进锁内执行，等价于
 * Rust 中持有 `BrowserLock` 守卫直到调用方离开作用域。
 */
export async function withCookieLock<T>(
  body: (lockSignal: AbortSignal) => Promise<T>,
  signal: AbortSignal,
): Promise<Result<T, string>> {
  const acquired = await withBrowserLock(LOCK_NAME, DEFAULT_REQUEST_TIMEOUT_MILLIS, signal, body);
  return acquired.ok ? acquired : err(lockErrorCode(acquired.error));
}

/** 登录时长命令经此入口持锁；传输失败只核对 Cookie，保留调用方的原命令结果和重试规则。 */
export async function lock<T, E extends { readonly status: number }>(
  body: (lockSignal: AbortSignal) => Promise<Result<T, E>>,
  mapLockError: (code: string) => E,
  signal: AbortSignal,
): Promise<Result<T, E>> {
  const acquired = await withCookieLock(body, signal);
  if (!acquired.ok) {
    return err(mapLockError(acquired.error));
  }
  const result = acquired.value;
  // 命令体在取得锁后直接发送。0 表示发出后无完整传输结果，5xx 也不能否定已提交的修改。
  // 完整的 2xx 由命令体通知（包括解码失败），避免同一次写入唤醒两次会话读取。
  if (!result.ok && (result.error.status === 0 || result.error.status >= 500)) {
    notifyPublicSessionChange("cookie_changed");
  }
  return result;
}

/**
 * 首次或期限变化须同步；其余在下一次权威读取且距上次同步满一天时刷新浏览器持久期。
 * 此判断只在本次权威 GET 已认证后调用；提示丢失只增加一次显式同步，不能授权用户。
 */
export function needsSync(session: PublicSession): boolean {
  const now = Date.now();
  return (
    syncedCookie === null ||
    syncedCookie.userId !== session.user_id ||
    syncedCookie.expiresAt !== session.expires_at ||
    syncedCookie.absoluteExpiresAt !== session.absolute_expires_at ||
    !Number.isFinite(now) ||
    now < syncedCookie.synchronizedAt ||
    now - syncedCookie.synchronizedAt >= SYNC_INTERVAL_MILLIS
  );
}

/** 只在完整成功响应及安全投影解码后记录；网络失败或组件取消不声称 Cookie 已同步。 */
export function record(session: PublicSession): void {
  const now = Date.now();
  syncedCookie = Number.isFinite(now)
    ? {
        userId: session.user_id,
        expiresAt: session.expires_at,
        absoluteExpiresAt: session.absolute_expires_at,
        synchronizedAt: now,
      }
    : null;
}

/** 写入尝试先撤掉提示；未知结果后由新的权威读取或原命令重放同步真实期限。 */
export function invalidate(): void {
  syncedCookie = null;
}
