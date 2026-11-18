// 公开只读 GET 的条件请求、非 2xx 响应的缓存分类与带缓存的严格解码；其余查询都经这里读写
// 公开读缓存。

import type {
  ResponseObserver,
  TransportError,
  TransportResponse,
  TransportResponseMetadata,
} from "@auction/web-shared/net/transport";
import { transportErrorCode } from "@auction/web-shared/net/transport";
import type { Result } from "@auction/web-shared/result";
import { err, ok, unreachable } from "@auction/web-shared/result";
import { parseImfFixdate, sendGetConditional } from "../clock";
import { decodeErrorCode } from "../decoder";
import {
  type CacheRead,
  deliverCacheRead,
  type FetchOutcome,
  publicReadRetryAfter,
  readOrFetch,
  rememberPublicReadRetryAfter,
} from "../swr-cache";

/** 条件 GET 的响应分类；304 只在确实发送了 If-None-Match 时成立。 */
export type ConditionalResponse =
  | { readonly kind: "not_modified" }
  | {
      readonly kind: "success";
      readonly response: TransportResponse;
      readonly etag: string | null;
    }
  | {
      readonly kind: "failure";
      readonly status: number;
      readonly result: Result<TransportResponse, TransportError>;
    };

/** 发送条件 GET：有 ETag 时带 `If-None-Match`，否则要求浏览器向服务端再验证；`signal` 中止时
 * 传输层取消请求与正文读取。 */
export async function conditionalGet(
  url: string,
  etag: string | null,
  evictMissingSnapshot: () => void,
  signal: AbortSignal,
  observeResponse?: ResponseObserver,
): Promise<Result<ConditionalResponse, TransportError>> {
  let status: number | null = null;
  const sent = await sendGetConditional(
    url,
    etag,
    (observed) => {
      status = observed.status;
      // 404/410 一到就撤销旧快照；读取正文时关闭页面也不能恢复其回退资格。
      if (missingStatus(observed.status)) {
        evictMissingSnapshot();
      }
      observeResponse?.(observed);
    },
    signal,
  );
  if (!sent.ok) {
    // 超时本身不携带 HTTP 状态；以已收到的响应头区分明确拒绝和可回退的传输失败。
    if (status !== null && !responseOk({ status })) {
      return ok({ kind: "failure", status, result: sent });
    }
    return err(sent.error);
  }
  if (signal.aborted) {
    return err({ kind: "network", status: null });
  }
  const response = sent.value;
  if (response.status === 304) {
    return ok(
      etag !== null
        ? { kind: "not_modified" }
        : // 无 ETag 的请求不应收到 304；交给错误 envelope 解码归一为 invalid_response。
          { kind: "failure", status: response.status, result: sent },
    );
  }
  if (!responseOk(response)) {
    return ok({ kind: "failure", status: response.status, result: sent });
  }
  return ok({ kind: "success", response, etag: response.headers.get("ETag") });
}

/** 非 2xx 响应的缓存分类：429（读闸繁忙或来源超额）、503（读端口暂不可用），以及源站不可达时
 * 网关返回的 502/504 与 CDN 的 520–524，都是暂时性拒绝，与断网同样处理：缓存层有可回退的旧
 * 快照时继续展示。其余状态（含无 ETag 却收到的 304、500 与业务拒绝）失败关闭。 */
export function failureOutcome<T, E>(status: number, error: E): FetchOutcome<T, E> {
  if (missingStatus(status)) {
    return { kind: "missing", error };
  }
  if (
    status === 429 ||
    status === 502 ||
    status === 503 ||
    status === 504 ||
    (status >= 520 && status <= 524)
  ) {
    return { kind: "busy", error };
  }
  return { kind: "rejected", error };
}

function missingStatus(status: number): boolean {
  return status === 404 || status === 410;
}

/** 对公开只读 GET 执行带缓存的严格解码。decode 必须完成全部跨字段校验（含响应与请求身份的
 * 绑定）；只有它返回成功的正文才写入缓存，4xx/5xx 或校验失败不会留下条目。 */
