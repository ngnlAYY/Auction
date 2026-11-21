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
import {
  type TransportError,
  type TransportRequestBuilder,
  type TransportResponse,
  transportErrorCode,
} from "@auction/web-shared/net/transport";
import { type JsonFieldPolicy, jsonHasExactKeys } from "@auction/web-shared/net/wire";
import type { Result } from "@auction/web-shared/result";
import { err, ok } from "@auction/web-shared/result";
import { utf8ByteLength } from "@auction/web-shared/text";
import { reportBrowserError } from "@auction/web-shared/ui/bootstrap";
import { validUuid } from "@auction/web-shared/ui/validation";
import * as v from "valibot";
import { sendRequest } from "../clock";
import { decodeError, decodeErrorCode, decodeSuccess, object, string } from "../decoder";
import { invalidate, needsSync, record, withCookieLock } from "./cookie";
import {
  type LogoutReason,
  markPublicLogin,
  markPublicLogout,
  markPublicSessionChange,
  notifyPublicSessionChange,
  type PublicSessionIdentityTrigger,
  publicSessionIdentityRevision,
  publicSessionPrimaryRevision,
  publicSessionRevisions,
  publicWatchCacheRevision,
  publishPublicSessionIdentity,
  requestPublicSessionCheck,
  type SessionRevisions,
  startPublicSessionIdentityObservation,
} from "./revision";
import { parseStrictRfc3339 } from "./rfc3339";

const SESSION_PATH = "/api/v1/session";

export {
  notifyPublicSessionChange,
  PUBLIC_SESSION_CHANGE_STORAGE_KEY,
  type PublicSessionCheckTrigger,
  registerPublicSessionCheckTrigger,
  requestPublicSessionCheck,
  type SessionRefreshReason,
} from "./revision";
/** Public 会话的稳定身份投影；认证凭据只存在于服务端设置的 HttpOnly Cookie。 */
export type { PublicSession };
/** 严格 RFC3339 解析也供论坛与偏好响应复用，保持历史上从会话模块导入的路径。 */
export { parseStrictRfc3339 };

