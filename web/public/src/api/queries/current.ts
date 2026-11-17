// 世界列表、当前场次与按场次 ID 打开的历史场次读取。

import type { WorldsResponse } from "@auction/contracts/dto/public/account";
import type {
  AuctionListResponse,
  AuctionSummaryResponse,
  CurrentAuctionItemResponse,
} from "@auction/contracts/dto/public/catalog/auction";
import { transportErrorCode } from "@auction/web-shared/net/transport";
import { err, ok, type Result } from "@auction/web-shared/result";
import { compareText } from "@auction/web-shared/text";
import { sendGet } from "../clock";
import { decodeErrorCode } from "../decoder";
import { decodeCurrentAuction, decodeWorlds } from "../decoder/catalog";
import { catalogLevelFilterDefault } from "../model";
import { LIVE_TTL_MILLIS } from "../swr-cache";
import { cachedPublicGet } from "./cached-get";
import { fetchAuctionPage, SUMMARY_PAGE_SIZE } from "./directory";
import { fetchCurrentLots } from "./lots";

/** 当前场次投影的缓存类型标识；同一 URL 不得被其他查询类型复用。 */
const CURRENT_AUCTION_CACHE_TYPE = "public_current_auction";

/** 读取公开世界列表；不做缓存，调用方每次都要服务端当前快照。 */
export async function fetchWorlds(signal: AbortSignal): Promise<Result<WorldsResponse, string>> {
  const sent = await sendGet("/api/v1/worlds", signal);
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
  return decodeWorlds(response);
}

/** 读取指定世界的当前场次；没有进行中场次时返回 `null`。 */
export async function fetchCurrentAuction(
  worldId: string,
  signal: AbortSignal,
): Promise<Result<CurrentAuctionItemResponse | null, string>> {
  const url = currentAuctionUrl(worldId);
  return cachedPublicGet(
    url,
    LIVE_TTL_MILLIS,
    CURRENT_AUCTION_CACHE_TYPE,
    (response) => {
      const decoded = decodeCurrentAuction(response, worldId);
      return decoded.ok ? ok(decoded.value.auction) : decoded;
    },
    signal,
  );
}

/** 当前场次的唯一 URL 构造点：始终用 include=proxy_default_mode 显式请求冻结策略的代理
 * 默认模式，解码器据此要求该键。服务端只对显式请求追加该键，旧页面的缺省请求仍得到原字段集合。 */
/** 仅供测试使用：核对当前场次读取的唯一 URL 构造点。 */
export function currentAuctionUrl(worldId: string): string {
  const query = new URLSearchParams();
  query.append("world", worldId);
  query.append("include", "proxy_default_mode");
  return `/api/v1/auctions/current?${query.toString()}`;
}

/** 读取指定历史拍卖的公开投影。
 * 旧 v1 lot wire 不包含世界、场次号和计划结束时间；这些展示上下文必须从拍卖目录读取，不能
 * 通过扩张 lot 响应破坏旧客户端的严格字段闭集。 */
export async function fetchHistoricalAuction(
  auctionId: string,
  signal: AbortSignal,
): Promise<Result<CurrentAuctionItemResponse, string>> {
  // 目录与看板默认目录共用同一 URL（page_size=20），命中 SWR 缓存，避免点击详情时额外触发
  // 一次 10 条目录请求并在返回时闪现加载态。按场次 ID 打开的页面绝大多数是已结算场次：已结算
  // 目录第一页与 lot 目录同时发出，省去“等状态再查摘要”的一轮串行往返；该 URL 是公开低频缓存
  // 档，状态不是 settled 时丢弃预取结果、按实际状态查找。
  const [envelopeResult, settledFirstPage] = await Promise.all([
    fetchCurrentLots(auctionId, null, null, catalogLevelFilterDefault(), 1, 20, signal),
    fetchAuctionPage(null, "settled", 1, SUMMARY_PAGE_SIZE, signal),
  ]);
  if (signal.aborted) {
    return err("network_error");
  }
  if (!envelopeResult.ok) {
    return envelopeResult;
  }
  const envelope = envelopeResult.value;
  const status = envelope.auction_status;
  // 未结算的场次（含仍可出价的 live/final）是所在世界唯一的活动场次，从当前场次接口取得完整
  // 上下文，其中的冻结代理默认模式决定代理新建表单的初始选项；已结算场次不再竞价，只从往期
  // 目录拼装，没有该值。
  if (status !== "settled") {
    return fetchMatchingCurrentAuction(envelope.auction_id, status, signal);
  }
  const summaryResult = await fetchAuctionSummary(
    envelope.auction_id,
    status,
    settledFirstPage,
    signal,
  );
  if (signal.aborted) {
    return err("network_error");
  }
  if (!summaryResult.ok) {
    return summaryResult;
  }
  const summary = summaryResult.value;
  return ok({
    id: envelope.auction_id,
    world: summary.world_id,
    number: summary.sequence,
    status,
    opens_at: envelope.opens_at,
    scheduled_ends_at: summary.effective_end_at,
    presentation_mode: envelope.presentation_mode,
    bidding_open: envelope.bidding_open,
    preview_revision: envelope.preview_revision,
    proxy_default_mode: null,
  });
}

