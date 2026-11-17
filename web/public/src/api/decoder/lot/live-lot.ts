// live lot 投影的字段闭集、价格与领先者一致性、状态时间与完整装备校验。

import { jsonKeysOnly } from "@auction/web-shared/net/wire";
import { validUuid } from "@auction/web-shared/ui/validation";

import { boolValue, int, nonempty, object, positive, string, time } from "..";
import { validEquipment } from "./preview-equipment";

/** 校验 live lot 的字段闭集、价格关系、领先者状态、时间与完整装备。 */
export function validLiveLot(value: unknown, auctionId: string, lotId: string | null): boolean {
  const decoded = object(value);
  if (decoded === null) return false;
  const hasLeader = boolValue(decoded.has_leader);
  const leaderPresent = Object.hasOwn(decoded, "leader_name");
  const startPrice = int(decoded.start_price);
  const currentPrice = int(decoded.current_price);
  const nextBid = int(decoded.next_bid);
  const leaderMatches =
    hasLeader === true
      ? nextBid !== null && currentPrice !== null && nextBid > currentPrice
      : currentPrice === startPrice && nextBid === startPrice;
  const status = string(decoded.status);
  const statusMatches =
    status === "unsold"
      ? !Object.hasOwn(decoded, "effective_end_at") || time(decoded.effective_end_at)
      : (status === "live" || status === "sold") && time(decoded.effective_end_at);
  return (
    jsonKeysOnly(
      decoded,
      [
        "id",
        "auction_id",
        "lot_number",
        "status",
        "start_price",
        "current_price",
        "next_bid",
        "has_leader",
        "seller_name",
        "state_version",
        "equipment",
      ],
      ["effective_end_at", "leader_name"],
    ) &&
    validUuid(string(decoded.id) ?? "") &&
    (lotId === null || string(decoded.id) === lotId) &&
    string(decoded.auction_id) === auctionId &&
    positive(decoded.lot_number) &&
    (status === "live" || status === "sold" || status === "unsold") &&
    positive(decoded.start_price) &&
    positive(decoded.current_price) &&
    currentPrice !== null &&
    startPrice !== null &&
    currentPrice >= startPrice &&
    positive(decoded.next_bid) &&
    hasLeader !== null &&
    leaderMatches &&
    nonempty(decoded.seller_name) &&
    (!leaderPresent || nonempty(decoded.leader_name)) &&
    hasLeader === leaderPresent &&
    statusMatches &&
    positive(decoded.state_version) &&
    validEquipment(decoded.equipment, true)
  );
}
