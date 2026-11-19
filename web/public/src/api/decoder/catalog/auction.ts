// 场次、当前拍卖与拍卖目录页的字段闭集、响应绑定、状态组合和分页形状校验。

import {
  AuctionSummaryResponseSchema,
  AuctionSummaryV2ResponseSchema,
  auctionListResponseSchema,
} from "@auction/contracts/dto/public/catalog/auction";
import { type JsonObject, jsonKeysOnly } from "@auction/web-shared/net/wire";
import { validUuid } from "@auction/web-shared/ui/validation";

import { validForumUrl } from "../../forum";
import { parseStrictRfc3339 } from "../../session/rfc3339";
import {
  accountNonempty,
  boolValue,
  int,
  nonnegative,
  object,
  optionalTime,
  positive,
  positivePageSize,
  proxyMode,
  rawNonemptyStr,
  revision,
  string,
  time,
  world,
} from "..";

export const AUCTION_LIST_SCHEMA = auctionListResponseSchema(AuctionSummaryResponseSchema);
export const AUCTION_LIST_V2_SCHEMA = auctionListResponseSchema(AuctionSummaryV2ResponseSchema);

/** 校验世界目录的动态 key 与固定 value 字段闭集。 */
export function validWorlds(value: unknown): boolean {
  const worlds = object(value);
  if (worlds === null) return false;
  if (!jsonKeysOnly(worlds, ["worlds"], [])) return false;
  const entries = object(worlds.worlds);
  if (entries === null) return false;
  return Object.entries(entries).every(([name, entry]) => {
    const status = object(entry);
    return (
      status !== null &&
      rawNonemptyStr(name) &&
      jsonKeysOnly(status, ["enabled"], []) &&
      boolValue(status.enabled) !== null
    );
  });
}

/** 校验当前拍卖响应的外层闭集，并绑定请求的世界。 */
export function validCurrentAuctionResponse(value: unknown, worldId: string): boolean {
  const response = object(value);
  if (response === null) return false;
  const auction = response.auction;
  return (
    jsonKeysOnly(response, ["auction"], []) &&
    (auction === null || validCurrentAuction(auction, worldId))
  );
}

function validCurrentAuction(value: unknown, worldId: string): boolean {
  const auction = object(value);
  if (auction === null) return false;
  if (
    !jsonKeysOnly(
      auction,
      [
        "id",
        "world",
        "number",
        "status",
        "opens_at",
        "scheduled_ends_at",
        "presentation_mode",
        "bidding_open",
        "preview_revision",
        "proxy_default_mode",
      ],
      ["final_restriction"],
    ) ||
    !validUuid(string(auction.id) ?? "") ||
    string(auction.world) !== worldId ||
    !positive(auction.number) ||
    !time(auction.opens_at) ||
    !(auction.scheduled_ends_at === null || time(auction.scheduled_ends_at)) ||
    boolValue(auction.bidding_open) === null ||
    !revision(auction.preview_revision) ||
    // 冻结策略的代理默认模式决定代理新建表单的初始选项；缺失或闭集外的值整条拒绝。
    !proxyMode(auction.proxy_default_mode)
  ) {
    return false;
  }
  const presentationMode = string(auction.presentation_mode);
  const status = string(auction.status);
  const biddingOpen = boolValue(auction.bidding_open);
  if (presentationMode === "preview" || presentationMode === "opening") {
    return status === "preview" && biddingOpen === false && auction.scheduled_ends_at === null;
  }
  if (presentationMode === "live") {
    return (
      status === "live" || status === "final" || (status === "settling" && biddingOpen === false)
    );
  }
  return false;
}

/** 长缓存摘要只接受与请求 ID 相符的公开场次数据。 */
export function validSettledAuctionSummary(value: unknown, auctionId: string): boolean {
  const summary = object(value);
  if (summary === null) return false;
  const endsAt = string(summary.effective_end_at);
  return (
    jsonKeysOnly(summary, ["id", "world", "sequence", "effective_end_at"], []) &&
    string(summary.id) === auctionId &&
    validUuid(auctionId) &&
    world(summary.world) &&
    positive(summary.sequence) &&
    (summary.effective_end_at === null ||
      (endsAt?.endsWith("Z") === true && parseStrictRfc3339(endsAt) !== null))
  );
}

/** 校验拍卖目录编号页及逐项响应绑定。 */
export function validAuctionList(
  value: unknown,
  worldId: string | null,
  status: string,
  withForumUrl: boolean,
): boolean {
  const page = object(value);
  if (page === null) return false;
  const auctions = page.auctions;
  if (!jsonKeysOnly(page, ["auctions", "page", "page_size", "total"], [])) return false;
  if (!validNumberedPage(page) || !Array.isArray(auctions)) return false;
  return auctions.every((auction: unknown) =>
    validAuctionSummary(auction, worldId, status, withForumUrl),
  );
}

function validAuctionSummary(
  value: unknown,
  worldId: string | null,
  status: string,
  withForumUrl: boolean,
): boolean {
  const auction = object(value);
  if (auction === null) return false;
  const optionalKeys = ["opened_at", "final_phase_at", "effective_end_at", "settled_at"];
  if (withForumUrl) optionalKeys.push("forum_thread_url");
  if (
    !jsonKeysOnly(
      auction,
      [
        "id",
        "world_id",
        "sequence",
        "status",
        "scheduled_open_at",
        "lot_count",
        "sold_count",
        "unsold_count",
        "sold_amount",
        "state_version",
      ],
      optionalKeys,
    ) ||
    !validUuid(string(auction.id) ?? "") ||
    !accountNonempty(auction.world_id) ||
    string(auction.status) !== status ||
    (worldId !== null && string(auction.world_id) !== worldId) ||
    !positive(auction.sequence) ||
    !time(auction.scheduled_open_at) ||
    !optionalTime(auction, "opened_at") ||
    !optionalTime(auction, "final_phase_at") ||
    !optionalTime(auction, "effective_end_at") ||
    !optionalTime(auction, "settled_at") ||
    !nonnegative(auction.lot_count) ||
    !nonnegative(auction.sold_count) ||
    !nonnegative(auction.unsold_count) ||
    !nonnegative(auction.sold_amount) ||
    !positive(auction.state_version)
  ) {
    return false;
  }
  if (Object.hasOwn(auction, "forum_thread_url")) {
    const forumUrl = string(auction.forum_thread_url);
    if (forumUrl === null || !validForumUrl(forumUrl)) {
      return false;
    }
  }
  const sold = int(auction.sold_count);
  const unsold = int(auction.unsold_count);
  const lotCount = int(auction.lot_count);
  return sold !== null && unsold !== null && lotCount !== null && sold + unsold <= lotCount;
}

/** 校验编号页的三个基础字段。 */
export function validNumberedPage(value: JsonObject): boolean {
  return positive(value.page) && positivePageSize(value.page_size) && nonnegative(value.total);
}
