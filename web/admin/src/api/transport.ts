// Admin 同源请求发送与写前 CSRF 复核。
// 复用共享传输的超时与错误分类，在实际业务写响应处保留裁定所需的证据。
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
  type AdminWriteEndpoint,
  type AdminWriteFailure,
  type AdminWriteNotSent,
  type AdminWriteOutcomeUnknown,
  adminApiFailureClient,
  adminApiFailureNotSentCode,
  decodeResponseFailure,
  invalidRequestError,
  responseFailure,
  validHttpProtocolCode,
  writeNotSent,
  writeOutcomeUnknown,
} from "./failure";
import {
  type AdminSession,
  type AdminSessionHost,
  adminSessionHostLive,
  adminSessionSameIdentity,
  adminSessionWriteReview,
  adminSessionWritesAllowed,
  bindAdminSessionInvalidation,
  captureAdminSessionInvalidation,
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
  const invalidate = captureAdminSessionInvalidation(host);
  const result = await sharedSendGet(url, signal);
  // 等待期间的任务取消或宿主注销对应 Rust 被丢弃的 future；不能把已缓冲的成功响应当作本次结果。
  if (signal.aborted || !adminSessionHostLive(host)) {
    return err("network_error");
  }
  if (!result.ok) {
    return err(adminTransportErrorCode(result.error));
  }
  bindAdminSessionInvalidation(result.value, invalidate);
  return ok(result.value);
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

/** 一次业务写操作的请求描述，端点类别与方法、URL 一同交给发送器。 */
export interface AdminWriteOperation {
  readonly url: string;
  readonly method: "POST" | "PUT" | "PATCH" | "DELETE";
  readonly endpoint: AdminWriteEndpoint;
}

const VERIFIED_WRITE_RECEIPT_BRAND: unique symbol = Symbol("admin.verified-write-receipt");

/** 已验证写回执的句柄，只有发送器登记的原实例才能用于裁定命令。 */
export interface VerifiedWriteReceipt {
  readonly [VERIFIED_WRITE_RECEIPT_BRAND]: true;
}

interface VerifiedWriteEvidence {
  readonly status: number;
  readonly endpoint: AdminWriteEndpoint;
  readonly protocolCode: string;
}

// 只有登记的原始回执才有资格了结未决命令；复制品牌不能复制证据。
const VERIFIED_WRITE_RECEIPTS = new WeakMap<VerifiedWriteReceipt, VerifiedWriteEvidence>();

/** 从发送器的登记表读取不可变证据；副本和未登记对象返回 `null`。 */
export function readVerifiedWriteReceipt(
  receipt: VerifiedWriteReceipt,
): VerifiedWriteEvidence | null {
  return VERIFIED_WRITE_RECEIPTS.get(receipt) ?? null;
}

type AdminSendFailure = AdminWriteNotSent | AdminWriteOutcomeUnknown;

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
export function prepareJsonCommand(
  command: unknown,
): Result<PreparedJsonCommand, AdminWriteNotSent> {
  try {
    const body = JSON.stringify(command);
    if (body === undefined) {
      return err(writeNotSent(invalidRequestError(command)));
    }
    const prepared = Object.freeze({ [preparedJsonCommandBrand]: true as const, body });
    preparedJsonCommandBodies.set(prepared, body);
    return ok(prepared);
  } catch (error) {
    return err(writeNotSent(invalidRequestError(error)));
  }
}

/**
 * 为正文里带 `command_id` 的命令固定正文：提交时冻结输入，执行体第一次运行时生成并缓存正文。
 * 同一个 ID 的后续重试返回同一份文本；换 ID 是编程错误，返回客户端失败。
 */
export function prepareJsonCommandOnce(
  build: (commandId: string) => unknown,
): (commandId: string) => Result<PreparedJsonCommand, AdminWriteNotSent> {
  let cachedId: string | null = null;
  let cached: Result<PreparedJsonCommand, AdminWriteNotSent> | null = null;
  return (commandId) => {
    if (cached !== null) {
      if (commandId !== cachedId) {
        return err(writeNotSent(invalidRequestError(commandId)));
      }
      return cached;
    }
    let prepared: Result<PreparedJsonCommand, AdminWriteNotSent>;
    try {
      prepared = prepareJsonCommand(build(commandId));
    } catch (error) {
      prepared = err(writeNotSent(invalidRequestError(error)));
    }
    cachedId = commandId;
    cached = prepared;
    return prepared;
  };
}

/** 发送已固定的业务命令；只有实际写请求的有效错误信封可以成为裁定证据。 */
export function sendPreparedJsonWrite(
  host: AdminSessionHost,
  operation: AdminWriteOperation,
  prepared: PreparedJsonCommand,
  signal: AbortSignal,
  timeoutMillis: number = DEFAULT_REQUEST_TIMEOUT_MILLIS,
): Promise<Result<TransportResponse, AdminWriteFailure>> {
  const { url, method, endpoint } = operation;
  return sendPreparedJsonRequest<AdminWriteFailure>(
    host,
    url,
    method,
    prepared,
    signal,
    timeoutMillis,
    (failure) => failure,
    (response) => {
      // 寄售错误契约要求 data 为 null，其他形态不能解除未决命令（不变量 1、6）。
      const decoded = decodeResponseFailure(
        response,
        signal,
        host,
        endpoint === "consignment-delist",
      );
      if (decoded === null) {
        return writeOutcomeUnknown("network_error");
      }
      if (decoded.kind === "invalid-envelope" || !validHttpProtocolCode(decoded.protocolCode)) {
        return writeOutcomeUnknown(`http_${decoded.status}`, decoded.status);
      }
      const receipt = Object.freeze<VerifiedWriteReceipt>({ [VERIFIED_WRITE_RECEIPT_BRAND]: true });
      VERIFIED_WRITE_RECEIPTS.set(
        receipt,
        Object.freeze({ status: decoded.status, endpoint, protocolCode: decoded.protocolCode }),
      );
      return { kind: "verified-http", receipt };
    },
  );
}

/** 验证和预览的 POST 沿用原错误投影，不参与业务命令的裁定。 */
export function sendPreparedJsonPreview(
  host: AdminSessionHost,
  url: string,
  prepared: PreparedJsonCommand,
  signal: AbortSignal,
  timeoutMillis: number = DEFAULT_REQUEST_TIMEOUT_MILLIS,
): Promise<Result<TransportResponse, AdminApiFailure>> {
  return sendPreparedJsonRequest(
    host,
    url,
    "POST",
    prepared,
    signal,
    timeoutMillis,
    (failure) => adminApiFailureClient(sendFailureCode(failure)),
    (response) => responseFailure(response, signal, host),
  );
}

async function sendPreparedJsonRequest<E>(
  host: AdminSessionHost,
  url: string,
  method: string,
  prepared: PreparedJsonCommand,
  signal: AbortSignal,
  timeoutMillis: number,
  sendFailure: (failure: AdminSendFailure) => E,
  httpFailure: (response: TransportResponse) => E | Promise<E>,
): Promise<Result<TransportResponse, E>> {
  // 只接受工厂返回的同一个对象；展开副本虽能复制品牌，却没有登记的正文。
  const body = preparedJsonCommandBodies.get(prepared);
  if (body === undefined) {
    return err(sendFailure(writeNotSent(invalidRequestError(prepared))));
  }
  // 写请求尚未发出：已取消时连 CSRF GET 都不发起。
  if (signal.aborted) {
    return err(sendFailure(writeNotSent("network_error")));
  }
  if (!adminSessionHostLive(host) || !adminSessionWritesAllowed(host)) {
    return err(sendFailure(writeNotSent("unauthenticated")));
  }
  // 命令属于发起时的会话代际；CSRF 等待期间即使换成同账号的新 token，也不能借用。
  const expectedSession = confirmedAdminSession(host);
  if (expectedSession === null) {
    return err(sendFailure(writeNotSent("unauthenticated")));
  }
  const csrfToken = await fetchCsrfToken(host, signal);
  // CSRF 复核成功兑现与调用方恢复之间仍可能被取消；此时业务写入尚未发出。
  if (signal.aborted || !adminSessionHostLive(host)) {
    return err(sendFailure(writeNotSent("network_error")));
  }
  if (!csrfToken.ok) {
    return err(sendFailure(writeNotSent(csrfToken.error)));
  }
  const token = csrfToken.value;
  if (!writeSessionAuthorized(host, expectedSession, token)) {
    return err(sendFailure(writeNotSent("unauthenticated")));
  }
  let invalidate: (() => void) | null = null;
  const response = await sendWithFailurePhase(
    () => {
      // 所有 await 已经结束；交给 fetch 前同步复核原宿主、写许可、身份与本次 CSRF。
      // 会话复核可能在 CSRF 返回后撤回许可，不能把旧页面保存的命令继续发出。
      if (signal.aborted) {
        return err("network_error");
      }
      if (!writeSessionAuthorized(host, expectedSession, token)) {
        return err("unauthenticated");
      }
      invalidate = captureAdminSessionInvalidation(host);
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
    sendFailure,
  );
  if (!response.ok) {
    return response;
  }
  const received = response.value;
  if (signal.aborted || !adminSessionHostLive(host)) {
    // 取消或宿主注销后 Rust 的 future 已被丢弃；不再读取回执或交付失效证据。
    return err(sendFailure(writeOutcomeUnknown("network_error")));
  }
  bindAdminSessionInvalidation(received, invalidate);
  return responseOk(received) ? ok(received) : err(await httpFailure(received));
}

/** 发送一次带 CSRF 复核的 JSON 验证或预览请求。 */
export async function sendJsonPreview(
  host: AdminSessionHost,
  url: string,
  command: unknown,
  signal: AbortSignal,
): Promise<Result<TransportResponse, AdminApiFailure>> {
  // 已取消时连序列化与 CSRF GET 都不做。
  if (signal.aborted) {
    return err(adminApiFailureClient(adminApiFailureNotSentCode("network_error")));
  }
  const prepared = prepareJsonCommand(command);
  if (!prepared.ok) {
    return err(adminApiFailureClient(sendFailureCode(prepared.error)));
  }
  return sendPreparedJsonPreview(host, url, prepared.value, signal);
}

/** 为单次非命令请求指定等待预算，保留原有失败投影。 */
export function sendRequestWithTimeout(
  build: AdminRequestBuilder,
  timeoutMillis: number,
  signal: AbortSignal,
): Promise<Result<TransportResponse, string>> {
  return sendWithFailurePhase(build, timeoutMillis, signal, sendFailureCode);
}

function sendFailureCode(failure: AdminSendFailure): string {
  return failure.kind === "not-sent" ? adminApiFailureNotSentCode(failure.code) : failure.code;
}

async function sendWithFailurePhase<E>(
  build: AdminRequestBuilder,
  timeoutMillis: number,
  signal: AbortSignal,
  projectFailure: (failure: AdminSendFailure) => E,
): Promise<Result<TransportResponse, E>> {
  // 请求尚未交给共享传输；取消只能裁定本次尝试，不能占住未决命令。
  if (signal.aborted) {
    return err(projectFailure(writeNotSent("network_error")));
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
      return err(projectFailure(writeOutcomeUnknown("network_error")));
    }
    return ok(result.value);
  }
  if (built) {
    return err(projectFailure(writeOutcomeUnknown(adminTransportErrorCode(result.error))));
  }
  return err(projectFailure(writeNotSent(buildFailure ?? adminTransportErrorCode(result.error))));
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
  if (
    !adminSessionHostLive(host) ||
    !adminSessionWritesAllowed(host) ||
    adminSessionWriteReview(host)?.isCurrent() === false
  ) {
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
async function fetchCsrfToken(
  host: AdminSessionHost,
  signal: AbortSignal,
): Promise<Result<string, string>> {
  // 已取消或宿主已注销时连 CSRF GET 都不发起。
  if (signal.aborted) {
    return err("network_error");
  }
  // 认证内状态不等于继续授权；会话复核未确认时不发起新的管理写入。
  if (!adminSessionHostLive(host) || !adminSessionWritesAllowed(host)) {
    return err("unauthenticated");
  }
  const expected = confirmedAdminSession(host);
  const review = adminSessionWriteReview(host);
  if (expected === null || review?.isCurrent() === false) {
    return err("unauthenticated");
  }
  const current = await fetchAdminSession(host, signal);
  // 调用方取消或宿主注销后不再发出会话重查；Rust 里被丢弃的 future 同样不会继续执行。
  if (signal.aborted || !adminSessionHostLive(host)) {
    return err("network_error");
  }
  if (review?.isCurrent() === false) {
    return err("unauthenticated");
  }
  if (!current.ok || current.value === null) {
    if (!current.ok) {
      // 无可信失效证据的复核故障沿用普通重查；可信 401 已随响应归属交给外壳。
      requestAdminSessionCheck(host);
    }
    const code = current.ok ? "unauthenticated" : current.error;
    // 此处只有会话 GET 被发送；写入未开始，不能把 GET 的超时或坏响应当作写入结果未知。
    return err(code);
  }
  // 其他标签页可能已重新登录；新会话的 token 不能授权旧宿主保存的命令或草稿。
  // 同宿主的写许可即使已被暂停，有效的异代正文仍要先交给宿主撤销 Key 展示。
  if (!adminSessionSameIdentity(expected, current.value)) {
    if (review === null) {
      requestAdminSessionCheck(host);
    } else {
      review.observe(current.value);
    }
    return err("unauthenticated");
  }
  if (!writeSessionAuthorized(host, expected, current.value.csrfToken)) {
    return err("unauthenticated");
  }
  return ok(current.value.csrfToken);
}

/** gloo 的 `Response::ok`：状态码 200 到 299。 */
function responseOk(response: TransportResponse): boolean {
  return response.status >= 200 && response.status <= 299;
}
