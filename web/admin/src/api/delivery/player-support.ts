// 玩家支持查询的响应解码与参数编码。

import type { AdminPlayerSupportBundleResponse } from "@auction/contracts/dto/admin/delivery";
import { err, ok, type Result } from "@auction/web-shared/result";
import { validUuid } from "@auction/web-shared/ui/validation";

import { MAX_JSON_SAFE_INTEGER, validSafeText } from "../validation";
import type { StrictDeliveryEnvelope } from "./decode";
import { decodeDelivery, parseDeliveryTime } from "./decode";
import { DELIVERY_PROJECTION_INCLUDE, MAX_PLAYER_SUPPORT_ROWS } from "./index";

/** 只放行页面实际展示的闭集与有界数值；任何未知种类或越界 ID 都让整页失败关闭。 */
export function decodePlayerSupport(
  payload: StrictDeliveryEnvelope<AdminPlayerSupportBundleResponse>,
): Result<AdminPlayerSupportBundleResponse, null> {
  if (payload.code !== "ok" || payload.message.length === 0) {
    return err(null);
  }
  const bundle = payload.data;
  if (
    bundle.bids.length > MAX_PLAYER_SUPPORT_ROWS ||
    bundle.deals.length > MAX_PLAYER_SUPPORT_ROWS ||
    bundle.obligations.length > MAX_PLAYER_SUPPORT_ROWS ||
    bundle.deliveries.length > MAX_PLAYER_SUPPORT_ROWS ||
    bundle.notifications.length > MAX_PLAYER_SUPPORT_ROWS
  ) {
    return err(null);
  }
  const user = bundle.user;
  if (
    user !== null &&
    (!validUuid(user.id) ||
      user.username.length === 0 ||
      (user.status !== "active" && user.status !== "blacklisted") ||
      user.game_player_id === "")
  ) {
    return err(null);
  }
  for (const bid of bundle.bids) {
    if (
      !validUuid(bid.bid_id) ||
      !validUuid(bid.lot_id) ||
      !validUuid(bid.auction_id) ||
      bid.lot_number < 0 ||
      !validWorld(bid.world_id) ||
      bid.amount < 0 ||
      bid.amount > MAX_JSON_SAFE_INTEGER ||
      (bid.mechanism !== "ordinary" && bid.mechanism !== "proxy") ||
      (bid.lot_status !== "pending" &&
        bid.lot_status !== "live" &&
        bid.lot_status !== "sold" &&
        bid.lot_status !== "unsold" &&
        bid.lot_status !== "withdrawn") ||
      parseDeliveryTime(bid.created_at) === null
    ) {
      return err(null);
    }
  }
  for (const deal of bundle.deals) {
    if (
      !validUuid(deal.lot_id) ||
      !validUuid(deal.auction_id) ||
      deal.lot_number < 0 ||
      !validWorld(deal.world_id) ||
      deal.equipment_name.length === 0 ||
      deal.final_price < 0 ||
      deal.final_price > MAX_JSON_SAFE_INTEGER
    ) {
      return err(null);
    }
  }
  for (const obligation of bundle.obligations) {
    if (
      !validUuid(obligation.obligation_id) ||
      !validUuid(obligation.auction_id) ||
      !validUuid(obligation.lot_id) ||
      !validWorld(obligation.world_id) ||
      !validSafeText(obligation.obligation_kind, 64) ||
      (obligation.business_state !== "ready" &&
        obligation.business_state !== "blocked" &&
        obligation.business_state !== "contingent") ||
      obligation.amount < 0 ||
      obligation.amount > MAX_JSON_SAFE_INTEGER ||
      (obligation.delivery_id !== null && !validUuid(obligation.delivery_id))
    ) {
      return err(null);
    }
  }
  for (const delivery of bundle.deliveries) {
    if (!decodeDelivery(delivery).ok) {
      return err(null);
    }
  }
  for (const notification of bundle.notifications) {
    if (
      !validUuid(notification.notification_id) ||
      (notification.channel !== "mm" &&
        notification.channel !== "pm" &&
        notification.channel !== "pending") ||
      !validSafeText(notification.state, 64) ||
      (notification.lot_id !== null && !validUuid(notification.lot_id)) ||
      (notification.auction_id !== null && !validUuid(notification.auction_id)) ||
      (notification.delivery_id !== null && !validUuid(notification.delivery_id)) ||
      (notification.state_reason !== null && !validSafeText(notification.state_reason, 96)) ||
      parseDeliveryTime(notification.created_at) === null ||
      parseDeliveryTime(notification.updated_at) === null
    ) {
      return err(null);
    }
  }
  for (const reference of bundle.references) {
    if (
      (reference.kind !== "delivery" &&
        reference.kind !== "obligation" &&
        reference.kind !== "notification" &&
        reference.kind !== "lot" &&
        reference.kind !== "auction") ||
      !validUuid(reference.id)
    ) {
      return err(null);
    }
  }
  return ok(bundle);
}

export function validWorld(world: string): boolean {
  return world === "main" || world === "isekai";
}

function urlEncode(value: string): string {
  const params = new URLSearchParams();
  params.append("v", value);
  return params.toString().slice(2);
}

/** 玩家支持的唯一 URL 构造点：总是请求扩展投影，投递分区才包含结算退件 Take（本页面的闭集认识
 * 该来源）；缺省投影留给升级前的页面。 */
export function playerSupportUrl(key: string, value: string): string {
  return `/api/v1/admin/player-support?${key}=${urlEncode(value)}&include=${DELIVERY_PROJECTION_INCLUDE}`;
}
