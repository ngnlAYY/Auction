// Admin 会话请求、身份确认与本地会话登记。
// 会话读取、登录、续期与注销都只消费稳定 code；CSRF、Basic Auth 秘密不进入日志或持久状态。
// 写许可与重查回调只属于已挂载的认证外壳；每一层请求都必须携带登记时发给它的宿主句柄。

import { AdminSessionResponseSchema } from "@auction/contracts/dto/admin/session";
import { apiResponseSchema } from "@auction/contracts/dto/response";
import { decode, jsonValue } from "@auction/contracts/wire";
import type { TransportResponse } from "@auction/web-shared/net/transport";
import { parseJson } from "@auction/web-shared/net/wire";
import { err, ok, type Result } from "@auction/web-shared/result";

import {
  type AdminApiFailure,
  adminApiFailureClient,
  invalidResponseError,
  readResponseFailure,
  responseFailure,
} from "./failure";
import { parseStrictRfc3339, retryAfterMillis } from "./session-time";
import { sendGet, sendRequest } from "./transport";

const SESSION_WITH_SERVER_TIME = "/api/v1/admin/session?include=server_time";
const SESSION_ACTIVITY = "/api/v1/admin/session/activity";

/** 外壳提供的会话重读入口；跨标签页结束会话、重新聚焦和页面可见时使用。 */
export type AdminSessionCheckTrigger = () => void;

/** 认证内状态不等于继续授权；连接丢失或复核未完成时写门禁必须失败关闭。 */
export type AdminSessionWritePermission = () => boolean;

/** 请求发出时记录代际，异代正文与可信 401 都交回原宿主。 */
export interface AdminSessionWriteReview {
  readonly isCurrent: () => boolean;
  readonly observe: (session: AdminSession) => void;
  readonly cookieGeneration: () => string | null;
  readonly invalidate: (session: AdminSession) => void;
}

/**
 * Admin 会话的进程内快照；服务端时刻保留完整纳秒，接收时间用单调时钟。
 * 内部字段用小驼峰；`csrf_token` 是照搬回执字段名的兼容别名，两处取值相同。
 */
export interface AdminSession {
  readonly actor: string;
  readonly csrfToken: string;
  /** @deprecated 兼容按回执字段名书写的调用方；新代码使用 `csrfToken`。 */
  readonly csrf_token: string;
  readonly expiresAt: bigint;
  readonly absoluteExpiresAt: bigint;
  readonly serverTime: bigint;
  readonly receivedAtMillis: number;
}

/** 续期被拒绝时同时保留稳定代码与服务端允许的最早重试时间。 */
export interface AdminSessionRenewalFailure {
  readonly failure: AdminApiFailure;
  readonly retryAfterMillis: number | null;
}

/** 认证外壳在挂载时登记的能力；句柄由登记函数生成，调用方不能自行构造。 */
export interface AdminSessionHostCapabilities {
  readonly check: AdminSessionCheckTrigger;
  readonly writesAllowed: AdminSessionWritePermission;
  readonly confirmed: () => AdminSession | null;
  readonly cookieGeneration?: () => string | null;
  readonly writeReview?: AdminSessionWriteReview;
}

// 句柄品牌只在模块内可见；外部无法构造出满足 `AdminSessionHost` 的对象。
const adminSessionHostBrand: unique symbol = Symbol("admin.session-host");

/**
 * 认证外壳的宿主句柄。只有 `registerAdminSessionHost` 在登记时发出的对象有效；
 * 句柄寿命等于一次登记到对应的注销，注销后的旧引用不会因重新登记而恢复。
 */
export interface AdminSessionHost {
  readonly [adminSessionHostBrand]: true;
}

/** 登记结果：`host` 交给本次挂载的所有请求，`release` 挂到外壳卸载。 */
export interface AdminSessionHostRegistration {
  readonly host: AdminSessionHost;
  readonly release: () => void;
}

interface RegisteredSessionHost {
  readonly capabilities: AdminSessionHostCapabilities;
  live: boolean;
}

// 句柄本身决定请求属于哪一次挂载；模块不保存“当前宿主”，由认证外壳经 Solid Context 下发。
const sessionHostEntries = new WeakMap<AdminSessionHost, RegisteredSessionHost>();

/** 登记认证外壳；每次登记生成新句柄，卸载时调用返回的 `release`。 */
export function registerAdminSessionHost(
  capabilities: AdminSessionHostCapabilities,
): AdminSessionHostRegistration {
  const host: AdminSessionHost = Object.freeze({ [adminSessionHostBrand]: true as const });
  const entry: RegisteredSessionHost = { capabilities, live: true };
  sessionHostEntries.set(host, entry);
  return {
    host,
    release() {
      if (!entry.live) {
        return;
      }
      entry.live = false;
    },
  };
}

