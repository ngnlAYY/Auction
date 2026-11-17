// 公开目录路由（世界、拍卖目录、竞价日志、价格历史、规则、黑名单、补偿池）的对外解码入口。

import {
  type PublicBlacklistResponse,
  PublicBlacklistResponseSchema,
  type PublicRulesResponse,
  PublicRulesResponseSchema,
  type WorldsResponse,
  WorldsResponseSchema,
} from "@auction/contracts/dto/public/account";
import {
  type AuctionListResponse,
  type AuctionSummaryV2Response,
  type CurrentAuctionResponse,
  CurrentAuctionResponseSchema,
} from "@auction/contracts/dto/public/catalog/auction";
import {
  type PublicCompensationLotPageResponse,
  PublicCompensationLotPageResponseSchema,
  type PublicCompensationResponse,
  PublicCompensationResponseSchema,
} from "@auction/contracts/dto/public/catalog/compensation";
import {
  type PublicBidLogListResponse,
  PublicBidLogListResponseSchema,
  type PublicPriceHistoryListResponse,
  PublicPriceHistoryListResponseSchema,
} from "@auction/contracts/dto/public/catalog/history";
import type { TransportResponse } from "@auction/web-shared/net/transport";
import type { Result } from "@auction/web-shared/result";

import { decodeSuccess } from "..";
import {
  AUCTION_LIST_SCHEMA,
  AUCTION_LIST_V2_SCHEMA,
  validAuctionList,
  validCurrentAuctionResponse,
  validWorlds,
} from "./auction";
import { validBidLogPage } from "./bid-log";
import { validCompensationLots, validCompensationPool } from "./compensation";
import { validPriceHistoryPage } from "./price-history";
import { validBlacklistPage, validRules } from "./rules-blacklist";

/** 解码世界目录；世界 ID 是动态 key，但每个 value 的字段闭集固定。 */
export function decodeWorlds(response: TransportResponse): Result<WorldsResponse, string> {
  return decodeSuccess(response, WorldsResponseSchema, validWorlds);
}

/** 解码指定世界的当前拍卖，并把响应中的 world 与请求目标绑定。请求总是带
 * `include=proxy_default_mode`，因此条目必须含冻结策略的代理默认模式。 */
export function decodeCurrentAuction(
  response: TransportResponse,
  worldId: string,
): Result<CurrentAuctionResponse, string> {
  return decodeSuccess(response, CurrentAuctionResponseSchema, (value) =>
    validCurrentAuctionResponse(value, worldId),
  );
}

/** 解码拍卖目录编号页，并恢复旧浏览器对 status/world 查询条件的响应绑定。 */
export function decodeAuctionList(
  response: TransportResponse,
  worldId: string | null,
  status: string,
): Result<AuctionListResponse, string> {
  return decodeSuccess(response, AUCTION_LIST_SCHEMA, (value) =>
    validAuctionList(value, worldId, status, false),
  );
}

/** v2 独立扩展摘要字段闭集；v1 仍拒绝论坛地址字段。 */
export function decodeAuctionListV2(
  response: TransportResponse,
  worldId: string | null,
  status: string,
): Result<AuctionListResponse<AuctionSummaryV2Response>, string> {
  return decodeSuccess(response, AUCTION_LIST_V2_SCHEMA, (value) =>
    validAuctionList(value, worldId, status, true),
  );
}

/** 解码按编号分页且顺序稳定的公开竞价事实。 */
export function decodeBidLogPage(
  response: TransportResponse,
): Result<PublicBidLogListResponse, string> {
  return decodeSuccess(response, PublicBidLogListResponseSchema, validBidLogPage);
}

/** 解码价格历史编号页；cursor 形状不能混入当前 WebUI 的编号页请求。 */
export function decodePriceHistoryPage(
  response: TransportResponse,
): Result<PublicPriceHistoryListResponse, string> {
  return decodeSuccess(response, PublicPriceHistoryListResponseSchema, validPriceHistoryPage);
}

/** 解码已发布规则文档。 */
export function decodeRules(response: TransportResponse): Result<PublicRulesResponse, string> {
  return decodeSuccess(response, PublicRulesResponseSchema, validRules);
}

/** 解码公开黑名单编号页。 */
export function decodeBlacklist(
  response: TransportResponse,
): Result<PublicBlacklistResponse, string> {
  return decodeSuccess(response, PublicBlacklistResponseSchema, validBlacklistPage);
}

/** 解码补偿池批次编号页。 */
export function decodeCompensationPool(
  response: TransportResponse,
): Result<PublicCompensationResponse, string> {
  return decodeSuccess(response, PublicCompensationResponseSchema, validCompensationPool);
}

/** 解码单场逐件结算明细编号页。 */
export function decodeCompensationLots(
  response: TransportResponse,
): Result<PublicCompensationLotPageResponse, string> {
  return decodeSuccess(response, PublicCompensationLotPageResponseSchema, validCompensationLots);
}
