// 公开竞价日志的编号页、稳定排序和来源字段组合校验。

import { type JsonObject, jsonKeysOnly } from "@auction/web-shared/net/wire";
import { compareText } from "@auction/web-shared/text";
import { validUuid } from "@auction/web-shared/ui/validation";

import { parseStrictRfc3339 } from "../../session/rfc3339";
import { boolValue, forumPostUrl, int, nonempty, object, positive, string } from "..";
import { validNumberedPage } from "./auction";

/** 校验竞价日志编号页、逐条闭集和稳定降序事实。 */
export function validBidLogPage(value: unknown): boolean {
  const page = object(value);
  if (page === null) return false;
  const bids = page.bids;
  if (!jsonKeysOnly(page, ["bids", "page", "page_size", "total"], [])) return false;
  return validNumberedPage(page) && Array.isArray(bids) && validOrderedBidLog(bids);
}

interface ValidatedBidLogEntry {
  bid: JsonObject;
  createdAt: bigint;
}

function validOrderedBidLog(values: readonly unknown[]): boolean {
  let previous: ValidatedBidLogEntry | null = null;
  for (const value of values) {
    const current = validatedBidLogEntry(value);
    if (current === null) return false;
    if (
      previous !== null &&
      (previous.createdAt < current.createdAt ||
        (previous.createdAt === current.createdAt && !orderedBidLogTie(previous.bid, current.bid)))
    ) {
      return false;
    }
    previous = current;
  }
  return true;
}

function orderedBidLogTie(previous: JsonObject, current: JsonObject): boolean {
  const lotOrder = compareText(string(previous.lot_id) ?? "", string(current.lot_id) ?? "");
  if (lotOrder !== 0) return lotOrder > 0;
  for (const key of ["accepted_state_version", "fact_index"]) {
    const previousField = int(previous[key]) ?? 0;
    const currentField = int(current[key]) ?? 0;
    if (previousField !== currentField) return previousField > currentField;
  }
  return compareText(string(previous.id) ?? "", string(current.id) ?? "") > 0;
}

/** 仅供测试使用：直接校验单条竞价事实及其落败代理附加字段。 */
export function validBidLogEntry(value: unknown): boolean {
  return validatedBidLogEntry(value) !== null;
}

function validatedBidLogEntry(value: unknown): ValidatedBidLogEntry | null {
  const bid = object(value);
  if (bid === null) return null;
  const valid =
    jsonKeysOnly(
      bid,
      [
        "id",
        "auction_id",
        "lot_id",
        "lot_number",
        "amount",
        "equipment_name",
        "equipment_category",
        "bidder_name",
        "valid",
        "accepted_state_version",
        "fact_index",
        "mechanism",
        "created_at",
      ],
      ["source", "source_post_number", "source_post_url", "defeated_proxy"],
    ) &&
    validUuid(string(bid.id) ?? "") &&
    validUuid(string(bid.auction_id) ?? "") &&
    validUuid(string(bid.lot_id) ?? "") &&
    positive(bid.lot_number) &&
    positive(bid.amount) &&
    nonempty(bid.equipment_name) &&
    nonempty(bid.equipment_category) &&
    nonempty(bid.bidder_name) &&
    boolValue(bid.valid) !== null &&
    positive(bid.accepted_state_version) &&
    positive(bid.fact_index) &&
    (string(bid.mechanism) === "ordinary" || string(bid.mechanism) === "proxy") &&
    validBidLogSource(bid) &&
    validBidLogDefeatedProxy(bid);
  if (!valid) return null;
  const createdAt = parseStrictRfc3339(string(bid.created_at) ?? "");
  return createdAt === null ? null : { bid, createdAt };
}

/** 落败代理只随代理竞争的那一条胜者代理事实下发，且不会是胜者本人。 */
function validBidLogDefeatedProxy(value: JsonObject): boolean {
  if (!Object.hasOwn(value, "defeated_proxy")) return true;
  const defeated = object(value.defeated_proxy);
  if (defeated === null) return false;
  return (
    jsonKeysOnly(defeated, ["bidder_name", "maximum_amount"], []) &&
    nonempty(defeated.bidder_name) &&
    positive(defeated.maximum_amount) &&
    string(value.mechanism) === "proxy" &&
    int(value.fact_index) === 1 &&
    string(defeated.bidder_name) !== string(value.bidder_name)
  );
}

function validBidLogSource(value: JsonObject): boolean {
  const mechanism = string(value.mechanism);
  const source = string(value.source);
  const postNumber = int(value.source_post_number);
  const sourceUrl = string(value.source_post_url);
  if (mechanism === "proxy" && source === "proxy" && postNumber === null && sourceUrl === null) {
    return true;
  }
  if (mechanism === "ordinary" && source === "forum") {
    if (postNumber !== null && postNumber >= 3) {
      return sourceUrl === null || forumPostUrl(sourceUrl);
    }
    return postNumber === null && sourceUrl === null;
  }
  // 兼容尚未增加链接字段的同版本旧响应；新服务会交付可点击的真实帖子 URL。
  return mechanism === "ordinary" && source === null && postNumber === null && sourceUrl === null;
}
