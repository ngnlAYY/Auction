// 账户路由（`/api/v1/me/*` 与代理授权投影）的 data 校验。
// `validQuota` / `validTransitions` 同时服务 lot 路由的命令授权投影，以导出符号共享，
// 避免同一代理生命周期谓词出现两份实现。

import {
  type PublicMyBidsResponse,
  PublicMyBidsResponseSchema,
  type PublicMyProxyResponse,
  PublicMyProxyResponseSchema,
} from "@auction/contracts/dto/public/account";
import type { TransportResponse } from "@auction/web-shared/net/transport";
import { type JsonObject, jsonKeysOnly } from "@auction/web-shared/net/wire";
import type { Result } from "@auction/web-shared/result";
import { validUuid } from "@auction/web-shared/ui/validation";

import {
  boolValue,
  decodeSuccess,
  int,
  nonemptyMode,
  nonnegative,
  object,
  optionalInt,
  optionalString,
  positive,
  proxyMode,
  proxyStatus,
  string,
  time,
  timeValue,
} from ".";

/** 解码当前用户的竞价与代理历史页。 */
export function decodeMyBids(response: TransportResponse): Result<PublicMyBidsResponse, string> {
  return decodeSuccess(response, PublicMyBidsResponseSchema, validMyBids);
}

/** 解码当前用户对指定 lot 的代理授权投影。 */
export function decodeMyProxy(
  response: TransportResponse,
  lotId: string,
): Result<PublicMyProxyResponse, string> {
  return decodeSuccess(response, PublicMyProxyResponseSchema, (value) =>
    validMyProxy(value, lotId),
  );
}

function validMyBids(value: unknown): boolean {
  const decoded = object(value);
  if (decoded === null) {
    return false;
  }
  const bids = decoded.bids;
  const proxies = decoded.proxies;
  return (
    jsonKeysOnly(
      decoded,
      [
        "bids",
        "proxies",
        "bid_total",
        "proxy_total",
        "page",
        "bid_page",
        "proxy_page",
        "page_size",
      ],
      [],
    ) &&
    positive(decoded.page) &&
    positive(decoded.bid_page) &&
    positive(decoded.proxy_page) &&
    positive(decoded.page_size) &&
    Array.isArray(bids) &&
    bids.every(validAccountBid) &&
    Array.isArray(proxies) &&
    proxies.every((proxy) => validOwnedProxy(proxy, null, false)) &&
    nonnegative(decoded.bid_total) &&
    nonnegative(decoded.proxy_total)
  );
}

function validMyProxy(value: unknown, lotId: string): boolean {
  const decoded = object(value);
  if (decoded === null || !jsonKeysOnly(decoded, ["authorization"], [])) {
    return false;
  }
  const authorization = decoded.authorization;
  return authorization === null || validOwnedProxy(authorization, lotId, true);
}

function validOwnedBid(value: unknown): boolean {
  return validOwnedBidWithEquipment(value, true);
}

function validAccountBid(value: unknown): boolean {
  return validOwnedBidWithEquipment(value, false);
}

function validOwnedBidWithEquipment(value: unknown, trimmedEquipment: boolean): boolean {
  const decoded = object(value);
  if (decoded === null) {
    return false;
  }
  return (
    jsonKeysOnly(
      decoded,
      [
        "id",
        "auction_id",
        "lot_id",
        "lot_number",
        "amount",
        "accepted_state_version",
        "fact_index",
        "mechanism",
        "created_at",
        "leading",
        "equipment",
      ],
      [],
    ) &&
    validUuid(string(decoded.id) ?? "") &&
    validUuid(string(decoded.auction_id) ?? "") &&
    validUuid(string(decoded.lot_id) ?? "") &&
    positive(decoded.lot_number) &&
    positive(decoded.amount) &&
    positive(decoded.accepted_state_version) &&
    positive(decoded.fact_index) &&
    (string(decoded.mechanism) === "ordinary" || string(decoded.mechanism) === "proxy") &&
    time(decoded.created_at) &&
    boolValue(decoded.leading) !== null &&
    validOwnedEquipment(decoded.equipment, trimmedEquipment)
  );
}

