// lot 目录读取（首次未得到权威值时的单次重读、首屏预取与唯一 URL 构造点）与 lot 详情读取。

import type { PublicLotEnvelopeResponse } from "@auction/contracts/dto/public/catalog/lot";
import { sleep } from "@auction/web-shared/platform/timer";
import type { Result } from "@auction/web-shared/result";
import { err, ok, unreachable } from "@auction/web-shared/result";
import { decodeLotDetail, decodeLotList } from "../decoder/lot";
import type { CatalogLevelFilter } from "../model";
import type { CacheRead } from "../swr-cache";
import { getOrFetch, LIVE_TTL_MILLIS } from "../swr-cache";
import { cachedPublicRead, conditionalGet, failureOutcome } from "./cached-get";

const PUBLIC_QUERY_RETRY_DELAY_MILLIS = 1_000;

/** lot 外壳/列表的缓存投影标识。 */
const LOT_ENVELOPE_CACHE_TYPE = "public_lot_envelope";

/** 详情只区分 HTTP 明确不存在与暂时读不到，不带入错误正文。 */
export type LotDetailFetchError = {
  readonly kind: "lot_detail_fetch_error" | "not_found";
};

/** 暂时未取得有效详情。 */
export const LOT_DETAIL_FETCH_ERROR: LotDetailFetchError = { kind: "lot_detail_fetch_error" };

/** 读取当前场次目录的一页；首次没拿到权威值时按旧实现等待一秒后再读一次。 */
export async function fetchCurrentLots(
  auctionId: string,
  category: string | null,
  equipmentGroup: string | null,
  levels: CatalogLevelFilter,
  page: number,
  pageSize: number,
  signal: AbortSignal,
): Promise<Result<PublicLotEnvelopeResponse, string>> {
  return retryOnceUnlessAuthoritative(
    () => fetchCurrentLotsOnce(auctionId, category, equipmentGroup, levels, page, pageSize, signal),
    () => wait(PUBLIC_QUERY_RETRY_DELAY_MILLIS, signal),
    "network_error",
    signal,
  );
}

/** 旧 Public QueryClient 对目录读取固定再尝试一次，错误壳只在第二次失败后出现。
 * 首次读到权威值（新鲜命中、304 确认或新响应）直接返回；首次失败，或只得到传输失败、暂时性
 * 拒绝后回退的旧快照时，等待后重读一次：等待期间 Resource 保留已展示的目录，拥塞通常已过，
 * 能及时追上价格与领先者。第二次结果原样返回：再次回退时接受旧快照，失败关闭的拒绝照常报错。
 * wait 在首次读取结束后才构造，浏览器定时器从那一刻起计时。 */
/** 仅供测试使用：以注入的读取与等待函数核对首次回退后的单次重读。 */
export async function retryOnceUnlessAuthoritative<T, E>(
  read: () => Promise<Result<CacheRead<T>, E>>,
  wait: () => Promise<boolean>,
  abortError: E,
  signal: AbortSignal,
): Promise<Result<T, E>> {
  const first = await read();
  if (signal.aborted) {
    return err(abortError);
  }
  if (first.ok && first.value.staleFallback === null) {
    return ok(first.value.value);
  }
  if (!(await wait())) {
    return err(abortError);
  }
  const second = await read();
  if (signal.aborted) {
    return err(abortError);
  }
  return second.ok ? ok(second.value.value) : second;
}

/** 首屏预取：与 /auctions/current 并行读取上次记住的场次 lots，只单次尝试、丢弃结果。目录随后
 * 以相同参数读取时由缓存层复用这次在途或已写入的结果；场次已变化时只多一次请求。 */
export async function prefetchCurrentLots(
  auctionId: string,
  category: string | null,
  equipmentGroup: string | null,
  levels: CatalogLevelFilter,
  page: number,
  pageSize: number,
  signal: AbortSignal,
): Promise<void> {
  await fetchCurrentLotsOnce(auctionId, category, equipmentGroup, levels, page, pageSize, signal);
}

