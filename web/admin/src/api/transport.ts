// Admin 同源请求发送与写前 CSRF 复核。
// 传输边界复用 `@auction/web-shared` 的统一超时与错误分类；本层只把结构化错误折回稳定 code。
// `x-csrf-token` 与 Basic Auth header 由调用方在 `build` 闭包叠加（共享层不感知凭据）。

import {
  DEFAULT_REQUEST_TIMEOUT_MILLIS,
  sendGet as sharedSendGet,
  sendRequestWithTimeout as sharedSendRequestWithTimeout,
  type TransportError,
  type TransportRequest,
  type TransportResponse,
  transportErrorCode,
} from "@auction/web-shared/net/transport";
import { err, ok, type Result } from "@auction/web-shared/result";

import {
  type AdminApiFailure,
  adminApiFailureClient,
  adminApiFailureNotSentCode,
  invalidRequestError,
  responseFailure,
} from "./failure";
import {
  type AdminSession,
  type AdminSessionHost,
  adminSessionHostLive,
  adminSessionSameIdentity,
  adminSessionWritesAllowed,
  confirmedAdminSession,
  fetchAdminSession,
  requestAdminSessionCheck,
} from "./session";

/** 调用方构造的 Admin 请求；构建失败只暴露稳定错误码。取消信号由传输层统一提供。 */
export type AdminRequestBuilder = (signal: AbortSignal) => Result<TransportRequest, string>;

/** 发送不改写 URL 的 Admin GET 请求；宿主句柄已注销时不再发出。 */
export async function sendGet(
  host: AdminSessionHost,
  url: string,
  signal: AbortSignal,
): Promise<Result<TransportResponse, string>> {
  // 请求尚未交给共享传输：取消或宿主注销时不再发出。
  if (signal.aborted || !adminSessionHostLive(host)) {
    return err("network_error");
  }
  const result = await sharedSendGet(url, signal);
  // 等待期间的任务取消或宿主注销对应 Rust 被丢弃的 future；不能把已缓冲的成功响应当作本次结果。
  if (signal.aborted || !adminSessionHostLive(host)) {
    return err("network_error");
  }
  return result.ok ? ok(result.value) : err(adminTransportErrorCode(result.error));
}

/**
 * 在共享传输层的统一超时上限内发送一次 Admin 请求。
 * 保存构建阶段：只有成功构造请求、交给 fetch 后的故障可能对应已提交的业务写入。
 */
export function sendRequest(
  build: AdminRequestBuilder,
  signal: AbortSignal,
): Promise<Result<TransportResponse, string>> {
  return sendRequestWithTimeout(build, DEFAULT_REQUEST_TIMEOUT_MILLIS, signal);
}

// 只有 `prepareJsonCommand` 能构造已固定的命令；外部同形对象在运行期也不会被接受。
const preparedJsonCommandBrand: unique symbol = Symbol("admin.prepared-json-command");

/** 已固定的 JSON 命令正文；只持有不可变的文本，重试必须复用同一个值。 */
export interface PreparedJsonCommand {
  readonly [preparedJsonCommandBrand]: true;
  readonly body: string;
}

// 品牌属性会随对象展开被复制；正文只按工厂返回对象的身份从私有表取出，副本不能发送。
const preparedJsonCommandBodies = new WeakMap<PreparedJsonCommand, string>();

/** 在任何 await 之前序列化命令；同一 `command_id` 的重试只能发送这份文本。 */
export function prepareJsonCommand(command: unknown): Result<PreparedJsonCommand, AdminApiFailure> {
  try {
    const body = JSON.stringify(command);
    if (body === undefined) {
      return err(adminApiFailureClient(invalidRequestError(command)));
    }
    const prepared = Object.freeze({ [preparedJsonCommandBrand]: true as const, body });
    preparedJsonCommandBodies.set(prepared, body);
    return ok(prepared);
  } catch (error) {
    return err(adminApiFailureClient(invalidRequestError(error)));
  }
}

/**
 * 为正文里带 `command_id` 的命令固定正文：提交时冻结输入，执行体第一次运行时生成并缓存正文。
 * 同一个 ID 的后续重试返回同一份文本；换 ID 是编程错误，返回客户端失败。
 */
export function prepareJsonCommandOnce(
  build: (commandId: string) => unknown,
): (commandId: string) => Result<PreparedJsonCommand, AdminApiFailure> {
  let cachedId: string | null = null;
  let cached: Result<PreparedJsonCommand, AdminApiFailure> | null = null;
  return (commandId) => {
    if (cached !== null) {
      if (commandId !== cachedId) {
        return err(adminApiFailureClient(invalidRequestError(commandId)));
      }
      return cached;
    }
    let prepared: Result<PreparedJsonCommand, AdminApiFailure>;
    try {
      prepared = prepareJsonCommand(build(commandId));
    } catch (error) {
      prepared = err(adminApiFailureClient(invalidRequestError(error)));
    }
    cachedId = commandId;
    cached = prepared;
    return prepared;
  };
}

