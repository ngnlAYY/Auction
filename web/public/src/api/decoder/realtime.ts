// 实时路由（`/api/v1/events` catch-up 页与 WebSocket 单事件帧）的 data 校验。
// 两条入口共用同一套事件防腐层；HTTP catch-up 只冻结 auction ID，单事件帧另外核对 world。

import {
  type PublicRealtimeEventResponse,
  PublicRealtimeEventResponseSchema,
  type PublicRealtimeResponse,
  PublicRealtimeResponseSchema,
} from "@auction/contracts/dto/response";
import { decode } from "@auction/contracts/wire";
import type { TransportResponse } from "@auction/web-shared/net/transport";
import { type JsonObject, jsonKeysOnly, parseJson } from "@auction/web-shared/net/wire";
import { err, ok, type Result } from "@auction/web-shared/result";
import { validUuid } from "@auction/web-shared/ui/validation";

import {
  boolValue,
  decodeSuccess,
  int,
  nonnegative,
  object,
  positive,
  revision,
  string,
  time,
  world,
} from "./index";

/** 解码旧版单拍卖 catch-up 页；共享 envelope 允许扩展，但 data/event/payload 必须精确同形。 */
export function decodeAuctionRealtimePage(
  response: TransportResponse,
  auctionId: string,
): Result<PublicRealtimeResponse, string> {
  return decodeSuccess(response, PublicRealtimeResponseSchema, (value) =>
    validAuctionRealtimePage(value, auctionId),
  );
}

/** 解码 WebSocket 的单事件文本帧，并复用 catch-up 页的严格事件防腐层。 */
export function decodeRealtimeEvent(
  message: string,
  auctionId: string,
  worldId: string,
): Result<PublicRealtimeEventResponse, string> {
  const parsed = parseJson(message);
  if (!parsed.ok) {
    return err("invalid_response");
  }
  const value = parsed.value;
  if (!validRealtimeEvent(value, auctionId) || !validRealtimeEventWorld(value, worldId)) {
    return err("invalid_response");
  }
  // JS 的 1.0 与 1 是同一个数；旧整数规范化在浏览器实现里是空操作。
  const decoded = decode(PublicRealtimeEventResponseSchema, value);
  return decoded.ok ? ok(decoded.value) : err("invalid_response");
}

function validAuctionRealtimePage(value: unknown, auctionId: string): boolean {
  const page = object(value);
  if (page === null) {
    return false;
  }
  return (
    jsonKeysOnly(
      page,
      ["auction_id", "events", "next_after", "has_more", "refetch_required"],
      [],
    ) &&
    string(page.auction_id) === auctionId &&
    validUuid(auctionId) &&
    validRealtimeEvents(page.events, auctionId) &&
    validRealtimePageMetadata(page)
  );
}

function validRealtimePageMetadata(value: JsonObject): boolean {
  return (
    nonnegative(value.next_after) &&
    boolValue(value.has_more) !== null &&
    boolValue(value.refetch_required) !== null
  );
}

function validRealtimeEvents(value: unknown, auctionId: string): boolean {
  return (
    Array.isArray(value) && value.every((event: unknown) => validRealtimeEvent(event, auctionId))
  );
}