async function fetchCurrentLotsOnce(
  auctionId: string,
  category: string | null,
  equipmentGroup: string | null,
  levels: CatalogLevelFilter,
  page: number,
  pageSize: number,
  signal: AbortSignal,
): Promise<Result<CacheRead<PublicLotEnvelopeResponse>, string>> {
  const url = currentLotsUrl(auctionId, category, equipmentGroup, levels, page, pageSize);
  return cachedPublicRead(
    url,
    LIVE_TTL_MILLIS,
    LOT_ENVELOPE_CACHE_TYPE,
    (response) => decodeLotList(response, auctionId, category, equipmentGroup, levels),
    signal,
  );
}

/** lot 目录 URL 的唯一构造点：读取、首屏预取、标记页与历史场次都经这里，同一组参数得到逐字
 * 相同的 URL，缓存层才能按 URL 合并在途请求。目录行不展示装备属性，始终请求精简投影
 * （fields=summary）；属性明细只在打开详情抽屉时经 lot 详情读取。 */
/** 仅供测试使用：核对目录读取唯一 URL 构造点。 */
export function currentLotsUrl(
  auctionId: string,
  category: string | null,
  equipmentGroup: string | null,
  levels: CatalogLevelFilter,
  page: number,
  pageSize: number,
): string {
  const query = new URLSearchParams();
  query.append("page", String(page));
  query.append("page_size", String(pageSize));
  if (category !== null) {
    query.append("category", category);
  }
  if (equipmentGroup !== null) {
    query.append("equipment_group", equipmentGroup);
  }
  if (levels.maxLevel !== null) {
    query.append("max_level", String(levels.maxLevel));
  }
  if (!levels.includeUnleveled) {
    query.append("include_unleveled", "false");
  }
  query.append("fields", "summary");
  return `/api/v1/auctions/${auctionId}/lots?${query.toString()}`;
}

/** 读取单个 lot 的详情外壳；与目录共用同一投影类型标识，因为两者都是 lot 外壳响应，
 * 且 URL 不相交，不会互相命中。 */
export async function fetchLotDetail(
  lotId: string,
  signal: AbortSignal,
): Promise<Result<PublicLotEnvelopeResponse, LotDetailFetchError>> {
  const url = `/api/v1/lots/${lotId}`;
  const requestUrl = url;
  return getOrFetch<PublicLotEnvelopeResponse, LotDetailFetchError>(
    url,
    LIVE_TTL_MILLIS,
    LOT_ENVELOPE_CACHE_TYPE,
    async (etag, evictMissingSnapshot) => {
      const conditional = await conditionalGet(requestUrl, etag, evictMissingSnapshot, signal);
      if (signal.aborted) {
        return { kind: "transport", error: LOT_DETAIL_FETCH_ERROR };
      }
      if (!conditional.ok) {
        return { kind: "transport", error: LOT_DETAIL_FETCH_ERROR };
      }
      const outcome = conditional.value;
      switch (outcome.kind) {
        case "not_modified":
          return { kind: "not_modified" };
        case "failure":
          return failureOutcome<PublicLotEnvelopeResponse, LotDetailFetchError>(
            outcome.status,
            outcome.status === 404 || outcome.status === 410
              ? { kind: "not_found" }
              : LOT_DETAIL_FETCH_ERROR,
          );
        case "success": {
          const decoded = decodeLotDetail(outcome.response, null, lotId);
          return decoded.ok
            ? { kind: "changed", value: decoded.value, etag: outcome.etag }
            : { kind: "rejected", error: LOT_DETAIL_FETCH_ERROR };
        }
        default:
          return unreachable(outcome);
      }
    },
    LOT_DETAIL_FETCH_ERROR,
    signal,
  );
}

async function wait(milliseconds: number, signal: AbortSignal): Promise<boolean> {
  return sleep(milliseconds, signal);
}
