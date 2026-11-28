// 本人 v2 寄件指引、预约签发与代管件撤回意愿；v2 decoder 只经动态导入加载。

import { PUBLIC_EXPECTED_USER_ID_HEADER } from "@auction/contracts/dto/public/commands";
import type {
  DepositInstructions,
  DepositReservationReceipt,
  IssueDepositReservation,
} from "@auction/contracts/dto/public/consignment/deposit";
import type { WithdrawReceiptV2 } from "@auction/contracts/dto/public/consignment/listing-v2";
import { ConsignmentUuidSchema } from "@auction/contracts/dto/public/consignment/scalar";
import { ApiErrorResponseSchema, apiResponseSchema } from "@auction/contracts/dto/response";
import { decode } from "@auction/contracts/wire";
import type { TransportResponse } from "@auction/web-shared/net/transport";
import { type JsonFieldPolicy, jsonHasExactKeys } from "@auction/web-shared/net/wire";
import { err, ok, type Result } from "@auction/web-shared/result";
import type { GenericSchema } from "valibot";

import { sendRequest } from "./clock";
import { COMMAND_NOT_SENT, type CommandError } from "./commands";
import { observePublicSessionFailure } from "./session";
import { publicSessionIdentityRevision } from "./session/revision";

const INSTRUCTIONS = "/api/v2/me/consignment/listings";
const WITHDRAW = "/api/v2/consignment/listings";
const ERROR_KEYS = ["code", "message", "data", "request_id"];
const SUCCESS_KEYS = ["code", "message", "data"];

const GUARD_REJECTIONS: Readonly<Record<string, number>> = {
  invalid_expected_user_id: 400,
  unauthenticated: 401,
  session_changed: 409,
  rate_limited: 429,
};

const RESERVATION_REJECTIONS: Readonly<Record<string, number>> = {
  ...GUARD_REJECTIONS,
  invalid_request: 400,
  consignment_disabled: 404,
  participant_ineligible: 403,
  bot_party: 403,
  action_not_allowed: 403,
  command_id_conflict: 409,
  stale_version: 409,
  listing_not_active: 409,
  equipment_busy: 409,
  season_closed: 409,
  listing_limit_reached: 429,
  mail_unavailable: 503,
};

const WITHDRAW_REJECTIONS: Readonly<Record<string, number>> = {
  ...GUARD_REJECTIONS,
  invalid_request: 400,
  participant_ineligible: 403,
  bot_party: 403,
  action_not_allowed: 403,
  command_id_conflict: 409,
  stale_version: 409,
  listing_not_active: 409,
  equipment_committed: 409,
  custody_unproven: 409,
  season_closed: 409,
};

const ERROR_CODES = new Set([
  ...Object.keys(RESERVATION_REJECTIONS),
  ...Object.keys(WITHDRAW_REJECTIONS),
  "outcome_unknown",
  "consignment_unavailable",
  "storage_unavailable",
  "listing_not_found",
  "unknown_error",
]);

type DepositSchemas = typeof import("@auction/contracts/dto/public/consignment/deposit");
type ListingSchemas = typeof import("@auction/contracts/dto/public/consignment/listing-v2");
let depositSchemas: Promise<DepositSchemas> | undefined;
let listingSchemas: Promise<ListingSchemas> | undefined;

/** 分块加载失败不缓存：下一次进入寄售页可重新拉取同一批契约。 */
async function schemas(): Promise<
  Result<[DepositSchemas, ListingSchemas], ConsignmentDepositError>
> {
  depositSchemas ??= import("@auction/contracts/dto/public/consignment/deposit");
  listingSchemas ??= import("@auction/contracts/dto/public/consignment/listing-v2");
  try {
    return ok(await Promise.all([depositSchemas, listingSchemas]));
  } catch {
    depositSchemas = undefined;
    listingSchemas = undefined;
    return err({ status: 0, code: "consignment_unavailable" });
  }
}

type Request =
  | { readonly method: "GET" }
  | {
      readonly method: "POST";
      readonly body: object;
      readonly onSessionGuardRejected: (() => void) | undefined;
    };

type SuccessStatus = 200 | 201 | 202;

