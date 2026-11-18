// Public 会话 HTTP DTO 与同源请求。
// 原始登录 key 只存在于浏览器表单和一次 HTTPS 请求中；成功后由服务端设置 HttpOnly Cookie，
// 组件只持有安全身份投影。

import {
  type PublicLoginResponse,
  PublicLoginResponseSchema,
  type PublicSessionCurrentResponse as PublicSession,
  PublicSessionCurrentResponseSchema,
} from "@auction/contracts/dto/public/account";
import { PUBLIC_EXPECTED_USER_ID_HEADER } from "@auction/contracts/dto/public/commands";
import { jsonValue } from "@auction/contracts/wire";
import { type TransportResponse, transportErrorCode } from "@auction/web-shared/net/transport";
import { jsonHasExactKeys } from "@auction/web-shared/net/wire";
import { setItem } from "@auction/web-shared/platform/storage";
import { createUuidV4 } from "@auction/web-shared/platform/uuid";
import type { Result } from "@auction/web-shared/result";
import { err, ok } from "@auction/web-shared/result";
import { utf8ByteLength } from "@auction/web-shared/text";
import { validUuid } from "@auction/web-shared/ui/validation";
import * as v from "valibot";
import { sendGet, sendRequest } from "../clock";
import { decodeError, decodeErrorCode, decodeSuccess, object, string } from "../decoder";
import { invalidateAll } from "../swr-cache";
import { invalidate, lock as lockCookie, needsSync, record } from "./cookie";
import { parseStrictRfc3339 } from "./rfc3339";

/** Public 会话的稳定身份投影；认证凭据只存在于服务端设置的 HttpOnly Cookie。 */
export type { PublicSession };

/** 严格 RFC3339 解析也供论坛与偏好响应复用，保持历史上从会话模块导入的路径。 */
export { parseStrictRfc3339 };

/** 标签页间只广播随机修订，不保存用户身份或凭据。 */
export const PUBLIC_SESSION_CHANGE_STORAGE_KEY = "hv.public.session-revision";

/** 会话重读原因。认证失败要先失效旧身份并提示服务端撤销；本浏览器主动退出只同步为匿名。 */
export type SessionRefreshReason = "refresh" | "authentication_failed" | "logged_out";

/** 已知认证变化必须先卸载旧用户命令，阻止继续使用旧确认宿主。 */
export function sessionRefreshReasonInvalidates(reason: SessionRefreshReason): boolean {
  return reason !== "refresh";
}

/** 未到期却失效的认证会话只可能来自服务端撤销；主动退出与普通重读不提示。 */
export function sessionRefreshReasonShowsEndedElsewhere(reason: SessionRefreshReason): boolean {
  return reason === "authentication_failed";
}

/** 外壳会话重读入口。 */
export type PublicSessionCheckTrigger = (reason: SessionRefreshReason) => void;

interface SessionCheckRegistration {
  trigger: PublicSessionCheckTrigger;
}

/** 当前页登记的唯一会话检查入口；后登记的替换先登记的。 */
let registeredSessionCheckTrigger: SessionCheckRegistration | null = null;

/**
 * 登记当前页的会话检查入口，返回注销函数。
 *
 * API 可能由事件处理器启动的任务调用，当时没有组件 owner；因此由会话外壳在挂载时登记，
 * 卸载时注销，不能依赖调用瞬间的响应式上下文。
 */
export function registerPublicSessionCheckTrigger(trigger: PublicSessionCheckTrigger): () => void {
  const registration: SessionCheckRegistration = { trigger };
  registeredSessionCheckTrigger = registration;
  return () => {
    // 后登记的入口已经替换自己时，旧注销函数不得清掉新入口。
    if (registeredSessionCheckTrigger === registration) {
      registeredSessionCheckTrigger = null;
    }
  };
}

/** 认证失败或身份前置条件冲突时，重新读取当前 Cookie 对应的权威身份。 */
export function observePublicSessionFailure(status: number, code: string): void {
  if (status === 401 || code === "session_changed") {
    notifyPublicSessionChange("authentication_failed");
  }
}

/** 通知本页外壳与其他标签页重新读取权威会话；修订值只用于唤醒，不携带身份。 */
export function notifyPublicSessionChange(reason: SessionRefreshReason): void {
  const registration = registeredSessionCheckTrigger;
  if (registration !== null) {
    registration.trigger(reason);
  }
  const revision = createUuidV4();
  setItem(PUBLIC_SESSION_CHANGE_STORAGE_KEY, revision);
}

/** Public 登录失败的稳定投影；界面只根据错误码和安全公开原因选择本地化文案。 */
export interface PublicSessionApiError {
  status: number | null;
  code: string | null;
  publicReason: string | null;
}

/** 构造浏览器本地校验错误，不把用户输入或底层网络错误带入页面。 */
export function publicSessionApiErrorLocal(code: string): PublicSessionApiError {
  return { status: null, code, publicReason: null };
}

function responseOk(response: TransportResponse): boolean {
  return response.status >= 200 && response.status <= 299;
}