/** 在指定状态的场次目录中查找摘要；firstPage 是调用方已并行取得的第一页。 */
async function fetchAuctionSummary(
  auctionId: string,
  status: string,
  firstPage: Result<AuctionListResponse, string>,
  signal: AbortSignal,
): Promise<Result<AuctionSummaryResponse, string>> {
  if (signal.aborted) {
    return err("network_error");
  }
  let pageNumber = 1;
  let first: Result<AuctionListResponse, string> | null = firstPage;
  for (;;) {
    let page: AuctionListResponse;
    if (first !== null) {
      const firstResult = first;
      first = null;
      if (!firstResult.ok) {
        return firstResult;
      }
      page = firstResult.value;
    } else {
      if (signal.aborted) {
        return err("network_error");
      }
      const fetched = await fetchAuctionPage(null, status, pageNumber, SUMMARY_PAGE_SIZE, signal);
      if (signal.aborted) {
        return err("network_error");
      }
      if (!fetched.ok) {
        return fetched;
      }
      page = fetched.value;
    }
    const rowCount = page.auctions.length;
    // Rust 的数组长度转换不可能失败；保留该分支的失败关闭语义，拒绝异常的宿主结果。
    if (!Number.isSafeInteger(rowCount)) {
      return err("拍卖目录页过大");
    }
    const total = page.total;
    const auction = page.auctions.find((candidate) => candidate.id === auctionId);
    if (auction !== undefined) {
      return ok(auction);
    }
    if (
      rowCount < SUMMARY_PAGE_SIZE ||
      (total !== null && saturatingMultiply(pageNumber, SUMMARY_PAGE_SIZE) >= total)
    ) {
      return err("拍卖目录中不存在目标拍卖");
    }
    const nextPage = pageNumber + 1;
    if (!Number.isSafeInteger(nextPage)) {
      return err("拍卖目录页码溢出");
    }
    pageNumber = nextPage;
  }
}

async function fetchMatchingCurrentAuction(
  auctionId: string,
  expectedStatus: string,
  signal: AbortSignal,
): Promise<Result<CurrentAuctionItemResponse, string>> {
  const worlds = await fetchWorlds(signal);
  if (signal.aborted) {
    return err("network_error");
  }
  if (!worlds.ok) {
    return worlds;
  }
  // 各世界的当前场次互不依赖，并行读取：任一世界命中即返回；都未命中时先报读取失败，不把读不到
  // 的世界当作“不存在目标拍卖”。世界 ID 排序保持旧 BTreeMap 的遍历顺序，便于稳定复现。
  const worldIds = Object.keys(worlds.value.worlds).sort(compareText);
  const currents = await Promise.all(
    worldIds.map((worldId) => fetchCurrentAuction(worldId, signal)),
  );
  if (signal.aborted) {
    return err("network_error");
  }
  let firstError: string | null = null;
  for (const current of currents) {
    if (current.ok) {
      const auction = current.value;
      if (auction !== null && isTargetCurrent(auction, auctionId, expectedStatus)) {
        return ok(auction);
      }
    } else {
      firstError ??= current.error;
    }
  }
  return err(firstError ?? "当前拍卖目录中不存在目标拍卖");
}

/** 当前场次条目是否就是按 ID 打开的目标场次。lot 外壳与当前场次各有缓存 TTL，live→final
 * 切换前后两者可能短暂各持一边；同属可出价阶段时以当前场次条目为准，其余状态必须一致。 */
/** 仅供测试使用：核对按场次 ID 打开时容忍的 live/final 切换。 */
export function isTargetCurrent(
  auction: CurrentAuctionItemResponse,
  auctionId: string,
  expectedStatus: string,
): boolean {
  return (
    auction.id === auctionId &&
    (auction.status === expectedStatus ||
      ((auction.status === "live" || auction.status === "final") &&
        (expectedStatus === "live" || expectedStatus === "final")))
  );
}

function responseOk(response: { readonly status: number }): boolean {
  return response.status >= 200 && response.status < 300;
}

function saturatingMultiply(left: number, right: number): number {
  const value = left * right;
  return Number.isSafeInteger(value) ? value : Number.MAX_SAFE_INTEGER;
}