/** 句柄对应的登记；注销后与从未登记过的同形对象都返回 `null`。 */
function registeredSessionHost(host: AdminSessionHost): RegisteredSessionHost | null {
  const entry = sessionHostEntries.get(host);
  return entry?.live === true ? entry : null;
}

/** 句柄是否仍对应一次未注销的登记。 */
export function adminSessionHostLive(host: AdminSessionHost): boolean {
  return registeredSessionHost(host) !== null;
}

/** 会话重读必须由外壳触发，不能根据单次网络错误推断登出；只重查这个句柄对应的外壳。 */
export function requestAdminSessionCheck(host: AdminSessionHost): void {
  try {
    registeredSessionHost(host)?.capabilities.check();
  } catch {
    // A check callback failure cannot authorize or alter the request that triggered it.
  }
}

/** 未确认或外壳已卸载时写门禁关闭。 */
export function adminSessionWritesAllowed(host: AdminSessionHost): boolean {
  try {
    return registeredSessionHost(host)?.capabilities.writesAllowed() ?? false;
  } catch {
    return false;
  }
}

/** 最近一次已认证会话；只用于同代际核对，不是授权来源。 */
export function confirmedAdminSession(host: AdminSessionHost): AdminSession | null {
  try {
    const entry = registeredSessionHost(host);
    return entry === null ? null : entry.capabilities.confirmed();
  } catch {
    return null;
  }
}

/** 读取此宿主登记的身份观测与请求失效回调，旧句柄不能接管新宿主。 */
export function adminSessionWriteReview(host: AdminSessionHost): AdminSessionWriteReview | null {
  return registeredSessionHost(host)?.capabilities.writeReview ?? null;
}
/** 读取发送边界上的 Cookie 代际；旧能力对象只提供 review 时沿用其读取器。 */
export function adminSessionCookieGeneration(host: AdminSessionHost): string | null {
  const capabilities = registeredSessionHost(host)?.capabilities;
  try {
    return (
      capabilities?.cookieGeneration?.() ?? capabilities?.writeReview?.cookieGeneration() ?? null
    );
  } catch {
    return null;
  }
}

/** 两个会话属于同一身份与同一 CSRF 代际；宿主归属由调用方持有的句柄保证。 */
export function adminSessionSameIdentity(left: AdminSession, right: AdminSession): boolean {
  return left.actor === right.actor && left.csrfToken === right.csrfToken;
}

// 只有实际发出时可核对的代际才能报告失效。回执到达时不重新认领宿主或代际。
const responseInvalidations = new WeakMap<TransportResponse, () => void>();

/** 在发出请求前固定归属。回执中的代际不能从收包时的宿主反推。 */
export function captureAdminSessionInvalidation(host: AdminSessionHost): (() => void) | null {
  const review = adminSessionWriteReview(host);
  if (review === null) {
    return null;
  }
  // 宿主能力回调可能抛错；任一步失败都只能拒绝登记，不能把异常带出发送路径。
  let current: boolean;
  try {
    current = review.isCurrent();
  } catch {
    return null;
  }
  if (!current) {
    return null;
  }
  const session = confirmedAdminSession(host);
  if (session === null) {
    return null;
  }
  let generation: string | null;
  try {
    generation = review.cookieGeneration();
  } catch {
    return null;
  }
  if (generation !== session.csrfToken) {
    return null;
  }
  return () => {
    if (adminSessionHostLive(host)) {
      try {
        review.invalidate(session);
      } catch {
        // 失效回调失败同样不能改变已返回给调用方的回执。
      }
    }
  };
}

/** 将请求发出时的失效归属附到本次 HTTP 401 回执。 */
export function bindAdminSessionInvalidation(
  response: TransportResponse,
  invalidate: (() => void) | null,
): void {
  if (response.status === 401 && invalidate !== null) {
    responseInvalidations.set(response, invalidate);
  }
}

/** 完整解码可信的 401 后向原宿主交付一次失效证据。 */
export function reportAdminSessionInvalidation(response: TransportResponse): void {
  const invalidate = responseInvalidations.get(response);
  responseInvalidations.delete(response);
  invalidate?.();
}

/** 请求返回后仍须由发起时的宿主收尾；宿主注销或任务取消后不能再报告结果。 */
function requestStillLive(host: AdminSessionHost, signal: AbortSignal): boolean {
  return !signal.aborted && adminSessionHostLive(host);
}

