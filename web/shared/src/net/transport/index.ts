// 同源请求的共享传输边界。
// 唯一的 fetch 实现：默认 10 秒、可按单次请求指定的取消预算与结构化 TransportError 分类。
// Admin 的 CSRF token 与 Basic Auth 不进入本层：调用方在 build 闭包里建立请求时自行叠加 header，
// 本层只负责发送、超时取消与错误分类。

import {
  observeServerBuildId,
  responseReportsCurrentBuild,
  SERVER_BUILD_HEADER,
} from "@auction/web-shared/ui/build-notice";
import { createTimeoutFuture } from "../../platform/timer";
import { err, ok, type Result, unreachable } from "../../result";
import { type JsonFieldPolicy, parseJson } from "../wire";

/** 两端共用的默认请求超时上限（恢复旧版 10 秒语义）。 */
export const DEFAULT_REQUEST_TIMEOUT_MILLIS = 10_000;

/** 共享传输层的结构化错误。 */
export type TransportError =
  | { readonly kind: "timeout" }
  | { readonly kind: "network"; readonly status: number | null }
  | { readonly kind: "decode" };

/** 映射为两端契约中的稳定 transport code。 */
export function transportErrorCode(
  error: TransportError,
): "request_timeout" | "network_error" | "invalid_response" {
  switch (error.kind) {
    case "timeout":
      return "request_timeout";
    case "network":
      return "network_error";
    case "decode":
      return "invalid_response";
    default:
      return unreachable(error);
  }
}

/** 响应头到达、正文尚未读取时可观察的元数据。 */
export interface TransportResponseMetadata {
  readonly status: number;
  readonly statusText: string;
  readonly headers: Headers;
}

/** 正文已完整读入内存的传输层响应；没有自动重试。 */
export interface TransportResponse extends TransportResponseMetadata {
  /** 204/205/304 按 Fetch 约定为 `null`，其余状态为完整正文字节。 */
  readonly body: Uint8Array | null;
  /** 把正文按 UTF-8 解码；无正文时返回空字符串。 */
  text(): string;
  /** 把正文解析为严格 JSON；无正文或不符合源码拒绝边界时返回 decode 错误。 */
  json(policy?: JsonFieldPolicy): Result<unknown, TransportError>;
}

/** 响应头到达时调用；正文仍须在同一超时预算内完整读取。 */
export type ResponseObserver = (response: TransportResponseMetadata) => void;

/** 调用方构造的请求；取消信号由传输层统一附加。 */
export interface TransportRequest {
  readonly url: string;
  readonly init?: Omit<RequestInit, "signal">;
}

/** 在请求发出前构造 URL、方法、header 与请求体；可在 build 内读取本次取消信号。 */
export type TransportRequestBuilder = (
  signal: AbortSignal,
) => Result<TransportRequest, TransportError>;

/** 构造响应对象；正文字节已经完整读入，后续解码不再等网络。 */
function transportResponse(
  status: number,
  statusText: string,
  headers: Headers,
  body: Uint8Array | null,
): TransportResponse {
  return {
    status,
    statusText,
    headers,
    body,
    text() {
      return body === null ? "" : new TextDecoder().decode(body);
    },
    json(policy) {
      if (body === null) {
        return err({ kind: "decode" });
      }
      const decoded = parseJson(new TextDecoder().decode(body), policy);
      return decoded.ok ? decoded : err({ kind: "decode" });
    },
  };
}

/** 只在 `no-store` 响应上比对服务端构建号；可缓存响应可能是旧副本，不代表当前进程。 */
function observeResponseBuild(headers: Headers): void {
  if (!responseReportsCurrentBuild(headers.get("cache-control"))) {
    return;
  }
  const build = headers.get(SERVER_BUILD_HEADER);
  if (build !== null) {
    observeServerBuildId(build);
  }
}

/** 发送一次 GET 并保留调用方给出的精确 URL。 */
export function sendGet(
  url: string,
  signal?: AbortSignal,
): Promise<Result<TransportResponse, TransportError>> {
  return sendGetObserved(url, () => {}, signal);
}

/** 发送 GET，并在响应头到达时观察元数据；正文仍须完整读取。 */
export function sendGetObserved(
  url: string,
  observe: ResponseObserver,
  signal?: AbortSignal,
): Promise<Result<TransportResponse, TransportError>> {
  return sendRequestObserved(() => ok({ url, init: { method: "GET" } }), observe, signal);
}

