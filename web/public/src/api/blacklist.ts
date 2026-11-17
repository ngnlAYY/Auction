// Public 黑名单 HTTP DTO 与请求。

import type {
  PublicBlacklistEntryResponse,
  PublicBlacklistResponse,
} from "@auction/contracts/dto/public/account";
import { transportErrorCode } from "@auction/web-shared/net/transport";
import { err, type Result } from "@auction/web-shared/result";
import { sendGet } from "./clock";
import { decodeErrorCode } from "./decoder";
import { decodeBlacklist } from "./decoder/catalog";

/** 单条公开黑名单记录。 */
export type BlacklistEntry = PublicBlacklistEntryResponse;
/** 公开黑名单的一页。 */
export type BlacklistPage = PublicBlacklistResponse;

/** 读取公开黑名单；页号小于一时按第一页请求。 */
export async function fetchBlacklist(
  page: number,
  pageSize: number,
  signal: AbortSignal,
): Promise<Result<PublicBlacklistResponse, string>> {
  const normalizedPage = Math.max(1, page);
  const sent = await sendGet(
    `/api/v1/blacklist?page=${normalizedPage}&page_size=${pageSize}`,
    signal,
  );
  if (!sent.ok) {
    return err(transportErrorCode(sent.error));
  }
  if (signal.aborted) {
    return err("network_error");
  }
  const response = sent.value;
  if (!responseOk(response)) {
    return err(decodeErrorCode(response));
  }
  return decodeBlacklist(response);
}

function responseOk(response: { readonly status: number }): boolean {
  return response.status >= 200 && response.status < 300;
}