/** 只暴露 HTTP 状态及安全分类；未知协议码保留为 unknown_error。 */
export type ConsignmentDepositError = CommandError;

/** 寄售路径和命令使用同一规范 UUID 校验。 */
export function validDepositUuid(value: string): boolean {
  return decode(ConsignmentUuidSchema, value).ok;
}

async function requestDeposit<T>(
  userId: string,
  url: string,
  request: Request,
  statuses: readonly SuccessStatus[],
  schema: GenericSchema<unknown, T>,
  signal: AbortSignal,
  integerPaths: NonNullable<JsonFieldPolicy["integerPaths"]>,
): Promise<Result<T, ConsignmentDepositError>> {
  if (!validDepositUuid(userId)) return err({ status: 400, code: "invalid_expected_user_id" });
  const identityAtStart = publicSessionIdentityRevision();
  let dispatched = false;
  let unauthorized = false;
  const deferUnauthorized =
    request.method !== "GET" && request.onSessionGuardRejected !== undefined;
  const sent = await sendRequest(
    (requestSignal) => {
      if (requestSignal.aborted) return err({ kind: "network", status: null });
      dispatched = true;
      const headers = {
        [PUBLIC_EXPECTED_USER_ID_HEADER]: userId,
        Accept: "application/json",
      };
      return ok({
        url,
        init: {
          method: request.method,
          credentials: "same-origin",
          cache: "no-store",
          redirect: "error",
          ...(request.method === "GET"
            ? { headers }
            : {
                headers: { ...headers, "content-type": "application/json" },
                body: JSON.stringify(request.body),
              }),
        },
      });
    },
    signal,
    (received) => {
      if (received === 401 && !signal.aborted) {
        unauthorized = true;
        if (!deferUnauthorized) observePublicSessionFailure(received, "unauthenticated", userId);
      }
    },
  );
  if (!dispatched) return err(COMMAND_NOT_SENT);
  if (signal.aborted) return err({ status: 0, code: "network_error" });
  if (unauthorized && request.method === "GET") {
    return err({ status: 401, code: "unauthenticated" });
  }
  const result = sent.ok
    ? decodeResponse(sent.value, statuses, request.method, schema, integerPaths)
    : err({
        status: sent.error.kind === "network" ? (sent.error.status ?? 0) : 0,
        code: sent.error.kind === "network" ? "network_error" : "request_timeout",
      });
  if (!result.ok) {
    const { status: rejectedStatus, code } = result.error;
    const sessionChanged = rejectedStatus === 409 && code === "session_changed";
    if (
      request.method !== "GET" &&
      (sessionChanged || (rejectedStatus === 401 && code === "unauthenticated"))
    ) {
      request.onSessionGuardRejected?.();
    }
    if (deferUnauthorized && unauthorized) {
      observePublicSessionFailure(401, "unauthenticated", userId);
    } else if (sessionChanged) {
      observePublicSessionFailure(rejectedStatus, code, userId, identityAtStart);
    }
  }
  return result;
}

function decodeResponse<T>(
  response: TransportResponse,
  statuses: readonly SuccessStatus[],
  method: Request["method"],
  schema: GenericSchema<unknown, T>,
  integerPaths: NonNullable<JsonFieldPolicy["integerPaths"]>,
): Result<T, ConsignmentDepositError> {
  const invalid = { status: response.status, code: "invalid_response" };
  const json = response.json({ uniqueKeys: "all", integerPaths });
  if (!json.ok) return err(invalid);
  if (statuses.includes(response.status as SuccessStatus)) {
    if (method !== "GET" && !jsonHasExactKeys(json.value, SUCCESS_KEYS)) return err(invalid);
    const decoded = decode(apiResponseSchema(schema), json.value);
    const expectedCode =
      response.status === 200 ? "ok" : response.status === 201 ? "created" : "accepted";
    return decoded.ok && decoded.value.code === expectedCode
      ? ok(decoded.value.data)
      : err(invalid);
  }
  if (response.status < 400) return err(invalid);
  if (method !== "GET" && !jsonHasExactKeys(json.value, ERROR_KEYS)) return err(invalid);
  const decoded = decode(ApiErrorResponseSchema, json.value);
  if (!decoded.ok || decoded.value.data !== null) return err(invalid);
  const code = ERROR_CODES.has(decoded.value.code) ? decoded.value.code : "unknown_error";
  return err({ status: response.status, code });
}