export async function cachedPublicGet<T>(
  url: string,
  ttlMillis: number,
  typeId: string,
  decode: (response: TransportResponse) => Result<T, string>,
  signal: AbortSignal,
): Promise<Result<T, string>> {
  const read = await cachedPublicRead(url, ttlMillis, typeId, decode, signal);
  if (signal.aborted) {
    return err("network_error");
  }
  return deliverCacheRead(read);
}

/** 与 cachedPublicGet 相同，但保留“暂时性失败后回退旧快照”的标记与确认时刻，供需要稍后再读
 * 的目录读取与须标明数据时点的页面使用。实际请求须在取得读取资格后再检查调用方的发送门禁。 */
export async function cachedPublicRead<T>(
  url: string,
  ttlMillis: number,
  typeId: string,
  decode: (response: TransportResponse) => Result<T, string>,
  signal: AbortSignal,
  observeRetryAfter?: (notBefore: number) => void,
  canSend?: () => boolean,
): Promise<Result<CacheRead<T>, string>> {
  const requestUrl = url;
  const read = await readOrFetch(
    requestUrl,
    ttlMillis,
    typeId,
    async (etag, evictMissingSnapshot) => {
      if (canSend?.() === false) {
        return { kind: "rejected", error: "network_error" };
      }
      // 进到这里才取得同 URL 的读取权。等待预取结束的读也必须检查已公布的时限。
      const blocked = publicReadRetryAfter(requestUrl);
      if (blocked !== null) {
        observeRetryAfter?.(blocked.notBefore);
        const failure = failureOutcome<T, string>(blocked.status, blocked.error);
        return failure.kind === "busy" ? failure : { kind: "rejected", error: blocked.error };
      }
      const observed: { window?: ReturnType<typeof rememberPublicReadRetryAfter> } = {};
      const observeResponse =
        observeRetryAfter === undefined
          ? undefined
          : (response: TransportResponseMetadata) => {
              const delay = requestedRetryDelay(response);
              if (delay === null) return;
              observed.window = rememberPublicReadRetryAfter(requestUrl, response.status, delay);
              observeRetryAfter(observed.window.notBefore);
            };
      const conditional = await conditionalGet(
        requestUrl,
        etag,
        evictMissingSnapshot,
        signal,
        observeResponse,
      );
      if (signal.aborted) {
        return { kind: "transport", error: "network_error" };
      }
      if (!conditional.ok) {
        return { kind: "transport", error: transportErrorCode(conditional.error) };
      }
      const outcome = conditional.value;
      switch (outcome.kind) {
        case "not_modified":
          return { kind: "not_modified" };
        case "failure": {
          const error = outcome.result.ok
            ? decodeErrorCode(outcome.result.value)
            : transportErrorCode(outcome.result.error);
          if (observed.window !== undefined) observed.window.error = error;
          return failureOutcome<T, string>(outcome.status, error);
        }
        case "success": {
          const decoded = decode(outcome.response);
          return decoded.ok
            ? { kind: "changed", value: decoded.value, etag: outcome.etag }
            : { kind: "rejected", error: decoded.error };
        }
        default:
          return unreachable(outcome);
      }
    },
    "network_error",
    signal,
  );
  return signal.aborted ? err("network_error") : read;
}

function requestedRetryDelay(response: TransportResponseMetadata): number | null {
  const retryAfter = response.headers.get("Retry-After");
  if (response.status < 400 || retryAfter === null) return null;
  if (/^[0-9]+$/u.test(retryAfter)) return Number(retryAfter) * 1_000;
  const deadline = parseImfFixdate(retryAfter);
  if (deadline === null) return null;
  const serverDate = parseImfFixdate(response.headers.get("Date") ?? "");
  return Math.max(0, deadline - (serverDate ?? Date.now()));
}

function responseOk(response: { readonly status: number }): boolean {
  return response.status >= 200 && response.status < 300;
}
