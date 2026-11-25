// 本人重新上架：未付款收回后，对仍在代管的原件再开一代销售。
//
// 命令与撤回同属 v2 路由，目标与版本固定；浏览器不判断能否重列，只提交服务端
// allowed_actions 里给出的动作，回执必须绑定原命令、目标与版本。

import { PUBLIC_EXPECTED_USER_ID_HEADER } from "@auction/contracts/dto/public/commands";
import { CommandUuidV4Schema } from "@auction/contracts/dto/public/consignment/command";
import {
  type RelistReceiptV2,
  RelistReceiptV2Schema,
} from "@auction/contracts/dto/public/consignment/receipt-v2";
import {
  ConsignmentUuidSchema,
  ListingVersionSchema,
} from "@auction/contracts/dto/public/consignment/scalar";
import { ApiErrorResponseSchema, apiResponseSchema } from "@auction/contracts/dto/response";
import { decode, strictStruct } from "@auction/contracts/wire";
import { type TransportResponse, transportErrorCode } from "@auction/web-shared/net/transport";
import { jsonHasExactKeys } from "@auction/web-shared/net/wire";
import { err, ok, type Result } from "@auction/web-shared/result";
import type * as v from "valibot";

import { sendRequest } from "./clock";
import { COMMAND_NOT_SENT, type CommandError, commandNotSent } from "./commands";
import { observePublicSessionFailure } from "./session";

const LISTINGS = "/api/v2/consignment/listings";
const ERROR_KEYS = ["code", "message", "data", "request_id"];
const SUCCESS_KEYS = ["code", "message", "data"];

/** 重列请求：稳定命令 ID 与预期版本（D9 §6.2 重列行）。 */
export const RelistRequestSchema = strictStruct({
  command_id: CommandUuidV4Schema,
  expected_version: ListingVersionSchema,
});
/** 重列请求。 */
export type RelistRequest = v.InferOutput<typeof RelistRequestSchema>;

/** 重列回执，形状由契约包的 `RelistReceiptV2Schema` 定义。 */
export type RelistReceipt = RelistReceiptV2;

const GUARD_REJECTIONS: Readonly<Record<string, number>> = {
  invalid_expected_user_id: 400,
  unauthenticated: 401,
  session_changed: 409,
  rate_limited: 429,
};
// D9 §6.1：重列的拒绝码；只有这些状态码与协议码的组合能证明首次写入被拒绝。
const RELIST_REJECTIONS: Readonly<Record<string, number>> = {
  ...GUARD_REJECTIONS,
  invalid_request: 400,
  participant_ineligible: 403,
  action_not_allowed: 403,
  command_id_conflict: 409,
  stale_version: 409,
  listing_not_active: 409,
  equipment_committed: 409,
  custody_unproven: 409,
  season_closed: 409,
  operations_hold: 409,
  equipment_busy: 409,
  listing_limit_reached: 429,
};
const ERROR_CODES = new Set([
  ...Object.keys(RELIST_REJECTIONS),
  "consignment_disabled",
  "consignment_unavailable",
  "listing_not_found",
  "outcome_unknown",
  "storage_unavailable",
  "timeout",
]);

/** 只暴露 HTTP 状态与安全分类。 */
export type RelistError = CommandError;

/**
 * 不变量 1、6：本地未发送或完整外壳中的可信拒绝才可结束首次提交。
 * 重放时守卫外壳（会话、预算）不查账本，不能证明原命令未生效，因此不结案。
 */
export function relistRejected(error: RelistError, replay: boolean): boolean {
  if (commandNotSent(error)) return !replay;
  if (replay && GUARD_REJECTIONS[error.code] === error.status) return false;
  return RELIST_REJECTIONS[error.code] === error.status;
}

/**
 * 回执必须绑定原命令与目标。重列是否把挂单版本推进恰好 +1，7C-1 的服务与领域代码里没有依据
 * （重列写端尚未合入），因此不比较版本，避免把合法回执误判为无效。
 */
export function relistReceiptMatches(
  receipt: RelistReceipt,
  listingId: string,
  command: RelistRequest,
): boolean {
  return receipt.command_id === command.command_id && receipt.listing_id === listingId;
}

/** 不变量 1、6、7：重放必须复用同一命令 ID 与预期版本。 */
export async function relistConsignmentListing(
  userId: string,
  listingId: string,
  command: RelistRequest,
  signal: AbortSignal,
  onSessionGuardRejected?: () => void,
): Promise<Result<RelistReceipt, RelistError>> {
  if (!decode(ConsignmentUuidSchema, userId).ok) {
    return err({ status: 400, code: "invalid_expected_user_id" });
  }
  const body = decode(RelistRequestSchema, command);
  if (!decode(ConsignmentUuidSchema, listingId).ok || !body.ok) {
    return err({ status: 400, code: "invalid_request" });
  }
  let dispatched = false;
  let unauthorized = false;
  const sent = await sendRequest(
    (requestSignal) => {
      if (requestSignal.aborted) return err({ kind: "network", status: null });
      dispatched = true;
      return ok({
        url: `${LISTINGS}/${listingId}/relist`,
        init: {
          method: "POST",
          credentials: "same-origin",
          cache: "no-store",
          redirect: "error",
          headers: {
            [PUBLIC_EXPECTED_USER_ID_HEADER]: userId,
            Accept: "application/json",
            "content-type": "application/json",
          },
          body: JSON.stringify(body.value),
        },
      });
    },
    signal,
    (received) => {
      if (received === 401 && !signal.aborted) unauthorized = true;
    },
  );
  if (!dispatched) return err(COMMAND_NOT_SENT);
  if (signal.aborted) return err({ status: 0, code: "network_error" });
  const result = sent.ok
    ? decodeRelist(sent.value)
    : err({
        status: sent.error.kind === "network" ? (sent.error.status ?? 0) : 0,
        code: transportErrorCode(sent.error),
      });
  if (!result.ok) {
    const { status, code } = result.error;
    const sessionChanged = status === 409 && code === "session_changed";
    // 不变量 1：只有完整守卫外壳可了结首次写入；通知会话前同步完成。
    if (sessionChanged || (status === 401 && code === "unauthenticated"))
      onSessionGuardRejected?.();
    if (unauthorized) observePublicSessionFailure(401, "unauthenticated", userId);
    else if (sessionChanged) observePublicSessionFailure(status, code, userId);
    return result;
  }
  return relistReceiptMatches(result.value, listingId, command)
    ? result
    : err({ status: 200, code: "invalid_response" });
}

function decodeRelist(response: TransportResponse): Result<RelistReceipt, RelistError> {
  const invalid = { status: response.status, code: "invalid_response" };
  const json = response.json({
    uniqueKeys: "all",
    integerPaths: [
      ["data", "listing_version"],
      ["data", "price"],
      ["data", "generation"],
    ],
  });
  if (!json.ok) return err(invalid);
  if (response.status === 200) {
    if (!jsonHasExactKeys(json.value, SUCCESS_KEYS)) return err(invalid);
    const decoded = decode(apiResponseSchema(RelistReceiptV2Schema), json.value);
    return decoded.ok && decoded.value.code === "ok" ? ok(decoded.value.data) : err(invalid);
  }
  if (response.status < 400) return err(invalid);
  if (!jsonHasExactKeys(json.value, ERROR_KEYS)) return err(invalid);
  const decoded = decode(ApiErrorResponseSchema, json.value);
  if (!decoded.ok || decoded.value.data !== null) return err(invalid);
  const code = ERROR_CODES.has(decoded.value.code) ? decoded.value.code : "unknown_error";
  return err({ status: response.status, code });
}