/** 在共享超时上限内完整读取响应头与正文；调用方信号中止时终止 fetch。 */
export function sendRequest(
  build: TransportRequestBuilder,
  signal?: AbortSignal,
): Promise<Result<TransportResponse, TransportError>> {
  return sendRequestWithTimeout(build, DEFAULT_REQUEST_TIMEOUT_MILLIS, signal);
}

/** 为单次请求显式指定预算；复用相同的正文读取、取消和错误边界，不添加自动重试。 */
export function sendRequestWithTimeout(
  build: TransportRequestBuilder,
  timeoutMillis: number,
  signal?: AbortSignal,
): Promise<Result<TransportResponse, TransportError>> {
  return sendRequestObservedWithTimeout(build, () => {}, timeoutMillis, signal);
}

/** 允许 surface 在响应头到达时校时，避免把正文传输耗时误算为时钟偏差。 */
export function sendRequestObserved(
  build: TransportRequestBuilder,
  observe: ResponseObserver,
  signal?: AbortSignal,
): Promise<Result<TransportResponse, TransportError>> {
  return sendRequestObservedWithTimeout(build, observe, DEFAULT_REQUEST_TIMEOUT_MILLIS, signal);
}

/** 共享实现的最后一道边界：构造、发送、观察、完整读正文、超时与调用方取消。 */
async function sendRequestObservedWithTimeout(
  build: TransportRequestBuilder,
  observe: ResponseObserver,
  timeoutMillis: number,
  signal?: AbortSignal,
): Promise<Result<TransportResponse, TransportError>> {
  if (signal?.aborted === true) {
    return err({ kind: "network", status: null });
  }
  const controller = new AbortController();
  const abortFromCaller = () => controller.abort();
  signal?.addEventListener("abort", abortFromCaller, { once: true });
  const cleanupCaller = () => {
    signal?.removeEventListener("abort", abortFromCaller);
  };

  let built: Result<TransportRequest, TransportError>;
  try {
    built = build(controller.signal);
  } catch (error) {
    controller.abort();
    cleanupCaller();
    throw error;
  }
  if (!built.ok) {
    cleanupCaller();
    return built;
  }

  const request = built.value;
  const timeout = createTimeoutFuture(timeoutMillis);
  let inFlightResponse: Response | null = null;
  const send = (async (): Promise<Result<TransportResponse, TransportError>> => {
    let response: Response;
    try {
      response = await fetch(request.url, { ...request.init, signal: controller.signal });
    } catch {
      return err({ kind: "network", status: null });
    }
    inFlightResponse = response;
    const metadata: TransportResponseMetadata = {
      status: response.status,
      statusText: response.statusText,
      headers: new Headers(response.headers),
    };
    observeResponseBuild(metadata.headers);
    observe(metadata);
    let bytes: Uint8Array;
    try {
      bytes = new Uint8Array(await response.arrayBuffer());
    } catch {
      return err({ kind: "network", status: metadata.status });
    }
    if (metadata.status < 200 || metadata.status > 599) {
      return err({ kind: "decode" });
    }
    const body =
      metadata.status === 204 || metadata.status === 205 || metadata.status === 304 ? null : bytes;
    return ok(transportResponse(metadata.status, metadata.statusText, metadata.headers, body));
  })();

  const abortAndSettle = async (): Promise<void> => {
    controller.abort();
    const response = inFlightResponse;
    if (response !== null && response.body !== null) {
      try {
        await response.body.cancel();
      } catch {
        // 正文已锁定或已读完时取消会失败；abort 与 send 收口仍会完成清理。
      }
    }
    await send.catch(() => undefined);
  };

  let outcome:
    | { kind: "sent"; result: Result<TransportResponse, TransportError> }
    | { kind: "timeout" };
  try {
    outcome = await Promise.race([
      send.then((result) => ({ kind: "sent" as const, result })),
      timeout.promise.then(() => ({ kind: "timeout" as const })),
    ]);
  } catch (error) {
    // 观察回调与响应构造不属于可分类的网络失败；取消并收口在途请求后再抛出原异常。
    timeout.cancel();
    cleanupCaller();
    await abortAndSettle();
    throw error;
  }
  timeout.cancel();
  if (outcome.kind === "sent") {
    cleanupCaller();
    return outcome.result;
  }

  // 超时后先取消 fetch，再消费在途请求与正文读取的终止结果，之后才允许外层锁释放。
  cleanupCaller();
  await abortAndSettle();
  return err({ kind: "timeout" });
}