/** 读取当前 Admin 会话；401 且稳定 code 为 `unauthenticated` 时是正常登出。 */
export async function fetchAdminSession(
  host: AdminSessionHost,
  signal: AbortSignal,
): Promise<Result<AdminSession | null, string>> {
  // 宿主已注销或任务已取消时不再发出会话 GET；等待期间挂载的新外壳不能接管这次读取。
  if (!requestStillLive(host, signal)) {
    return err("network_error");
  }
  const response = await sendGet(host, SESSION_WITH_SERVER_TIME, signal).catch(() =>
    err("network_error"),
  );
  if (!requestStillLive(host, signal)) {
    return err("network_error");
  }
  if (!response.ok) {
    return err(response.error);
  }
  const received = response.value;
  if (!responseOk(received)) {
    // 外壳自己的读票没有业务宿主归属。写前 GET 的可信 401 则携带实发归属。
    const failure = await responseFailure(received, signal, host);
    if (failure.status === 401 && failure.code === "unauthenticated") {
      return ok(null);
    }
    return err(failure.code);
  }
  const session = decodeAdminSessionResponse(received.text());
  if (!session.ok) {
    return err(session.error);
  }
  return ok(session.value);
}

/** Basic Auth 登录；秘密只用于本次请求头，不写入任何持久状态。 */
export async function loginAdmin(
  host: AdminSessionHost,
  actor: string,
  secret: string,
  signal: AbortSignal,
): Promise<Result<AdminSession, string>> {
  if (!requestStillLive(host, signal)) {
    return err("network_error");
  }
  if (actor.includes(":")) {
    return err("invalid_request");
  }
  const authorization = `Basic ${base64EncodeUtf8(`${actor}:${secret}`)}`;
  const response = await sendRequest(
    () =>
      ok({
        url: SESSION_WITH_SERVER_TIME,
        init: {
          method: "POST",
          headers: { authorization },
        },
      }),
    signal,
  );
  if (!requestStillLive(host, signal)) {
    return err("network_error");
  }
  if (!response.ok) {
    return err(response.error);
  }
  const received = response.value;
  if (!responseOk(received)) {
    // Basic Auth 的拒绝随登录回执交付，登录专属 GET 决定当前 Cookie 的状态。
    const failure = await responseFailure(received, signal, host);
    return err(failure.code);
  }
  return decodeAdminSessionResponse(received.text());
}

/** 注销当前会话；成功回执必须是 `code=ok` 且 `data=null`。 */
export async function logoutAdmin(
  host: AdminSessionHost,
  signal: AbortSignal,
): Promise<Result<void, AdminApiFailure>> {
  if (!requestStillLive(host, signal)) {
    return err(adminApiFailureClient("network_error"));
  }
  // 注销只使用该句柄自己确认的会话；调用方不能把另一个外壳的 token 带进来。
  const session = confirmedAdminSession(host);
  if (session === null) {
    return err(adminApiFailureClient("unauthenticated"));
  }
  const response = await sendRequest(() => {
    // 注销状态会推进复核 epoch；只按发出 DELETE 时的 Cookie 与冻结会话代际核对。
    if (adminSessionCookieGeneration(host) !== session.csrfToken) {
      return err("unauthenticated");
    }
    return ok({
      url: "/api/v1/admin/session",
      init: {
        method: "DELETE",
        headers: { "x-csrf-token": session.csrfToken },
      },
    });
  }, signal);
  if (!requestStillLive(host, signal)) {
    return err(adminApiFailureClient("network_error"));
  }
  if (!response.ok) {
    return err(adminApiFailureClient(response.error));
  }
  const received = response.value;
  if (!responseOk(received)) {
    return err(await sessionFailure(received, signal, host));
  }
  // 决定登出的回执必须拒绝任意层的重复键与非整数词法。
  const parsed = parseJson(received.text(), { uniqueKeys: "all", numbers: "integers" });
  if (!parsed.ok) {
    return err(adminApiFailureClient(invalidResponseError(parsed.error)));
  }
  const payload = decode(apiResponseSchema(jsonValue), parsed.value);
  if (!payload.ok) {
    return err(adminApiFailureClient(invalidResponseError(payload)));
  }
  if (payload.value.code !== "ok" || payload.value.data !== null) {
    // 语义不符合注销回执时不重复上报 DTO 解码错误，只返回稳定失败码。
    return err(adminApiFailureClient("invalid_response"));
  }
  return ok(undefined);
}