function validOwnedProxy(value: unknown, lotId: string | null, trimmedEquipment: boolean): boolean {
  const decoded = object(value);
  if (decoded === null) {
    return false;
  }
  const bidPresent = Object.hasOwn(decoded, "bid");
  const bid = object(decoded.bid);
  if (
    !jsonKeysOnly(
      decoded,
      [
        "authorization_id",
        "auction_id",
        "lot_id",
        "lot_number",
        "authorization_sequence",
        "usage_number",
        "mode",
        "maximum_amount",
        "quota",
        "transitions",
        "status",
        "state_version",
        "created_at",
        "updated_at",
        "equipment",
        "capabilities",
      ],
      ["bid", "invalid_reason", "terminal_reason"],
    ) ||
    !validUuid(string(decoded.authorization_id) ?? "") ||
    !validUuid(string(decoded.auction_id) ?? "") ||
    (lotId !== null && string(decoded.lot_id) !== lotId) ||
    !validUuid(string(decoded.lot_id) ?? "") ||
    !positive(decoded.lot_number) ||
    !positive(decoded.authorization_sequence) ||
    !positive(decoded.usage_number) ||
    !proxyMode(decoded.mode) ||
    !positive(decoded.maximum_amount) ||
    !validQuota(decoded.quota, int(decoded.usage_number)) ||
    !positive(decoded.state_version) ||
    !validOwnedLifecycle(decoded) ||
    !validCapabilities(decoded.capabilities) ||
    !validTransitions(decoded.transitions, decoded) ||
    !time(decoded.created_at) ||
    !time(decoded.updated_at) ||
    !validOwnedEquipment(decoded.equipment, trimmedEquipment) ||
    (bidPresent && !validOwnedBid(decoded.bid)) ||
    (bidPresent &&
      (string(bid?.auction_id) !== string(decoded.auction_id) ||
        string(bid?.lot_id) !== string(decoded.lot_id) ||
        int(bid?.lot_number) !== int(decoded.lot_number)))
  ) {
    return false;
  }
  const status = string(decoded.status);
  const capabilities = object(decoded.capabilities);
  if (status !== "active") {
    return (
      capabilities !== null &&
      boolValue(capabilities.can_lower) === false &&
      boolValue(capabilities.waiting) === false &&
      boolValue(capabilities.at_maximum) === false
    );
  }
  const activeBid = object(decoded.bid);
  if (activeBid === null) {
    return false;
  }
  return (
    capabilities !== null &&
    boolValue(activeBid.leading) !== boolValue(capabilities.waiting) &&
    (boolValue(capabilities.waiting) !== true || string(decoded.mode) === "slow")
  );
}

/** 转换历史必须覆盖完整的版本链，且末项与当前授权一致。 */
export function validTransitions(value: unknown, authorization: JsonObject): boolean {
  if (!Array.isArray(value)) {
    return false;
  }
  const transitions = value;
  const stateVersion = int(authorization.state_version);
  if (stateVersion === null) {
    return false;
  }
  if (transitions.length !== stateVersion || transitions.length === 0) {
    return false;
  }
  let previous: JsonObject | null = null;
  for (let index = 0; index < transitions.length; index += 1) {
    const transition = object(transitions[index]);
    if (transition === null || !validTransition(transition, index + 1)) {
      return false;
    }
    if (previous !== null) {
      const currentTime = timeValue(transition.created_at);
      const previousTime = timeValue(previous.created_at);
      if (
        optionalInt(transition, "old_maximum_amount") !==
          optionalInt(previous, "new_maximum_amount") ||
        optionalString(transition, "old_mode") !== optionalString(previous, "new_mode") ||
        optionalString(transition, "old_status") !== optionalString(previous, "new_status") ||
        (currentTime !== null && previousTime !== null && currentTime < previousTime)
      ) {
        return false;
      }
    }
    previous = transition;
  }
  if (previous === null) {
    return false;
  }
  return (
    optionalInt(previous, "new_maximum_amount") === int(authorization.maximum_amount) &&
    optionalString(previous, "new_mode") === string(authorization.mode) &&
    optionalString(previous, "new_status") === string(authorization.status) &&
    optionalString(previous, "reason") === optionalString(authorization, "terminal_reason") &&
    int(previous.resulting_state_version) === stateVersion
  );
}

