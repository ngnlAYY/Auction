// 公开只读 GET 的条件请求、非 2xx 响应的缓存分类与带缓存的严格解码；其余查询都经这里读写
// 公开读缓存。

import type { TransportError, TransportResponse } from "@auction/web-shared/net/transport";
import { transportErrorCode } from "@auction/web-shared/net/transport";
import type { Result } from "@auction/web-shared/result";
import { err, ok, unreachable } from "@auction/web-shared/result";
import { sendGetConditional } from "../clock";
import { decodeErrorCode } from "../decoder";
import { type CacheRead, type FetchOutcome, readOrFetch } from "../swr-cache";

/** 条件 GET 的响应分类；304 只在确实发送了 If-None-Match 时成立。 */
export type ConditionalResponse =
  | { readonly kind: "not_modified" }
  | {
      readonly kind: "success";
      readonly response: TransportResponse;
      readonly etag: string | null;
    }
  | { readonly kind: "failure"; readonly response: TransportResponse };

/** 发送条件 GET：有 ETag 时带 `If-None-Match`，否则要求浏览器向服务端再验证；`signal` 中止时
 * 传输层取消请求与正文读取。 */
export async function conditionalGet(
  url: string,
  etag: string | null,
  signal: AbortSignal,
): Promise<Result<ConditionalResponse, TransportError>> {
  const sent = await sendGetConditional(url, etag, signal);
  if (!sent.ok) {
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
          { kind: "failure", response },
    );
  }
  if (!responseOk(response)) {
    return ok({ kind: "failure", response });
  }
  return ok({ kind: "success", response, etag: response.headers.get("ETag") });
}

/** 非 2xx 响应的缓存分类：429（读闸繁忙或来源超额）、503（读端口暂不可用），以及源站不可达时
 * 网关返回的 502/504 与 CDN 的 520–524，都是暂时性拒绝，与断网同样处理：缓存层有可回退的旧
 * 快照时继续展示。其余状态（含无 ETag 却收到的 304、500 与业务拒绝）失败关闭。 */
export function failureOutcome<T, E>(status: number, error: E): FetchOutcome<T, E> {
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
  return read.ok ? ok(read.value.value) : read;
}

/** 与 cachedPublicGet 相同，但保留“暂时性失败后回退旧快照”的标记与确认时刻，供需要稍后再读
 * 的目录读取与须标明数据时点的页面使用。 */
export async function cachedPublicRead<T>(
  url: string,
  ttlMillis: number,
  typeId: string,
  decode: (response: TransportResponse) => Result<T, string>,
  signal: AbortSignal,
): Promise<Result<CacheRead<T>, string>> {
  const requestUrl = url;
  const read = await readOrFetch(
    requestUrl,
    ttlMillis,
    typeId,
    async (etag) => {
      const conditional = await conditionalGet(requestUrl, etag, signal);
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
        case "failure":
          return failureOutcome<T, string>(
            outcome.response.status,
            decodeErrorCode(outcome.response),
          );
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

function responseOk(response: TransportResponse): boolean {
  return response.status >= 200 && response.status < 300;
}