/** 活动续期；失败时保留服务端给出的最早重试时间。 */
export async function renewAdminSession(
  host: AdminSessionHost,
  signal: AbortSignal,
): Promise<Result<AdminSession, AdminSessionRenewalFailure>> {
  if (!requestStillLive(host, signal)) {
    return err({
      failure: adminApiFailureClient("network_error"),
      retryAfterMillis: null,
    });
  }
  // 续期只使用该句柄自己确认的会话；快照缺失时拒绝发送，不能借用别的宿主。
  const session = confirmedAdminSession(host);
  if (session === null) {
    return err({
      failure: adminApiFailureClient("unauthenticated"),
      retryAfterMillis: null,
    });
  }
  const response = await sendRequest(() => {
    // 等待前的快照不能代替发出时的门禁；代际或写许可已变时不能借用旧 CSRF 续期。
    if (
      !adminSessionWritesAllowed(host) ||
      adminSessionCookieGeneration(host) !== session.csrfToken
    ) {
      return err("unauthenticated");
    }
    return ok({
      url: SESSION_ACTIVITY,
      init: {
        method: "POST",
        headers: { "x-csrf-token": session.csrfToken },
      },
    });
  }, signal);
  if (!requestStillLive(host, signal)) {
    return err({
      failure: adminApiFailureClient("network_error"),
      retryAfterMillis: null,
    });
  }
  if (!response.ok) {
    return err({
      failure: adminApiFailureClient(response.error),
      retryAfterMillis: null,
    });
  }
  const received = response.value;
  if (!responseOk(received)) {
    const retryAfter = retryAfterMillis(
      received.headers.get("retry-after"),
      received.headers.get("date"),
    );
    return err({
      failure: await sessionFailure(received, signal, host),
      retryAfterMillis: retryAfter,
    });
  }
  const renewed = decodeAdminSessionResponse(received.text());
  if (!renewed.ok) {
    return err({
      failure: adminApiFailureClient(renewed.error),
      retryAfterMillis: null,
    });
  }
  return ok(renewed.value);
}

/** 401 错误回执按稳定码分类；不调用会再次触发会话读取的通用响应失败。 */
async function sessionFailure(
  response: TransportResponse,
  signal: AbortSignal,
  host: AdminSessionHost,
): Promise<AdminApiFailure> {
  if (response.status !== 401) {
    return responseFailure(response, signal, host);
  }
  // 决定是否登出的 401 回执必须按原始字节严格解码；坏字节不能确认会话结束。
  return readResponseFailure(response);
}

/** 解码成功会话；身份、CSRF 与三个时间都必须是可用值。 */
function decodeAdminSessionResponse(body: string): Result<AdminSession, string> {
  // 决定会话授权的投影必须拒绝任意层的重复键与非整数词法；策略同时执行 serde 的容器深度预算。
  const parsed = parseJson(body, { uniqueKeys: "all", numbers: "integers" });
  if (!parsed.ok) {
    return err(invalidResponseError(parsed.error));
  }
  const payload = decode(apiResponseSchema(AdminSessionResponseSchema), parsed.value);
  if (!payload.ok) {
    return err(invalidResponseError(payload));
  }
  const data = payload.value.data;
  if (
    payload.value.code !== "ok" ||
    !data.authenticated ||
    data.actor === "" ||
    data.csrf_token === ""
  ) {
    // 会话语义不符合成功投影时源码直接返回稳定码，不追加 DTO 解码信标。
    return err("invalid_response");
  }
  if (data.server_time === null) {
    return err("invalid_response");
  }
  const expiresAt = parseStrictRfc3339(data.expires_at);
  const absoluteExpiresAt = parseStrictRfc3339(data.absolute_expires_at);
  const serverTime = parseStrictRfc3339(data.server_time);
  if (
    expiresAt === null ||
    absoluteExpiresAt === null ||
    serverTime === null ||
    expiresAt <= serverTime ||
    absoluteExpiresAt < expiresAt
  ) {
    return err(invalidResponseError(null));
  }
  return ok({
    actor: data.actor,
    csrfToken: data.csrf_token,
    csrf_token: data.csrf_token,
    expiresAt,
    absoluteExpiresAt,
    serverTime,
    receivedAtMillis: performance.now(),
  });
}

/** gloo 的 `Response::ok`：状态码 200 到 299。 */
function responseOk(response: { status: number }): boolean {
  return response.status >= 200 && response.status <= 299;
}

/** UTF-8 字节的 Base64；Basic Auth 不能按 UCS-2 直接喂给 `btoa`。 */
function base64EncodeUtf8(text: string): string {
  const bytes = new TextEncoder().encode(text);
  let binary = "";
  const chunkSize = 0x8000;
  for (let offset = 0; offset < bytes.length; offset += chunkSize) {
    const chunk = bytes.subarray(offset, offset + chunkSize);
    let part = "";
    for (const byte of chunk) {
      part += String.fromCharCode(byte);
    }
    binary += part;
  }
  return btoa(binary);
}
