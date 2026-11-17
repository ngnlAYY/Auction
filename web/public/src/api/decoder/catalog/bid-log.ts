// 公开竞价日志的编号页、稳定排序和来源字段组合校验。

import { type JsonObject, jsonKeysOnly } from "@auction/web-shared/net/wire";
import { compareText } from "@auction/web-shared/text";
import { validUuid } from "@auction/web-shared/ui/validation";

import { boolValue, forumPostUrl, int, nonempty, object, positive, string, time } from "..";
import { validNumberedPage } from "./auction";

/** 校验竞价日志编号页、逐条闭集和稳定降序事实。 */
export function validBidLogPage(value: unknown): boolean {
  const page = object(value);
  if (page === null) return false;
  const bids = page.bids;
  if (!jsonKeysOnly(page, ["bids", "page", "page_size", "total"], [])) return false;
  return validNumberedPage(page) && Array.isArray(bids) && validOrderedBidLog(bids);
}

function validOrderedBidLog(values: readonly unknown[]): boolean {
  if (!values.every(validBidLogEntry)) return false;
  return values.slice(1).every((currentValue, offset) => {
    const previous = object(values[offset]);
    const current = object(currentValue);
    if (previous === null || current === null) return false;
    for (const key of ["created_at", "lot_id"]) {
      const previousField = string(previous[key]) ?? "";
      const currentField = string(current[key]) ?? "";
      if (previousField !== currentField) {
        return compareText(previousField, currentField) > 0;
      }
    }
    for (const key of ["accepted_state_version", "fact_index"]) {
      const previousField = int(previous[key]) ?? 0;
      const currentField = int(current[key]) ?? 0;
      if (previousField !== currentField) {
        return previousField > currentField;
      }
    }
    return compareText(string(previous.id) ?? "", string(current.id) ?? "") > 0;
  });
}

/** 仅供测试使用：直接校验单条竞价事实及其落败代理附加字段。 */
export function validBidLogEntry(value: unknown): boolean {
  const bid = object(value);
  if (bid === null) return false;
  return (
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
    validBidLogDefeatedProxy(bid) &&
    time(bid.created_at)
  );
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