function validTransition(value: JsonObject, number: number): boolean {
  if (
    !jsonKeysOnly(
      value,
      [
        "transition_number",
        "kind",
        "old_maximum_amount",
        "new_maximum_amount",
        "old_mode",
        "new_mode",
        "old_status",
        "new_status",
        "reason",
        "resulting_state_version",
        "created_at",
      ],
      [],
    ) ||
    int(value.transition_number) !== number ||
    int(value.resulting_state_version) !== number ||
    !positive(value.new_maximum_amount) ||
    !proxyMode(value.new_mode) ||
    !proxyStatus(value.new_status) ||
    !time(value.created_at)
  ) {
    return false;
  }
  const oldState =
    positive(value.old_maximum_amount) &&
    proxyMode(value.old_mode) &&
    proxyStatus(value.old_status);
  switch (string(value.kind)) {
    case "created":
      return (
        number === 1 &&
        value.old_maximum_amount === null &&
        value.old_mode === null &&
        value.old_status === null &&
        string(value.new_status) === "active" &&
        value.reason === null
      );
    case "lowered": {
      const newMaximum = int(value.new_maximum_amount);
      const oldMaximum = optionalInt(value, "old_maximum_amount");
      return (
        oldState &&
        newMaximum !== null &&
        oldMaximum !== null &&
        newMaximum < oldMaximum &&
        string(value.old_mode) === string(value.new_mode) &&
        string(value.old_status) === "active" &&
        string(value.new_status) === "active" &&
        value.reason === null
      );
    }
    case "final_fast":
      return (
        oldState &&
        int(value.new_maximum_amount) === optionalInt(value, "old_maximum_amount") &&
        string(value.old_mode) === "slow" &&
        string(value.new_mode) === "fast" &&
        string(value.old_status) === "active" &&
        string(value.new_status) === "active" &&
        value.reason === null
      );
    case "exhausted": {
      const reason = string(value.reason);
      return (
        unchangedActive(value) &&
        string(value.new_status) === "exhausted" &&
        (reason === "strictly_defeated" || reason === "ranked_below")
      );
    }
    case "frozen":
      return (
        unchangedActive(value) &&
        string(value.new_status) === "frozen" &&
        string(value.reason) === "eligibility_lost"
      );
    case "closed":
      return (
        unchangedActive(value) &&
        string(value.new_status) === "closed" &&
        string(value.reason) === "competition_closed"
      );
    default:
      return false;
  }
}

function unchangedActive(value: JsonObject): boolean {
  return (
    positive(value.old_maximum_amount) &&
    int(value.new_maximum_amount) === optionalInt(value, "old_maximum_amount") &&
    proxyMode(value.old_mode) &&
    string(value.old_mode) === string(value.new_mode) &&
    string(value.old_status) === "active"
  );
}

function validOwnedLifecycle(value: JsonObject): boolean {
  switch (string(value.status)) {
    case "active":
      return !Object.hasOwn(value, "terminal_reason") && !Object.hasOwn(value, "invalid_reason");
    case "exhausted": {
      const terminalReason = string(value.terminal_reason);
      return (
        (terminalReason === "strictly_defeated" || terminalReason === "ranked_below") &&
        string(value.invalid_reason) === "outbid"
      );
    }
    case "frozen":
      return (
        string(value.terminal_reason) === "eligibility_lost" &&
        string(value.invalid_reason) === "frozen"
      );
    case "closed":
      return (
        string(value.terminal_reason) === "competition_closed" &&
        string(value.invalid_reason) === "closed"
      );
    default:
      return false;
  }
}

function validOwnedEquipment(value: unknown, trimmed: boolean): boolean {
  const decoded = object(value);
  return (
    decoded !== null &&
    jsonKeysOnly(decoded, ["name", "category"], []) &&
    nonemptyMode(decoded.name, trimmed) &&
    nonemptyMode(decoded.category, trimmed)
  );
}

/** quota 的 `limit = 0` 表示无限；非零时用量不得超过上限，且不小于授权的使用序号。 */
export function validQuota(value: unknown, usageNumber: number | null): boolean {
  const decoded = object(value);
  if (decoded === null) {
    return false;
  }
  const used = int(decoded.used);
  const limit = int(decoded.limit);
  return (
    jsonKeysOnly(decoded, ["used", "limit"], []) &&
    positive(decoded.used) &&
    nonnegative(decoded.limit) &&
    (limit === 0 || (used !== null && limit !== null && used <= limit)) &&
    (usageNumber === null || (used !== null && used >= usageNumber))
  );
}

function validCapabilities(value: unknown): boolean {
  const decoded = object(value);
  if (decoded === null) {
    return false;
  }
  const canLower = boolValue(decoded.can_lower);
  const waiting = boolValue(decoded.waiting);
  const atMaximum = boolValue(decoded.at_maximum);
  return (
    jsonKeysOnly(decoded, ["can_lower", "waiting", "at_maximum"], []) &&
    canLower !== null &&
    waiting !== null &&
    atMaximum !== null &&
    !(atMaximum === true && (canLower === true || waiting === true))
  );
}