function validRealtimeEvent(value: unknown, auctionId: string): boolean {
  const event = object(value);
  if (event === null) {
    return false;
  }
  if (
    !jsonKeysOnly(
      event,
      ["event_id", "stream_sequence", "kind", "schema_version", "payload", "created_at"],
      [],
    ) ||
    !validUuid(string(event.event_id) ?? "") ||
    !positive(event.stream_sequence) ||
    !time(event.created_at)
  ) {
    return false;
  }
  const payload = object(event.payload);
  if (payload === null) {
    return false;
  }
  const kind = string(event.kind);
  const schemaVersion = int(event.schema_version);
  if (string(payload.auction_id) !== auctionId) {
    return false;
  }

  switch (kind) {
    case "auction.created":
      return (
        schemaVersion === 1 &&
        jsonKeysOnly(payload, ["auction_id", "world_id", "sequence", "state_version"], []) &&
        validUuid(string(payload.auction_id) ?? "") &&
        world(payload.world_id) &&
        positive(payload.sequence) &&
        positive(payload.state_version)
      );
    case "auction.preview_updated":
      return (
        schemaVersion === 1 &&
        jsonKeysOnly(
          payload,
          ["auction_id", "preview_revision", "item_count", "manual_open", "state_version"],
          [],
        ) &&
        validUuid(string(payload.auction_id) ?? "") &&
        revision(payload.preview_revision) &&
        nonnegative(payload.item_count) &&
        boolValue(payload.manual_open) !== null &&
        positive(payload.state_version)
      );
    case "preview_catalog_changed":
      return (
        schemaVersion === 1 &&
        jsonKeysOnly(payload, ["world", "auction_id", "preview_revision"], []) &&
        world(payload.world) &&
        validUuid(string(payload.auction_id) ?? "") &&
        positive(payload.preview_revision)
      );
    case "auction.opened":
      return (
        schemaVersion === 1 &&
        jsonKeysOnly(
          payload,
          ["auction_id", "world_id", "state_version", "preview_revision"],
          [],
        ) &&
        validUuid(string(payload.auction_id) ?? "") &&
        world(payload.world_id) &&
        positive(payload.state_version) &&
        positive(payload.preview_revision)
      );
    case "auction.schedule_updated":
      return (
        schemaVersion === 1 &&
        jsonKeysOnly(
          payload,
          ["auction_id", "world_id", "scheduled_end_at", "state_version"],
          [],
        ) &&
        validUuid(string(payload.auction_id) ?? "") &&
        world(payload.world_id) &&
        time(payload.scheduled_end_at) &&
        positive(payload.state_version)
      );
    case "auction.final_phase_entered":
    case "auction.early_settlement_requested":
    case "auction.cancelled":
    case "auction.settling":
      return (
        schemaVersion === 1 &&
        jsonKeysOnly(payload, ["auction_id", "state_version"], []) &&
        validUuid(string(payload.auction_id) ?? "") &&
        positive(payload.state_version)
      );
    case "auction.settled": {
      if (schemaVersion === 1) {
        return (
          jsonKeysOnly(
            payload,
            ["auction_id", "sold_count", "unsold_count", "state_version"],
            [],
          ) &&
          validUuid(string(payload.auction_id) ?? "") &&
          nonnegative(payload.sold_count) &&
          nonnegative(payload.unsold_count) &&
          positive(payload.state_version)
        );
      }
      if (schemaVersion !== 2) {
        return false;
      }
      const sold = int(payload.sold_count);
      const unsold = int(payload.unsold_count);
      return (
        jsonKeysOnly(
          payload,
          [
            "auction_id",
            "state_version",
            "policy_version",
            "sold_count",
            "unsold_count",
            "obligation_count",
          ],
          [],
        ) &&
        validUuid(string(payload.auction_id) ?? "") &&
        positive(payload.state_version) &&
        nonnegative(payload.policy_version) &&
        nonnegative(payload.sold_count) &&
        nonnegative(payload.unsold_count) &&
        sold !== null &&
        unsold !== null &&
        sold + unsold > 0 &&
        positive(payload.obligation_count)
      );
    }
    case "lot.bid_accepted": {
      if (schemaVersion !== 2) {
        return false;
      }
      const mechanism = string(payload.mechanism);
      return (
        jsonKeysOnly(
          payload,
          [
            "auction_id",
            "lot_id",
            "bid_id",
            "amount",
            "effective_end",
            "state_version",
            "fact_index",
            "mechanism",
          ],
          [],
        ) &&
        validUuid(string(payload.auction_id) ?? "") &&
        validUuid(string(payload.lot_id) ?? "") &&
        validUuid(string(payload.bid_id) ?? "") &&
        positive(payload.amount) &&
        time(payload.effective_end) &&
        positive(payload.state_version) &&
        positive(payload.fact_index) &&
        (mechanism === "ordinary" || mechanism === "proxy")
      );
    }
    default:
      return false;
  }
}

/** 旧版 auction-specific WebSocket 还会把带 world 的事件与当前页面目标核对；HTTP catch-up
 * 只冻结 auction ID，因此该校验只由单事件帧入口调用。 */
function validRealtimeEventWorld(value: unknown, expectedWorld: string): boolean {
  const event = object(value);
  if (event === null) {
    return false;
  }
  const payload = object(event.payload);
  if (payload === null) {
    return false;
  }
  switch (string(event.kind)) {
    case "preview_catalog_changed":
      return string(payload.world) === expectedWorld;
    case "auction.created":
    case "auction.opened":
    case "auction.schedule_updated":
      return string(payload.world_id) === expectedWorld;
    default:
      return true;
  }
}
