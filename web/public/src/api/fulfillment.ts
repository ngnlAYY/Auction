// Public 履约进度的 HTTP DTO 与请求：已结算场次目录与逐件履约状态页。

import type { AuctionSummaryResponse } from "@auction/contracts/dto/public/catalog/auction";
import type {
  PublicFulfillmentLotResponse,
  PublicFulfillmentPageResponse,
} from "@auction/contracts/dto/public/catalog/fulfillment";
import { err, ok, type Result } from "@auction/web-shared/result";
import { compareText } from "@auction/web-shared/text";
import { decodeFulfillmentPage } from "./decoder/fulfillment";
import { fetchBidLogAuctionStatus } from "./queries/bid-log";
import { cachedPublicRead } from "./queries/cached-get";
import { CATALOG_TTL_MILLIS } from "./swr-cache";

/** 单件履约进度投影。 */
export type FulfillmentLot = PublicFulfillmentLotResponse;
/** 履约进度的一页。 */
export type FulfillmentPage = PublicFulfillmentPageResponse;

/** 履约进度接口的每页数量上限；超过时服务端返回 400。页面按上限请求，自定义输入超出时就地提示。 */
export const MAX_FULFILLMENT_PAGE_SIZE = 100;

/** jiff `Timestamp` 能表示的最大毫秒数；超过后源实现不猜测时刻。 */
const MAX_CONFIRMATION_MILLIS = 253_402_207_200_999;

/** 履约进度页的缓存投影标识。 */
const FULFILLMENT_CACHE_TYPE = "public_fulfillment_page";

/** 一次履约进度读取：编号页，以及它是否为暂时性失败后回退的旧快照。 */
export interface FulfillmentRead {
  page: PublicFulfillmentPageResponse;
  /** 重读遇到网络失败或服务端暂时拒绝、展示的是旧快照时，给出该快照最近一次被服务端确认的
   * 时刻（RFC 3339 UTC，秒精度）；权威结果为 null。 */
  staleSince: string | null;
}

/** 读取一个世界全部已结算场次，按期号倒序；只有已结算场次才有履约进度。 */
export async function fetchFulfillmentAuctions(
  worldId: string,
  signal: AbortSignal,
): Promise<Result<AuctionSummaryResponse[], string>> {
  const result = await fetchBidLogAuctionStatus(worldId, "settled", signal);
  if (signal.aborted) {
    return err("network_error");
  }
  if (!result.ok) {
    return result;
  }
  const auctions = result.value;
  // 目录逐页读取期间新结算的场次会让后页重复出现前页的末尾行。
  auctions.sort((left, right) => {
    if (left.sequence !== right.sequence) {
      return right.sequence < left.sequence ? -1 : 1;
    }
    return compareText(right.id, left.id);
  });
  const deduped: AuctionSummaryResponse[] = [];
  for (const auction of auctions) {
    const previous = deduped[deduped.length - 1];
    if (previous === undefined || previous.id !== auction.id) {
      deduped.push(auction);
    }
  }
  return ok(deduped);
}

/** 读取单场的履约进度编号页；响应必须与请求的场次、世界与分页一致，否则按无效响应处理。 */
export async function fetchFulfillmentPage(
  auctionId: string,
  worldId: string,
  lot: string | null,
  page: number,
  pageSize: number,
  signal: AbortSignal,
): Promise<Result<FulfillmentRead, string>> {
  const normalizedPage = Math.max(1, page);
  const query = new URLSearchParams();
  query.append("page", String(normalizedPage));
  query.append("page_size", String(pageSize));
  query.append("world", worldId);
  if (lot !== null) {
    query.append("lot", lot);
  }
  const url = `/api/v1/auctions/${auctionId}/fulfillment?${query.toString()}`;
  const read = await cachedPublicRead(
    url,
    CATALOG_TTL_MILLIS,
    FULFILLMENT_CACHE_TYPE,
    (response) => decodeFulfillmentPage(response, auctionId, worldId, normalizedPage, pageSize),
    signal,
  );
  if (signal.aborted) {
    return err("network_error");
  }
  if (!read.ok) {
    return read;
  }
  // 确认时刻无法表示时不展示旧快照，按读取失败处理：页面显示错误与重试，不静默显示旧状态。
  let staleSince: string | null = null;
  if (read.value.staleFallback !== null) {
    const confirmedAt = confirmationTime(read.value.staleFallback);
    if (confirmedAt === null) {
      return err("invalid_response");
    }
    staleSince = confirmedAt;
  }
  return ok({ page: read.value.value, staleSince });
}

/** 把缓存层记录的确认时刻（浏览器墙钟毫秒）写成 RFC 3339 UTC，交给页面统一的时间格式显示。 */
function confirmationTime(millis: number): string | null {
  if (!Number.isSafeInteger(millis) || millis < 0) {
    return null;
  }
  if (millis > MAX_CONFIRMATION_MILLIS) {
    return null;
  }
  const date = new Date(millis);
  const time = date.getTime();
  if (!Number.isSafeInteger(time)) {
    return null;
  }
  // 上面的毫秒上限已经覆盖 jiff 的 Timestamp 范围；这里保留年份检查以防宿主 Date 行为差异。
  const year = date.getUTCFullYear();
  if (year < -9999 || year > 9999) {
    return null;
  }
  // Date 与 jiff 都覆盖当前浏览器可产生的时刻；秒精度输出与 to_rfc3339_opts(Secs, true) 一致。
  return `${date.toISOString().slice(0, 19)}Z`;
}