/** 读取本人当前寄件指引；`none` 不含秘密。 */
export async function fetchDepositInstructions(
  userId: string,
  listingId: string,
  signal: AbortSignal,
): Promise<Result<DepositInstructions, ConsignmentDepositError>> {
  if (!validDepositUuid(listingId)) return err({ status: 400, code: "invalid_request" });
  const loaded = await schemas();
  if (!loaded.ok) return loaded;
  const [{ DepositInstructionsSchema }] = loaded.value;
  return requestDeposit(
    userId,
    `${INSTRUCTIONS}/${listingId}/deposit-instructions`,
    { method: "GET" },
    [200],
    DepositInstructionsSchema,
    signal,
    [],
  );
}

/** 签发或换发预约；重放必须复用同一命令 ID 与预期版本。 */
export async function issueDepositReservation(
  userId: string,
  listingId: string,
  command: IssueDepositReservation,
  signal: AbortSignal,
  onSessionGuardRejected?: () => void,
): Promise<Result<DepositReservationReceipt, ConsignmentDepositError>> {
  if (!validDepositUuid(listingId)) return err({ status: 400, code: "invalid_request" });
  const loaded = await schemas();
  if (!loaded.ok) return loaded;
  const [{ IssueDepositReservationSchema, DepositReservationReceiptSchema }] = loaded.value;
  const decoded = decode(IssueDepositReservationSchema, command);
  if (!decoded.ok) return err({ status: 400, code: "invalid_request" });
  const result = await requestDeposit(
    userId,
    `${INSTRUCTIONS}/${listingId}/deposit-reservations`,
    { method: "POST", body: decoded.value, onSessionGuardRejected },
    [201],
    DepositReservationReceiptSchema,
    signal,
    [["data", "listing_version"]],
  );
  return result.ok &&
    (result.value.command_id !== decoded.value.command_id ||
      result.value.listing_id !== listingId ||
      result.value.listing_version !== command.expected_version + 1)
    ? err({ status: 201, code: "invalid_response" })
    : result;
}

/** 代管件登记撤回意愿；未代管件由同一 v2 命令返回 200 closed。 */
export async function withdrawDepositListing(
  userId: string,
  listingId: string,
  command: { readonly command_id: string; readonly expected_version: number },
  signal: AbortSignal,
  onSessionGuardRejected?: () => void,
): Promise<Result<WithdrawReceiptV2, ConsignmentDepositError>> {
  if (!validDepositUuid(listingId)) return err({ status: 400, code: "invalid_request" });
  const loaded = await schemas();
  if (!loaded.ok) return loaded;
  const [, { WithdrawListingV2Schema, WithdrawReceiptV2Schema }] = loaded.value;
  const decoded = decode(WithdrawListingV2Schema, command);
  if (!decoded.ok) return err({ status: 400, code: "invalid_request" });
  const result = await requestDeposit(
    userId,
    `${WITHDRAW}/${listingId}/withdraw`,
    { method: "POST", body: decoded.value, onSessionGuardRejected },
    [200, 202],
    WithdrawReceiptV2Schema,
    signal,
    [["data", "listing_version"]],
  );
  return result.ok &&
    (result.value.command_id !== decoded.value.command_id ||
      result.value.listing_id !== listingId ||
      // 已有撤回意愿时服务端复用当前版本并返回新命令的意愿回执，不再推进版本。
      !(
        result.value.listing_version === command.expected_version + 1 ||
        (result.value.disposition === "withdrawal_requested" &&
          result.value.listing_version === command.expected_version)
      ))
    ? err({ status: 200, code: "invalid_response" })
    : result;
}

/** 只有完整外壳中的可信拒绝才能结束首次提交；其余保留原命令。 */
export function depositWriteRejected(
  operation: "issue_reservation" | "withdraw_deposit",
  error: ConsignmentDepositError,
): boolean {
  if (error.code === "command_not_sent") return true;
  const table = operation === "issue_reservation" ? RESERVATION_REJECTIONS : WITHDRAW_REJECTIONS;
  return table[error.code] === error.status;
}
