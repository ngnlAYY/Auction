// 场次目录分页读取：目录 URL 构造、响应与请求身份绑定，以及内置摘要目录的读取页长。

import type {
  AuctionListResponse,
  AuctionSummaryResponse,
  AuctionSummaryV2Response,
} from "@auction/contracts/dto/public/catalog/auction";
import { MAX_PUBLIC_PAGE_SIZE } from "@auction/contracts/dto/public/pagination";
import type { Result } from "@auction/web-shared/result";
import { err, ok } from "@auction/web-shared/result";
import { decodeAuctionList, decodeAuctionListV2 } from "../decoder/catalog";
import { CATALOG_TTL_MILLIS } from "../swr-cache";
import { cachedPublicGet } from "./cached-get";

/** 场次摘要目录的内置读取页长（Public 页长上限）；与并行预取的第一页同 URL，命中同一缓存条目。 */
export const SUMMARY_PAGE_SIZE = MAX_PUBLIC_PAGE_SIZE;

/** v1 拍卖目录页的缓存投影标识。 */
const AUCTION_LIST_CACHE_TYPE = "public_auction_list_v1";
/** v2 拍卖目录页的缓存投影标识；与 v1 共用路径前缀但投影不同。 */
const AUCTION_HISTORY_LIST_CACHE_TYPE = "public_auction_list_v2";

/** 读取 v1 场次摘要目录的一页，并绑定响应与请求的页号、页长。 */
export async function fetchAuctionPage(
  worldId: string | null,
  status: string,
  page: number,
  pageSize: number,
  signal: AbortSignal,
): Promise<Result<AuctionListResponse, string>> {
  return fetchAuctionPageWithTtl(worldId, status, page, pageSize, CATALOG_TTL_MILLIS, signal);
}

/** 拍品已结算却不在目录时，仅跳过浏览器端的新鲜期复核第一页。 */
export function revalidateRecentSettled(
  signal: AbortSignal,
): Promise<Result<AuctionListResponse, string>> {
  return fetchAuctionPageWithTtl(null, "settled", 1, SUMMARY_PAGE_SIZE, 0, signal);
}

async function fetchAuctionPageWithTtl(
  worldId: string | null,
  status: string,
  page: number,
  pageSize: number,
  ttlMillis: number,
  signal: AbortSignal,
): Promise<Result<AuctionListResponse, string>> {
  const world = normalizedWorld(worldId);
  const url = auctionPageUrl("/api/v1/auctions", world, status, page, pageSize);
  const read = await cachedPublicGet(
    url,
    ttlMillis,
    AUCTION_LIST_CACHE_TYPE,
    (response) => {
      const decoded = decodeAuctionList(response, world, status);
      return decoded.ok ? bindAuctionPage(decoded.value, page, pageSize) : decoded;
    },
    signal,
  );
  return signal.aborted ? err("network_error") : read;
}

/** 历史页用 v2 一次取得本页摘要与论坛地址，不按行加载完整论坛镜像。 */
export async function fetchAuctionHistoryPage(
  worldId: string | null,
  status: string,
  page: number,
  pageSize: number,
  signal: AbortSignal,
): Promise<Result<AuctionListResponse<AuctionSummaryV2Response>, string>> {
  const world = normalizedWorld(worldId);
  const url = auctionPageUrl("/api/v2/auctions", world, status, page, pageSize);
  const read = await cachedPublicGet(
    url,
    CATALOG_TTL_MILLIS,
    AUCTION_HISTORY_LIST_CACHE_TYPE,
    (response) => {
      const decoded = decodeAuctionListV2(response, world, status);
      return decoded.ok ? bindAuctionPage(decoded.value, page, pageSize) : decoded;
    },
    signal,
  );
  return signal.aborted ? err("network_error") : read;
}

/** 页面和跨页查找共用请求身份校验；先完成严格解码，不能把错页响应拼进目录。 */
function bindAuctionPage<T>(
  response: AuctionListResponse<T>,
  page: number,
  pageSize: number,
): Result<AuctionListResponse<T>, string> {
  if (response.page !== page || response.page_size !== pageSize) {
    return err("invalid_response");
  }
  return ok(response);
}

function auctionPageUrl(
  endpoint: string,
  world: string | null,
  status: string,
  page: number,
  pageSize: number,
): string {
  const query = new URLSearchParams();
  query.append("page", String(page));
  query.append("page_size", String(pageSize));
  query.append("status", status);
  if (world !== null) {
    query.append("world", world);
  }
  return `${endpoint}?${query.toString()}`;
}

/** 预览切换只消费同一世界的近期结算场次，避免被其他世界的场次挤出第一页目录。 */
export async function fetchAuctionList(
  worldId: string,
  signal: AbortSignal,
): Promise<Result<AuctionSummaryResponse[], string>> {
  const page = await fetchAuctionPage(worldId, "settled", 1, SUMMARY_PAGE_SIZE, signal);
  if (signal.aborted) {
    return err("network_error");
  }
  return page.ok ? ok(page.value.auctions) : page;
}

function normalizedWorld(worldId: string | null): string | null {
  if (worldId === null || worldId === "" || worldId === "all") {
    return null;
  }
  return worldId;
}
