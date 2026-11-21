// lot 目录读取（首次未得到权威值时的单次重读、首屏预取与唯一 URL 构造点）与 lot 详情读取。

import type { PublicLotEnvelopeResponse } from "@auction/contracts/dto/public/catalog/lot";
import { sleep } from "@auction/web-shared/platform/timer";
import { documentIsVisible } from "@auction/web-shared/platform/visibility";
import type { Result } from "@auction/web-shared/result";
import { err, ok, unreachable } from "@auction/web-shared/result";
import { decodeLotDetail, decodeLotList } from "../decoder/lot";
import type { CatalogLevelFilter } from "../model";
import type { CacheRead } from "../swr-cache";
import { deliverCacheRead, getOrFetch, LIVE_TTL_MILLIS, publicReadRetryAfter } from "../swr-cache";
import { cachedPublicRead, conditionalGet, failureOutcome } from "./cached-get";

const PUBLIC_QUERY_RETRY_DELAY_MILLIS = 1_000;
const MAX_CATALOG_RETRY_DELAY_MILLIS = 60_000;

/** lot 外壳/列表的缓存投影标识。 */
const LOT_ENVELOPE_CACHE_TYPE = "public_lot_envelope";

/** 详情只区分 HTTP 明确不存在与暂时读不到，不带入错误正文。 */
export type LotDetailFetchError = {
  readonly kind: "lot_detail_fetch_error" | "not_found";
};

/** 暂时未取得有效详情。 */
export const LOT_DETAIL_FETCH_ERROR: LotDetailFetchError = { kind: "lot_detail_fetch_error" };

/** 读取目录的一页；在可见时按服务端提示的时限最多自动补读一次。 */
export async function fetchCurrentLots(
  auctionId: string,
  category: string | null,
  equipmentGroup: string | null,
  levels: CatalogLevelFilter,
  page: number,
  pageSize: number,
  signal: AbortSignal,
): Promise<Result<PublicLotEnvelopeResponse, string>> {
  const url = currentLotsUrl(auctionId, category, equipmentGroup, levels, page, pageSize);
  let retryAt: number | null = null;
  const read = (canSend?: () => boolean) =>
    fetchCurrentLotsOnce(
      url,
      auctionId,
      category,
      equipmentGroup,
      levels,
      signal,
      (notBefore) => {
        retryAt = notBefore;
      },
      canSend,
    );
  return retryOnceUnlessAuthoritative(
    read,
    async () => {
      const shared = publicReadRetryAfter(url)?.notBefore ?? null;
      const delay =
        shared === null && retryAt === null
          ? PUBLIC_QUERY_RETRY_DELAY_MILLIS
          : Math.ceil(Math.max(0, Math.max(shared ?? 0, retryAt ?? 0) - performance.now()));
      // 超过等待上限就交付首次结果，避免在服务端仍拒绝的窗口内提前重读。
      if (
        delay > MAX_CATALOG_RETRY_DELAY_MILLIS ||
        !documentIsVisible() ||
        !(await sleep(delay, signal)) ||
        !documentIsVisible()
      ) {
        return null;
      }
      let skipped = false;
      const second = await read(() => {
        // 同 URL 的在途读可能延后补读。必须在实际发送边界复核可见性。
        skipped = !documentIsVisible();
        return !skipped;
      });
      return skipped ? null : second;
    },
    "network_error",
    signal,
  );
}

/** 首次权威值直接交付；未得到权威值可在等待后补读一次。等待结束仍不能补读时交付首次结果，
 * 允许旧快照继续展示。中止优先于回退，第二次读取仍执行相同的失败关闭规则。导出供测试。 */
export async function retryOnceUnlessAuthoritative<T, E>(
  read: () => Promise<Result<CacheRead<T, E>, E>>,
  retry: () => Promise<Result<CacheRead<T, E>, E> | null>,
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
  const second = await retry();
  if (signal.aborted) {
    return err(abortError);
  }
  return deliverCacheRead(second ?? first);
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
  const url = currentLotsUrl(auctionId, category, equipmentGroup, levels, page, pageSize);
  await fetchCurrentLotsOnce(url, auctionId, category, equipmentGroup, levels, signal);
}

async function fetchCurrentLotsOnce(
  url: string,
  auctionId: string,
  category: string | null,
  equipmentGroup: string | null,
  levels: CatalogLevelFilter,
  signal: AbortSignal,
  observeRetryAfter: (notBefore: number) => void = () => {},
  canSend?: () => boolean,
): Promise<Result<CacheRead<PublicLotEnvelopeResponse>, string>> {
  return cachedPublicRead(
    url,
    LIVE_TTL_MILLIS,
    LOT_ENVELOPE_CACHE_TYPE,
    (response) => decodeLotList(response, auctionId, category, equipmentGroup, levels),
    signal,
    observeRetryAfter,
    canSend,
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
  return `/api/v1/auctions/${encodeURIComponent(auctionId)}/lots?${query.toString()}`;
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
