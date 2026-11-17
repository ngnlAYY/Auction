// 竞价命令结果与代理授权命令结果的字段闭集、生命周期和转换历史校验。

import { type JsonObject, jsonKeysOnly } from "@auction/web-shared/net/wire";
import { validUuid } from "@auction/web-shared/ui/validation";

import { boolValue, int, object, positive, proxyMode, string, time } from "..";
import { validQuota, validTransitions } from "../account";

/** 校验竞价命令成功响应的外形。 */
export function validBidCommand(value: unknown): boolean {
  const decoded = object(value);
  return decoded !== null && jsonKeysOnly(decoded, ["bid"], []) && validBidResult(decoded.bid);
}

function validBidResult(value: unknown): boolean {
  const decoded = object(value);
  if (decoded === null) return false;
  return (
    jsonKeysOnly(
      decoded,
      ["lot_id", "current_price", "effective_end_at", "state_version", "leading"],
      [],
    ) &&
    validUuid(string(decoded.lot_id) ?? "") &&
    positive(decoded.current_price) &&
    time(decoded.effective_end_at) &&
    positive(decoded.state_version) &&
    boolValue(decoded.leading) !== null
  );
}

/** 校验代理授权命令成功响应，并绑定 lot、上限、模式与降低上限后的形态。 */
export function validProxyCommand(
  value: unknown,
  lotId: string,
  expectedMaximum: number | null,
  expectedMode: string | null,
  lowered: boolean,
): boolean {
  const decoded = object(value);
  if (
    decoded === null ||
    !jsonKeysOnly(decoded, ["authorization"], []) ||
    !validCommandAuthorization(decoded.authorization ?? null)
  ) {
    return false;
  }
  const authorization = object(decoded.authorization ?? null);
  if (authorization === null) return false;
  const status = string(authorization.status);
  const bid = object(authorization.bid);
  return (
    string(authorization.lot_id) === lotId &&
    validUuid(lotId) &&
    (expectedMaximum === null || int(authorization.maximum_amount) === expectedMaximum) &&
    (expectedMode === null || string(authorization.mode) === expectedMode) &&
    (lowered
      ? status === "active" && !Object.hasOwn(authorization, "bid")
      : (status === "active" || status === "exhausted") &&
        Object.hasOwn(authorization, "bid") &&
        bid !== null &&
        string(bid.lot_id) === lotId &&
        boolValue(bid.leading) === (status === "active"))
  );
}

function validCommandAuthorization(value: unknown): boolean {
  const decoded = object(value);
  if (decoded === null) return false;
  return (
    jsonKeysOnly(
      decoded,
      [
        "authorization_id",
        "lot_id",
        "authorization_sequence",
        "usage_number",
        "mode",
        "maximum_amount",
        "quota",
        "transitions",
        "status",
        "state_version",
      ],
      ["terminal_reason", "bid"],
    ) &&
    validUuid(string(decoded.authorization_id) ?? "") &&
    validUuid(string(decoded.lot_id) ?? "") &&
    positive(decoded.authorization_sequence) &&
    positive(decoded.usage_number) &&
    proxyMode(decoded.mode) &&
    positive(decoded.maximum_amount) &&
    validQuota(decoded.quota, int(decoded.usage_number)) &&
    positive(decoded.state_version) &&
    validLifecycle(decoded) &&
    validTransitions(decoded.transitions, decoded) &&
    (!Object.hasOwn(decoded, "bid") || validBidResult(decoded.bid))
  );
}

function validLifecycle(value: JsonObject): boolean {
  const status = string(value.status);
  const reason = string(value.terminal_reason);
  switch (status) {
    case "active":
      return !Object.hasOwn(value, "terminal_reason");
    case "exhausted":
      return reason === "strictly_defeated" || reason === "ranked_below";
    case "frozen":
      return reason === "eligibility_lost";
    case "closed":
      return reason === "competition_closed";
    default:
      return false;
  }
}