function trimRustWhitespace(value: string): string {
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

function hasControlCharacter(value: string): boolean {
  return /\p{Cc}/u.test(value);
}

/** 永久会话必须同时为空；有限会话只接受完整 RFC3339 且截止不得晚于绝对截止。 */
function validSessionDeadlines(
  expiresAt: string | null,
  absoluteExpiresAt: string | null,
): boolean {
  if (expiresAt === null && absoluteExpiresAt === null) {
    return true;
  }
  if (expiresAt === null || absoluteExpiresAt === null) {
    return false;
  }
  const expires = parseStrictRfc3339(expiresAt);
  const absolute = parseStrictRfc3339(absoluteExpiresAt);
  return expires !== null && absolute !== null && expires <= absolute;
}

function validSessionFields(
  userId: string,
  username: string,
  expiresAt: string | null,
  absoluteExpiresAt: string | null,
): boolean {
  return (
    validUuid(userId) &&
    username !== "" &&
    username === trimRustWhitespace(username) &&
    utf8ByteLength(username) <= 256 &&
    !hasControlCharacter(username) &&
    validSessionDeadlines(expiresAt, absoluteExpiresAt)
  );
}

function validCurrentSession(session: PublicSession): boolean {
  return (
    session.authenticated &&
    validSessionFields(
      session.user_id,
      session.username,
      session.expires_at,
      session.absolute_expires_at,
    )
  );
}

function validLoginSession(session: PublicLoginResponse): boolean {
  return (
    session.status === "active" &&
    validSessionFields(
      session.user_id,
      session.username,
      session.expires_at,
      session.absolute_expires_at,
    )
  );
}

const CURRENT_SESSION_KEYS = [
  "authenticated",
  "user_id",
  "username",
  "expires_at",
  "absolute_expires_at",
];
const LOGIN_SESSION_KEYS = ["user_id", "username", "status", "expires_at", "absolute_expires_at"];

function validCurrentSessionValue(value: unknown): boolean {
  return value === null || jsonHasExactKeys(value, CURRENT_SESSION_KEYS);
}

function validLoginSessionValue(value: unknown): boolean {
  return jsonHasExactKeys(value, LOGIN_SESSION_KEYS);
}

function validPublicReason(value: unknown): string | null {
  const data = object(value);
  if (data === null || Object.keys(data).length !== 1) {
    return null;
  }
  if (!Object.hasOwn(data, "public_reason")) {
    return null;
  }
  const reason = string(data.public_reason);
  if (reason === null) {
    return null;
  }
  return reason !== "" &&
    reason === trimRustWhitespace(reason) &&
    utf8ByteLength(reason) <= 1024 &&
    !hasControlCharacter(reason)
    ? reason
    : null;
}

async function parseApiError(response: TransportResponse): Promise<PublicSessionApiError> {
  const status = response.status;
  const decoded = decodeError(response);
  if (!decoded.ok) {
    return { status, code: "invalid_response", publicReason: null };
  }
  const [code, data] = decoded.value;
  const publicReason =
    status === 403 && code === "account_blacklisted" ? validPublicReason(data) : null;
  return { status, code, publicReason };
}

async function decodeCurrentSession(
  response: TransportResponse,
): Promise<Result<PublicSession | null, string>> {
  if (!responseOk(response)) {
    const code = decodeErrorCode(response);
    return code === "unauthenticated" ? ok(null) : err(code);
  }
  const decoded = decodeSuccess(
    response,
    v.nullable(PublicSessionCurrentResponseSchema),
    validCurrentSessionValue,
  );
  if (!decoded.ok) {
    return err(decoded.error);
  }
  const payload = decoded.value;
  if (payload === null) {
    return ok(null);
  }
  return validCurrentSession(payload) ? ok(payload) : err("invalid_response");
}

async function fetchPublicSessionOnce(
  signal: AbortSignal,
): Promise<Result<PublicSession | null, string>> {
  const response = await sendGet("/api/v1/session", signal);
  if (signal.aborted) {
    return err("network_error");
  }
  if (!response.ok) {
    return err(transportErrorCode(response.error));
  }
  return decodeCurrentSession(response.value);
}

/** 在 Cookie 同步锁内执行调用体；锁名称和超时由 cookie 模块唯一拥有。 */
export function lockCookieMutation<T>(
  body: (lockSignal: AbortSignal) => Promise<Result<T, string>>,
  signal: AbortSignal,
): Promise<Result<T, string>> {
  return lockCookie(body, (code) => code, signal);
}

/** 偏好写入可能只提交数据库、未收到响应；下次权威读取须重新核对浏览器持久期。 */
export function invalidateCookieSync(): void {
  invalidate();
}

/**
 * 普通读取不写 Cookie；只有首次读取、截止变化或持久期同步到期时进入显式串行化写路径。
 */
export async function fetchPublicSession(
  signal: AbortSignal,
): Promise<Result<PublicSession | null, string>> {
  const current = await fetchPublicSessionOnce(signal);
  if (signal.aborted) {
    return err("network_error");
  }
  if (!current.ok) {
    return current;
  }
  if (current.value === null) {
    invalidate();
    return ok(null);
  }
  if (!needsSync(current.value)) {
    return ok(current.value);
  }
  return lockCookieMutation(async (lockSignal) => {
    // 等锁期间另一标签页可能已经登录 B；必须重读现 Cookie，不能把外层旧 A 投影交给 UI。
    const lockedCurrent = await fetchPublicSessionOnce(lockSignal);
    if (lockSignal.aborted) {
      return err("network_error");
    }
    if (!lockedCurrent.ok) {
      return lockedCurrent;
    }
    if (lockedCurrent.value === null) {
      invalidate();
      return ok(null);
    }
    if (!needsSync(lockedCurrent.value)) {
      return ok(lockedCurrent.value);
    }
    const currentSession = lockedCurrent.value;
    const response = await sendRequest(
      () =>
        ok({
          url: "/api/v1/session",
          init: {
            method: "PUT",
            headers: { [PUBLIC_EXPECTED_USER_ID_HEADER]: currentSession.user_id },
          },
        }),
      lockSignal,
    );
    if (lockSignal.aborted) {
      return err("network_error");
    }
    if (!response.ok) {
      return err(transportErrorCode(response.error));
    }
    const successful = responseOk(response.value);
    const synchronized = await decodeCurrentSession(response.value);
    if (lockSignal.aborted) {
      return err("network_error");
    }
    if (!synchronized.ok) {
      return synchronized;
    }
    const identity = synchronized.value;
    if (identity !== null && identity.user_id === currentSession.user_id) {
      record(identity);
      return ok(identity);
    }
    if (identity === null && !successful) {
      invalidate();
      return ok(null);
    }
    return err("invalid_response");
  }, signal);
}

/** 使用登录 key 登录；成功后在锁内失效公开读取缓存、解码身份并记录 Cookie 生命周期。 */
export async function loginPublic(
  key: string,
  signal: AbortSignal,
): Promise<Result<PublicSession, PublicSessionApiError>> {
  return lockCookie(
    async (lockSignal) => {
      invalidate();
      const response = await sendRequest(
        () =>
          ok({
            url: "/api/v1/session",
            init: {
              method: "POST",
              headers: { "content-type": "application/json" },
              body: JSON.stringify({ key }),
            },
          }),
        lockSignal,
      );
      if (lockSignal.aborted) {
        return err(publicSessionApiErrorLocal("network_error"));
      }
      if (!response.ok) {
        return err(publicSessionApiErrorLocal(transportErrorCode(response.error)));
      }
      if (!responseOk(response.value)) {
        const parsed = await parseApiError(response.value);
        if (lockSignal.aborted) {
          return err(publicSessionApiErrorLocal("network_error"));
        }
        return err(parsed);
      }
      // Cookie 已由响应写入；即使正文解码失败也重读。重登同账号不能清除原未决命令。
      notifyPublicSessionChange("refresh");
      // 响应成功即可能已换身份；公开读取缓存必须在正文校验前失效。
      invalidateAll();
      const decoded = decodeSuccess(
        response.value,
        PublicLoginResponseSchema,
        validLoginSessionValue,
      );
      if (!decoded.ok || !validLoginSession(decoded.value)) {
        return err(publicSessionApiErrorLocal("invalid_response"));
      }
      const payload = decoded.value;
      const session: PublicSession = {
        authenticated: true,
        user_id: payload.user_id,
        username: payload.username,
        expires_at: payload.expires_at,
        absolute_expires_at: payload.absolute_expires_at,
      };
      record(session);
      return ok(session);
    },
    (code) => publicSessionApiErrorLocal(code),
    signal,
  );
}

/** 退出当前浏览器会话；服务器失败时按会话错误通知其他页面，成功时清除全部公开缓存。 */
export async function logoutPublic(
  userId: string,
  signal: AbortSignal,
): Promise<Result<null, string>> {
  return lockCookie(
    async (lockSignal) => {
      invalidate();
      const response = await sendRequest(
        () =>
          ok({
            url: "/api/v1/session",
            init: {
              method: "DELETE",
              headers: { [PUBLIC_EXPECTED_USER_ID_HEADER]: userId },
            },
          }),
        lockSignal,
      );
      if (lockSignal.aborted) {
        return err("network_error");
      }
      if (!response.ok) {
        return err(transportErrorCode(response.error));
      }
      if (!responseOk(response.value)) {
        const status = response.value.status;
        const code = decodeErrorCode(response.value);
        observePublicSessionFailure(status, code);
        return err(code);
      }
      // 主动退出：其他标签页同步为匿名，但不提示被其他设备挤下线。
      notifyPublicSessionChange("logged_out");
      const decoded = decodeSuccess(response.value, jsonValue, (value) => value === null);
      if (!decoded.ok) {
        return err(decoded.error);
      }
      // 身份变化后登录期间的公开读取缓存全部过期：下一次读取先向服务端再验证。
      invalidateAll();
      return ok(null);
    },
    (code) => code,
    signal,
  );
}
