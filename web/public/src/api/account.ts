// Public 账户读取 DTO 与同源请求。
// 这里只解码后端的安全账户 projection；代理上限、历史转换和 capability 仍以服务端返回的公开
// 字段为准，浏览器不重建代理状态机。

import type {
  PublicMyBidsResponse,
  PublicOwnedBidResponse,
  PublicOwnedProxyResponse,
} from "@auction/contracts/dto/public/account";
import { PUBLIC_EXPECTED_USER_ID_HEADER } from "@auction/contracts/dto/public/commands";
import type { TransportResponse } from "@auction/web-shared/net/transport";
import { transportErrorCode } from "@auction/web-shared/net/transport";
import { sleep } from "@auction/web-shared/platform/timer";
import { err, ok, type Result } from "@auction/web-shared/result";
import { sendRequest } from "./clock";
import { decodeErrorCode } from "./decoder";
import { decodeMyBids, decodeMyProxy } from "./decoder/account";
import { observePublicSessionFailure } from "./session";
import { publicSessionIdentityRevision } from "./session/revision";

const PUBLIC_QUERY_RETRY_DELAY_MILLIS = 1_000;

/** 我的出价与代理授权分页投影。 */
export type MyBidsPage = PublicMyBidsResponse;
/** 单条已中标的公开投影。 */
export type OwnedBid = PublicOwnedBidResponse;
/** 单条代理授权投影。 */
export type OwnedProxy = PublicOwnedProxyResponse;

/** 单次账户读取：失败时附带是否值得一秒后重读；4xx 与解码失败是权威结论。 */
interface AccountAttempt<T> {
  readonly result: Result<T, string>;
  readonly transient: boolean;
}

/** 读取我的出价与代理授权；传输失败或服务端暂时拒绝后等待一秒重试一次，等待可被取消。 */
export async function fetchMyBids(
  userId: string,
  bidPage: number,
  proxyPage: number,
  signal: AbortSignal,
): Promise<Result<PublicMyBidsResponse, string>> {
  const url = `/api/v1/me/bids?bid_page=${bidPage}&page_size=10&proxy_page=${proxyPage}`;
  const first = await fetchMyBidsOnce(url, userId, signal);
  if (signal.aborted) {
    return err("network_error");
  }
  if (first.result.ok || !first.transient) {
    return first.result;
  }
  if (!(await sleep(PUBLIC_QUERY_RETRY_DELAY_MILLIS, signal))) {
    return err("network_error");
  }
  const second = await fetchMyBidsOnce(url, userId, signal);
  return signal.aborted ? err("network_error") : second.result;
}

async function fetchMyBidsOnce(
  url: string,
  userId: string,
  signal: AbortSignal,
): Promise<AccountAttempt<PublicMyBidsResponse>> {
  const response = await accountResponse(url, userId, signal);
  if (signal.aborted) {
    return { result: err("network_error"), transient: false };
  }
  if (!response.result.ok) {
    return { result: err(response.result.error), transient: response.transient };
  }
  return { result: decodeMyBids(response.result.value), transient: false };
}

/** 读取指定 lot 上我的代理授权；首次失败后等待一秒重试一次，等待可被取消。 */
export async function fetchMyProxy(
  userId: string,
  lotId: string,
  signal: AbortSignal,
): Promise<Result<PublicOwnedProxyResponse | null, string>> {
  const url = `/api/v1/lots/${lotId}/proxy-authorizations/mine`;
  const first = await fetchMyProxyOnce(url, userId, lotId, signal);
  if (signal.aborted) {
    return err("network_error");
  }
  if (first.result.ok || !first.transient) {
    return first.result;
  }
  if (!(await sleep(PUBLIC_QUERY_RETRY_DELAY_MILLIS, signal))) {
    return err("network_error");
  }
  const second = await fetchMyProxyOnce(url, userId, lotId, signal);
  return signal.aborted ? err("network_error") : second.result;
}

async function fetchMyProxyOnce(
  url: string,
  userId: string,
  lotId: string,
  signal: AbortSignal,
): Promise<AccountAttempt<PublicOwnedProxyResponse | null>> {
  const response = await accountResponse(url, userId, signal);
  if (signal.aborted) {
    return { result: err("network_error"), transient: false };
  }
  if (!response.result.ok) {
    return { result: err(response.result.error), transient: response.transient };
  }
  const decoded = decodeMyProxy(response.result.value, lotId);
  return {
    result: decoded.ok ? ok(decoded.value.authorization) : decoded,
    transient: false,
  };
}

/** 响应不携带用户 UUID，必须由同一请求的 Cookie 与预期宿主身份校验绑定归属。 */
async function accountResponse(
  url: string,
  userId: string,
  signal: AbortSignal,
): Promise<AccountAttempt<TransportResponse>> {
  const identityAtStart = publicSessionIdentityRevision();
  const sent = await sendRequest(
    () =>
      ok({
        url,
        init: {
          method: "GET",
          headers: { [PUBLIC_EXPECTED_USER_ID_HEADER]: userId },
        },
      }),
    signal,
  );
  if (!sent.ok) {
    return {
      result: err(transportErrorCode(sent.error)),
      transient: sent.error.kind !== "decode",
    };
  }
  if (signal.aborted) {
    return { result: err("network_error"), transient: false };
  }
  const response = sent.value;
  if (!responseOk(response)) {
    const status = response.status;
    const code = decodeErrorCode(response);
    observePublicSessionFailure(status, code, userId, identityAtStart);
    return { result: err(code), transient: status === 429 || status >= 500 };
  }
  return { result: ok(response), transient: false };
}

function responseOk(response: { readonly status: number }): boolean {
  return response.status >= 200 && response.status < 300;
}