/**
 * 发送一次已固定的 JSON 写请求，保留错误回执以便调用方按端点分类。
 * 缺省预算与 `sendRequest` 相同；只有需要更长等待的端点才传入 `timeoutMillis`。
 */
export async function sendPreparedJsonWrite(
  host: AdminSessionHost,
  url: string,
  method: string,
  prepared: PreparedJsonCommand,
  signal: AbortSignal,
  timeoutMillis: number = DEFAULT_REQUEST_TIMEOUT_MILLIS,
): Promise<Result<TransportResponse, AdminApiFailure>> {
  // 只接受工厂返回的同一个对象；展开副本虽能复制品牌，却没有登记的正文。
  const body = preparedJsonCommandBodies.get(prepared);
  if (body === undefined) {
    return err(adminApiFailureClient(invalidRequestError(prepared)));
  }
  // 写请求尚未发出：已取消时连 CSRF GET 都不发起。
  if (signal.aborted) {
    return err(adminApiFailureClient(adminApiFailureNotSentCode("network_error")));
  }
  if (!adminSessionHostLive(host) || !adminSessionWritesAllowed(host)) {
    return err(adminApiFailureClient(adminApiFailureNotSentCode("unauthenticated")));
  }
  // 命令属于发起时的会话代际；CSRF 等待期间即使换成同账号的新 token，也不能借用。
  const expectedSession = confirmedAdminSession(host);
  if (expectedSession === null) {
    return err(adminApiFailureClient(adminApiFailureNotSentCode("unauthenticated")));
  }
  const csrfToken = await fetchCsrfToken(host, signal);
  // CSRF 复核成功兑现与调用方恢复之间仍可能被取消；此时业务写入尚未发出。
  if (signal.aborted || !adminSessionHostLive(host)) {
    return err(adminApiFailureClient(adminApiFailureNotSentCode("network_error")));
  }
  if (!csrfToken.ok) {
    return err(adminApiFailureClient(csrfToken.error));
  }
  const token = csrfToken.value;
  if (!writeSessionAuthorized(host, expectedSession, token)) {
    return err(adminApiFailureClient(adminApiFailureNotSentCode("unauthenticated")));
  }
  const response = await sendRequestWithTimeout(
    () => {
      // 所有 await 已经结束；交给 fetch 前同步复核原宿主、写许可、身份与本次 CSRF。
      // 会话复核可能在 CSRF 返回后撤回许可，不能把旧页面保存的命令继续发出。
      if (signal.aborted) {
        return err("network_error");
      }
      if (!writeSessionAuthorized(host, expectedSession, token)) {
        return err("unauthenticated");
      }
      return ok({
        url,
        init: {
          method,
          headers: { "content-type": "application/json", "x-csrf-token": token },
          body,
        },
      });
    },
    timeoutMillis,
    signal,
  );
  if (!response.ok) {
    return err(adminApiFailureClient(response.error));
  }
  const received = response.value;
  if (signal.aborted || !adminSessionHostLive(host)) {
    // 取消或宿主注销后 Rust 的 future 已被丢弃；不再读取回执或触发 401 会话复核。
    return err(adminApiFailureClient("network_error"));
  }
  return responseOk(received) ? ok(received) : err(await responseFailure(received, signal, host));
}

/** 发送一次带 CSRF 复核的 JSON 写请求；先固定正文再交给每次尝试复用。 */
export async function sendJsonWrite(
  host: AdminSessionHost,
  url: string,
  method: string,
  command: unknown,
  signal: AbortSignal,
): Promise<Result<TransportResponse, AdminApiFailure>> {
  // 已取消时连序列化与 CSRF GET 都不做。
  if (signal.aborted) {
    return err(adminApiFailureClient(adminApiFailureNotSentCode("network_error")));
  }
  const prepared = prepareJsonCommand(command);
  if (!prepared.ok) {
    return err(prepared.error);
  }
  return sendPreparedJsonWrite(host, url, method, prepared.value, signal);
}

