// 价格历史读取；失败统一为不带服务端细节、可重试的错误。

import type { PublicPriceHistoryListResponse } from "@auction/contracts/dto/public/catalog/history";
import { type Result, unreachable } from "@auction/web-shared/result";
import { decodePriceHistoryPage } from "../decoder/catalog";
import type { PriceHistoryQuery } from "../model";
import { CATALOG_TTL_MILLIS, getOrFetch } from "../swr-cache";
import { conditionalGet, failureOutcome } from "./cached-get";

/** 价格历史读取失败。服务端只以 503 表示读端口不可用，不区分状态码：页面统一提示并可重试，
 * 也不把服务端 message、响应正文或底层网络错误带进组件。 */
export type PriceHistoryFetchError = { readonly kind: "price_history_fetch_error" };

/** 价格历史读取失败的共享值。 */
export const PRICE_HISTORY_FETCH_ERROR: PriceHistoryFetchError = {
  kind: "price_history_fetch_error",
};

/** 价格历史页的缓存投影标识。 */
const PRICE_HISTORY_CACHE_TYPE = "public_price_history_page";

/** 读取价格历史的一页；失败统一为可重试的固定错误，不携带服务端细节。 */
export async function fetchPriceHistory(
  query: PriceHistoryQuery,
  pageSize: number,
  signal: AbortSignal,
): Promise<Result<PublicPriceHistoryListResponse, PriceHistoryFetchError>> {
  const params = new URLSearchParams();
  const optional: ReadonlyArray<readonly [string, string | null]> = [
    ["world", query.world],
    ["name", query.name],
    ["from", query.from],
    ["to", query.to],
    ["min_price", query.minPrice],
    ["max_price", query.maxPrice],
    ["buyer", query.buyer],
    ["seller", query.seller],
  ];
  for (const [key, value] of optional) {
    if (value !== null && value !== "") {
      params.append(key, value);
    }
  }
  params.append("sort", query.sort);
  params.append("page", String(Math.max(1, query.page)));
  params.append("page_size", String(pageSize));
  const url = `/api/v1/price-history?${params.toString()}`;
  const requestUrl = url;
  return getOrFetch<PublicPriceHistoryListResponse, PriceHistoryFetchError>(
    url,
    CATALOG_TTL_MILLIS,
    PRICE_HISTORY_CACHE_TYPE,
    async (etag, evictMissingSnapshot) => {
      const conditional = await conditionalGet(requestUrl, etag, evictMissingSnapshot, signal);
      if (signal.aborted) {
        return { kind: "transport", error: PRICE_HISTORY_FETCH_ERROR };
      }
      if (!conditional.ok) {
        return { kind: "transport", error: PRICE_HISTORY_FETCH_ERROR };
      }
      const outcome = conditional.value;
      switch (outcome.kind) {
        case "not_modified":
          return { kind: "not_modified" };
        case "failure":
          return failureOutcome<PublicPriceHistoryListResponse, PriceHistoryFetchError>(
            outcome.status,
            PRICE_HISTORY_FETCH_ERROR,
          );
        case "success": {
          const decoded = decodePriceHistoryPage(outcome.response);
          return decoded.ok
            ? { kind: "changed", value: decoded.value, etag: outcome.etag }
            : { kind: "rejected", error: PRICE_HISTORY_FETCH_ERROR };
        }
        default:
          return unreachable(outcome);
      }
    },
    PRICE_HISTORY_FETCH_ERROR,
    signal,
  );
}