/** 认证失败或身份前置条件冲突时，重新读取当前 Cookie 对应的权威身份。 */
export function observePublicSessionFailure(
  status: number,
  code: string,
  expectedUser?: string,
): void {
  if (status === 409 && code === "session_changed" && expectedUser !== undefined) {
    publishPublicSessionIdentity({ kind: "expected-user-rejected", userId: expectedUser });
  }
  if (status === 401 || (status === 409 && code === "session_changed")) {
    notifyPublicSessionChange("authentication_failed");
  }
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

function parseApiError(response: TransportResponse): PublicSessionApiError {
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

/** 能证明当前归属的响应必须是约定的 200 / ok，再按路由严格解码投影。 */
export function decodeIdentitySuccess<TSchema extends v.GenericSchema>(
  response: TransportResponse,
  schema: TSchema,
  validate: (value: unknown) => boolean,
  policy?: JsonFieldPolicy,
): Result<v.InferOutput<TSchema>, string> {
  const payload = response.json(policy);
  if (response.status !== 200 || !payload.ok || object(payload.value)?.code !== "ok") {
    reportBrowserError("dto_decode");
    return err("invalid_response");
  }
  // 复用共享信封解码，整场标记等正文只解析一次。
  return decodeSuccess({ ...response, json: () => payload }, schema, validate);
}

function decodeCurrentSession(response: TransportResponse): Result<PublicSession | null, string> {
  if (!responseOk(response)) {
    return err(decodeErrorCode(response));
  }
  const decoded = decodeIdentitySuccess(
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
  const response = await sendRequest(
    () => ok({ url: SESSION_PATH, init: { method: "GET", cache: "no-store" } }),
    signal,
  );
  if (signal.aborted) {
    return err("network_error");
  }
  if (!response.ok) {
    return err(transportErrorCode(response.error));
  }
  return decodeCurrentSession(response.value);
}

/** 偏好写入可能只提交数据库、未收到响应；下次权威读取须重新核对浏览器持久期。 */
export function invalidateCookieSync(): void {
  invalidate();
}

interface SessionObservation {
  readonly result: Result<PublicSession | null, string>;
  readonly revisions: SessionRevisions;
  readonly identityRevision: number | null;
  readonly watchCacheRebase?: {
    readonly userId: string;
    readonly fromRevision: string | null;
    readonly toRevision: string | null;
  } | null;
}

/** 在同一锁内取得当前身份和退出修订；普通复核需要同步时复用锁内的 GET。 */
export async function fetchPublicSession(
  signal: AbortSignal,
  synchronizeCookie: boolean,
  applyIdentity: PublicSessionIdentityTrigger,
): Promise<SessionObservation> {
  // 等锁失败也是本轮对账的结束，须交回排队时已经观察到的退出修订供外壳消费。
  const queuedRevisions = publicSessionRevisions();
  const queuedIdentityRevision = publicSessionIdentityRevision();
  const locked = await withCookieLock(async (lockSignal): Promise<SessionObservation> => {
    const revisions = publicSessionRevisions();
    let identityRevision = publicSessionIdentityRevision();
    let watchCacheRebase: SessionObservation["watchCacheRebase"] = null;
    const publish = (session: PublicSession | null, cookieWritten: boolean): void => {
      identityRevision = publishPublicSessionIdentity(
        { kind: "session", session },
        { revisions, cookieWritten, apply: applyIdentity },
      );
    };
    const current = await fetchPublicSessionOnce(lockSignal);
    if (lockSignal.aborted) {
      return { result: err("network_error"), revisions, identityRevision };
    }
    if (revisions.primary !== publicSessionPrimaryRevision()) {
      return { result: current, revisions, identityRevision };
    }
    if (current.ok) {
      // GET 已证明 Cookie 归属，后续持久期同步即使失败也不能保留旧用户宿主。
      publish(current.value, false);
      if (current.value === null) {
        invalidate();
      }
    }
    // 等锁期间 Cookie 可能被其他标签页替换，只能用锁内刚读到的身份同步持久期。
    let result = current;
    if (synchronizeCookie && current.ok && current.value !== null && needsSync(current.value)) {
      const userId = current.value.user_id;
      result = await synchronizePublicCookie(
        current.value,
        lockSignal,
        publish,
        (fromRevision, toRevision) => {
          watchCacheRebase = { userId, fromRevision, toRevision };
        },
      );
    }
    return { result, revisions, identityRevision, watchCacheRebase };
  }, signal);
  if (signal.aborted) {
    return { result: err("network_error"), revisions: queuedRevisions, identityRevision: null };
  }
  if (locked.ok) {
    return locked.value;
  }
  if (locked.error !== "session_lock_unavailable") {
    return {
      result: err(locked.error),
      revisions: queuedRevisions,
      identityRevision: queuedIdentityRevision,
    };
  }
  // 无 Web Locks 时仍可确认匿名；需要同步的身份继续失败关闭。
  const revisions = publicSessionRevisions();
  const startedRevision = publicSessionIdentityRevision();
  const observe = startPublicSessionIdentityObservation();
  const current = await fetchPublicSessionOnce(signal);
  if (signal.aborted) {
    return { result: err("network_error"), revisions, identityRevision: null };
  }
  const identityRevision = current.ok
    ? observe({ kind: "session", session: current.value }, { revisions, apply: applyIdentity })
    : startedRevision;
  if (identityRevision !== null && current.ok && current.value === null) {
    invalidate();
  }
  const result =
    synchronizeCookie && current.ok && current.value !== null && needsSync(current.value)
      ? err(locked.error)
      : current;
  return { result, revisions, identityRevision };
}

async function synchronizePublicCookie(
  current: PublicSession,
  signal: AbortSignal,
  publish: (session: PublicSession | null, cookieWritten: boolean) => void,
  onReschedule: (fromRevision: string | null, toRevision: string | null) => void,
): Promise<Result<PublicSession | null, string>> {
  const response = await sendRequest(
    () =>
      ok({
        url: SESSION_PATH,
        init: { method: "PUT", headers: { [PUBLIC_EXPECTED_USER_ID_HEADER]: current.user_id } },
      }),
    signal,
  );
  if (signal.aborted) {
    if (mayHaveChangedCookie(response)) {
      rescheduleCookieSync(onReschedule);
    }
    return err("network_error");
  }
  if (!response.ok) {
    if (mayHaveChangedCookie(response)) {
      rescheduleCookieSync(onReschedule);
    }
    return err(transportErrorCode(response.error));
  }
  const synchronized = decodeCurrentSession(response.value);
  if (!synchronized.ok) {
    if (response.value.status === 409) {
      observePublicSessionFailure(409, synchronized.error, current.user_id);
    } else if (mayHaveChangedCookie(response)) {
      rescheduleCookieSync(onReschedule);
    }
    return synchronized;
  }
  const identity = synchronized.value;
  if (identity === null || identity.user_id !== current.user_id) {
    rescheduleCookieSync(onReschedule);
    return err("invalid_response");
  }
  record(identity);
  publish(identity, true);
  return synchronized;
}

function rescheduleCookieSync(
  onReschedule: (fromRevision: string | null, toRevision: string | null) => void,
): void {
  invalidate();
  const fromRevision = publicWatchCacheRevision();
  markPublicSessionChange();
  onReschedule(fromRevision, publicWatchCacheRevision());
  requestPublicSessionCheck("cookie_changed");
}

function mayHaveChangedCookie(response: Result<TransportResponse, TransportError>): boolean {
  if (response.ok) {
    return responseOk(response.value) || response.value.status >= 500;
  }
  const error = response.error;
  return (
    error.kind !== "network" || error.status === null || error.status < 400 || error.status >= 500
  );
}

async function sendCookieMutation<T, E>(
  build: TransportRequestBuilder,
  signal: AbortSignal,
  method: "POST" | "DELETE",
  mapError: (code: string) => E,
  decodeReceipt: (response: TransportResponse) => Result<T, E>,
): Promise<Result<T, E>> {
  const change: { reason: "cookie_changed" | LogoutReason | null } = { reason: null };
  const locked = await withCookieLock(async (lockSignal) => {
    if (lockSignal.aborted) {
      return err(mapError("network_error"));
    }
    invalidate();
    const response = await sendRequest(build, lockSignal);
    // 请求可能已改变 Cookie，表单卸载也须由常驻外壳对账。对账只读，不能重发原命令。
    if (mayHaveChangedCookie(response)) {
      if (method === "POST") {
        change.reason = "cookie_changed";
        markPublicSessionChange();
      } else {
        // 完整成功回执才确认退出，取消页面任务也不能撤销已确认的 Cookie 变化。
        const receipt =
          response.ok && responseOk(response.value) ? decodeReceipt(response.value) : null;
        change.reason = receipt?.ok === true ? "logged_out" : "logout_unknown";
        markPublicLogout(change.reason);
        markPublicSessionChange();
        if (receipt?.ok === true) {
          publishPublicSessionIdentity({ kind: "session", session: null }, { cookieWritten: true });
        }
        if (receipt !== null && !lockSignal.aborted) {
          return receipt;
        }
      }
    }
    if (lockSignal.aborted) {
      return err(mapError("network_error"));
    }
    return response.ok
      ? decodeReceipt(response.value)
      : err(mapError(transportErrorCode(response.error)));
  }, signal);
  if (change.reason !== null) {
    // 复核在写锁作用域结束后另取同一锁；读取缓存由各自的 owner 按作用域管理。
    requestPublicSessionCheck(change.reason);
  }
  return locked.ok ? locked.value : err(mapError(locked.error));
}

/** 使用登录 key 登录一次；完整回执才记录 Cookie 生命周期。 */
export async function loginPublic(
  key: string,
  signal: AbortSignal,
): Promise<Result<PublicSession, PublicSessionApiError>> {
  return sendCookieMutation(
    () =>
      ok({
        url: SESSION_PATH,
        init: {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ key }),
        },
      }),
    signal,
    "POST",
    publicSessionApiErrorLocal,
    (response) => {
      if (!responseOk(response)) {
        return err(parseApiError(response));
      }
      const decoded = decodeIdentitySuccess(
        response,
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
      markPublicLogin();
      publishPublicSessionIdentity({ kind: "session", session }, { cookieWritten: true });
      return ok(session);
    },
  );
}

/** 确认退出在锁内结束旧归属，锁释放后仍须读回当前 Cookie。 */
export async function logoutPublic(
  userId: string,
  signal: AbortSignal,
): Promise<Result<null, string>> {
  return sendCookieMutation(
    () =>
      ok({
        url: SESSION_PATH,
        init: { method: "DELETE", headers: { [PUBLIC_EXPECTED_USER_ID_HEADER]: userId } },
      }),
    signal,
    "DELETE",
    (code) => code,
    (response) => {
      if (!responseOk(response)) {
        const code = decodeErrorCode(response);
        observePublicSessionFailure(response.status, code, userId);
        return err(code);
      }
      const decoded = decodeIdentitySuccess(response, jsonValue, (value) => value === null);
      return decoded.ok ? ok(null) : err(decoded.error);
    },
  );
}
