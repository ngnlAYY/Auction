// 世界列表、当前场次与按场次 ID 打开的历史场次读取。

import type { WorldsResponse } from "@auction/contracts/dto/public/account";
import type {
  CurrentAuctionItemResponse,
  SettledAuctionSummaryResponse,
} from "@auction/contracts/dto/public/catalog/auction";
import { transportErrorCode } from "@auction/web-shared/net/transport";
import { err, ok, type Result } from "@auction/web-shared/result";
import { compareText } from "@auction/web-shared/text";
import { validUuid } from "@auction/web-shared/ui/validation";
import { sendGet } from "../clock";
import { decodeErrorCode } from "../decoder";
import {
  decodeCurrentAuction,
  decodeSettledAuctionSummary,
  decodeWorlds,
} from "../decoder/catalog";
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

/** 已知场次所在世界后的再次读取：未结算的场次只读该世界的当前场次（一次带缓存的 GET），
 * 不再重读 lots 页与全部世界；当前场次已换成别的场次或读取失败时回退到完整读取。 */
export async function refetchKnownAuction(
  known: CurrentAuctionItemResponse,
  signal: AbortSignal,
): Promise<Result<CurrentAuctionItemResponse, string>> {
  if (known.status !== "settled") {
    const current = await fetchCurrentAuction(known.world, signal);
    if (
      current.ok &&
      current.value !== null &&
      isTargetCurrent(current.value, known.id, known.status)
    ) {
      return ok(current.value);
    }
  }
  return fetchHistoricalAuction(known.id, signal);
}

/** 读取指定历史拍卖的公开投影；世界、场次号和有效结束时间由目录或已结算摘要提供。 */
export async function fetchHistoricalAuction(
  auctionId: string,
  signal: AbortSignal,
): Promise<Result<CurrentAuctionItemResponse, string>> {
  if (!validUuid(auctionId)) {
    return err("invalid_auction_id");
  }
  // 结算目录沿用可复用的第一页缓存键，与 lots 并发读取，使目录命中时无需再等待一轮往返。
  const settledFirstPage = fetchAuctionPage(null, "settled", 1, SUMMARY_PAGE_SIZE, signal);
  const envelopeResult = await fetchCurrentLots(
    auctionId,
    null,
    null,
    catalogLevelFilterDefault(),
    1,
    20,
    signal,
  );
  if (signal.aborted) {
    return err("network_error");
  }
  if (!envelopeResult.ok) {
    return envelopeResult;
  }
  const envelope = envelopeResult.value;
  const status = envelope.auction_status;
  // 未结算的场次（含仍可出价的 live/final）是所在世界唯一的活动场次，从当前场次接口取得完整
  // 上下文，其中的冻结代理默认模式决定代理新建表单的初始选项；已结算场次不再竞价，没有该值。
  if (status !== "settled") {
    return fetchMatchingCurrentAuction(envelope.auction_id, status, signal);
  }
  const firstPage = await settledFirstPage;
  if (signal.aborted) {
    return err("network_error");
  }
  if (!firstPage.ok) {
    return firstPage;
  }
  // lots 与目录各有缓存新鲜期，目录缺席不能证明已结算的场次不存在。
  const entry = firstPage.value.auctions.find((auction) => auction.id === envelope.auction_id);
  const summaryResult = entry
    ? ok({
        world: entry.world_id,
        sequence: entry.sequence,
        effective_end_at: entry.effective_end_at,
      })
    : await fetchSettledAuctionSummary(envelope.auction_id, signal);
  if (signal.aborted) {
    return err("network_error");
  }
  if (!summaryResult.ok) {
    return summaryResult;
  }
  const summary = summaryResult.value;
  return ok({
    id: envelope.auction_id,
    world: summary.world,
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

async function fetchSettledAuctionSummary(
  auctionId: string,
  signal: AbortSignal,
): Promise<Result<SettledAuctionSummaryResponse, string>> {
  // 成功摘要由 HTTP 缓存复用。仅明确的 404 表示不存在，失败时交还错误供调用方重试。
  let missing = false;
  const sent = await sendGet(`/api/v1/auctions/${auctionId}/summary`, signal, (status) => {
    missing = status === 404;
  });
  if (signal.aborted) {
    return err("network_error");
  }
  if (missing) {
    return err("拍卖目录中不存在目标拍卖");
  }
  if (!sent.ok) {
    return err(transportErrorCode(sent.error));
  }
  const response = sent.value;
  if (response.status !== 200) {
    const code = decodeErrorCode(response);
    return err(code === "not_found" ? "invalid_response" : code);
  }
  return decodeSettledAuctionSummary(response, auctionId);
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
  const result = await new Promise<Result<CurrentAuctionItemResponse, string>>(
    (resolve, reject) => {
      const reads = worldIds.map(async (worldId) => {
        const current = await fetchCurrentAuction(worldId, signal);
        if (
          current.ok &&
          current.value !== null &&
          isTargetCurrent(current.value, auctionId, expectedStatus)
        ) {
          resolve(ok(current.value));
        }
        return current;
      });
      void Promise.all(reads).then(
        (currents) =>
          resolve(currents.find((current) => !current.ok) ?? err("当前拍卖目录中不存在目标拍卖")),
        reject,
      );
    },
  );
  return signal.aborted ? err("network_error") : result;
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