/** 为单次 Admin 请求指定等待预算；仍复用共享 fetch，不改变其他请求预算或添加重试。 */
export async function sendRequestWithTimeout(
  build: AdminRequestBuilder,
  timeoutMillis: number,
  signal: AbortSignal,
): Promise<Result<TransportResponse, string>> {
  // 请求尚未交给共享传输；取消只能裁定本次尝试，不能占住未决命令。
  if (signal.aborted) {
    return err(adminApiFailureNotSentCode("network_error"));
  }
  let buildFailure: string | null = null;
  let built = false;
  const result = await sharedSendRequestWithTimeout(
    (abortSignal) => {
      if (abortSignal.aborted) {
        buildFailure = "network_error";
        return err({ kind: "network", status: null });
      }
      const builtRequest = build(abortSignal);
      if (builtRequest.ok) {
        try {
          const url = builtRequest.value.url;
          const init: Omit<RequestInit, "signal"> =
            builtRequest.value.init === undefined ? {} : { ...builtRequest.value.init };
          if (init.headers !== undefined) {
            // 只读取调用方的 header 输入一次；共享层随后拿到的是普通 Headers。
            init.headers = new Headers(init.headers);
          }
          // 浏览器在这里才真正构造 Request；例如带正文的 GET 会在 fetch 前失败。
          // 构造成功后返回已经读取过的普通对象，避免 fetch 再次触发调用方 getter。
          new Request(url, {
            ...init,
            signal: abortSignal,
          });
          built = true;
          return ok({ url, init });
        } catch (error) {
          buildFailure = invalidRequestError(error);
          return err({ kind: "network", status: null });
        }
      }
      buildFailure = builtRequest.error;
      return err({ kind: "network", status: null });
    },
    timeoutMillis,
    signal,
  );
  if (result.ok) {
    // 正文已完整缓冲，但调用方在返回前取消：请求已交给共享传输，不能回报为未发送的成功。
    if (signal.aborted) {
      return err("network_error");
    }
    return ok(result.value);
  }
  if (built) {
    return err(adminTransportErrorCode(result.error));
  }
  return err(adminApiFailureNotSentCode(buildFailure ?? adminTransportErrorCode(result.error)));
}

/** 传输层的结构化错误折回稳定 code。 */
function adminTransportErrorCode(error: TransportError): string {
  return transportErrorCode(error);
}

/** 写请求的最终门禁：原宿主仍存活、同一会话代际、本次 CSRF 与当前写许可都成立。 */
function writeSessionAuthorized(
  host: AdminSessionHost,
  expected: AdminSession | null,
  token: string,
): boolean {
  if (!adminSessionHostLive(host) || !adminSessionWritesAllowed(host)) {
    return false;
  }
  const confirmed = confirmedAdminSession(host);
  return (
    expected !== null &&
    confirmed !== null &&
    adminSessionSameIdentity(expected, confirmed) &&
    confirmed.csrfToken === token
  );
}

/** 写请求的 CSRF 门禁：取得 token 前后都重读写许可，并核对会话身份未被其他标签页替换。 */
export async function fetchCsrfToken(
  host: AdminSessionHost,
  signal: AbortSignal,
): Promise<Result<string, string>> {
  // 已取消或宿主已注销时连 CSRF GET 都不发起。
  if (signal.aborted) {
    return err(adminApiFailureNotSentCode("network_error"));
  }
  // 认证内状态不等于继续授权；会话复核未确认时不发起新的管理写入。
  if (!adminSessionHostLive(host) || !adminSessionWritesAllowed(host)) {
    return err("unauthenticated");
  }
  const current = await fetchAdminSession(host, signal);
  // 调用方取消或宿主注销后不再发出会话重查；Rust 里被丢弃的 future 同样不会继续执行。
  if (signal.aborted || !adminSessionHostLive(host)) {
    return err(adminApiFailureNotSentCode("network_error"));
  }
  if (!current.ok || current.value === null) {
    // CSRF 复核失败也撤回写许可；旧宿主与未决命令交给既有会话重查流程保留。
    requestAdminSessionCheck(host);
    const code = current.ok ? "unauthenticated" : current.error;
    // 此处只有会话 GET 被发送；写入未开始，不能把 GET 的超时或坏响应当作写入结果未知。
    return err(adminApiFailureNotSentCode(code));
  }
  // 获取 CSRF 的等待期间也可能进入暂时未确认状态，不能带着旧页面授权继续提交。
  if (!adminSessionWritesAllowed(host)) {
    return err("unauthenticated");
  }
  // 其他标签页可能已重新登录；新会话的 token 不能授权旧宿主保存的命令或草稿。
  const confirmed = confirmedAdminSession(host);
  const sameIdentity =
    confirmed !== null &&
    confirmed.actor === current.value.actor &&
    confirmed.csrfToken === current.value.csrf_token;
  if (!sameIdentity) {
    requestAdminSessionCheck(host);
    return err("unauthenticated");
  }
  return ok(current.value.csrf_token);
}

/** gloo 的 `Response::ok`：状态码 200 到 299。 */
function responseOk(response: TransportResponse): boolean {
  return response.status >= 200 && response.status <= 299;
}
