// 受限权利的 Public 请求：找回、挑战、核验、当前权利与续办。
//
// 权利人没有登录会话，请求不带预期身份头；会话令牌只在 HttpOnly 的 `hv_right` Cookie 里，
// 脚本既读不到也不转发。定位符与挑战码只进这一次请求正文，不进地址、存储或错误。

import {
  type ChallengeRightRequest,
  ChallengeRightRequestSchema,
  type ClaimRightRequest,
  ClaimRightRequestSchema,
  type LocateRightRequest,
  LocateRightRequestSchema,
  type RightChallengeAccepted,
  RightChallengeAcceptedSchema,
  type RightClaimReceipt,
  RightClaimReceiptSchema,
  type RightVerifiedGrant,
  RightVerifiedGrantSchema,
  type RightView,
  RightViewSchema,
  type VerifyRightRequest,
  VerifyRightRequestSchema,
} from "@auction/contracts/dto/public/consignment/rights";
import { ConsignmentUuidSchema } from "@auction/contracts/dto/public/consignment/scalar";
import { ApiErrorResponseSchema, apiResponseSchema } from "@auction/contracts/dto/response";
import { decode } from "@auction/contracts/wire";
import { type TransportResponse, transportErrorCode } from "@auction/web-shared/net/transport";
import { type JsonFieldPolicy, jsonHasExactKeys } from "@auction/web-shared/net/wire";
import { err, ok, type Result } from "@auction/web-shared/result";
import type { GenericSchema } from "valibot";

import { sendRequest } from "./clock";
import { COMMAND_NOT_SENT, type CommandError, commandNotSent } from "./commands";

const RIGHTS = "/api/v2/consignment/rights";
const ERROR_KEYS = ["code", "message", "data", "request_id"];
const SUCCESS_KEYS = ["code", "message", "data"];
const VERSION_PATHS = [["data", "right_version"]] as const;

// 只有本路由认可的状态码与协议码组合，才能证明首次写入已被拒绝（不变量 1、6）。
const COMMON_REJECTIONS: Readonly<Record<string, number>> = {
  invalid_request: 400,
  rate_limited: 429,
  command_id_conflict: 409,
};
/** 权利写操作的种类。 */
export type RightWrite = "locate" | "challenge" | "verify" | "claim";
const REJECTIONS: Readonly<Record<RightWrite, Readonly<Record<string, number>>>> = {
  locate: COMMON_REJECTIONS,
  challenge: COMMON_REJECTIONS,
  verify: { ...COMMON_REJECTIONS, challenge_invalid: 403 },
  claim: {
    ...COMMON_REJECTIONS,
    unauthenticated: 401,
    action_not_allowed: 403,
    stale_version: 409,
    right_expired: 409,
  },
};

const ERROR_CODES = new Set([
  ...Object.keys(REJECTIONS.claim),
  ...Object.keys(REJECTIONS.verify),
  "command_not_found",
  "outcome_unknown",
  "rights_unavailable",
  "timeout",
]);

/** 只暴露 HTTP 状态与安全分类；不回显服务端说明正文。 */
export type RightsError = CommandError;

/** 不变量 1、6：本地未发送或完整外壳中的可信拒绝才可结束首次提交；其余只许原样重放。 */
export function rightWriteRejected(operation: RightWrite, error: RightsError): boolean {
  return commandNotSent(error) || REJECTIONS[operation][error.code] === error.status;
}

interface Reply<T> {
  readonly status: number;
  readonly value: T;
}

async function requestRights<T>(
  path: string,
  body: object | null,
  statuses: readonly number[],
  schema: GenericSchema<unknown, T>,
  signal: AbortSignal,
  integerPaths: NonNullable<JsonFieldPolicy["integerPaths"]> = [],
): Promise<Result<Reply<T>, RightsError>> {
  let dispatched = false;
  const sent = await sendRequest((requestSignal) => {
    if (requestSignal.aborted) return err({ kind: "network", status: null });
    // 共享传输在构造成功后同步调用 fetch，之后断流、取消或超时无法确认命令未到达。
    dispatched = true;
    const headers = { Accept: "application/json" };
    return ok({
      url: `${RIGHTS}${path}`,
      init: {
        method: body === null ? "GET" : "POST",
        credentials: "same-origin",
        cache: "no-store",
        redirect: "error",
        ...(body === null
          ? { headers }
          : {
              headers: { ...headers, "content-type": "application/json" },
              body: JSON.stringify(body),
            }),
      },
    });
  }, signal);
  if (!dispatched) return err(COMMAND_NOT_SENT);
  if (signal.aborted) return err({ status: 0, code: "network_error" });
  if (!sent.ok) {
    return err({
      status: sent.error.kind === "network" ? (sent.error.status ?? 0) : 0,
      code: transportErrorCode(sent.error),
    });
  }
  return decodeRights(sent.value, body !== null, statuses, schema, integerPaths);
}

function decodeRights<T>(
  response: TransportResponse,
  write: boolean,
  statuses: readonly number[],
  schema: GenericSchema<unknown, T>,
  integerPaths: NonNullable<JsonFieldPolicy["integerPaths"]>,
): Result<Reply<T>, RightsError> {
  const invalid = { status: response.status, code: "invalid_response" };
  const json = response.json({ uniqueKeys: "all", integerPaths });
  if (!json.ok) return err(invalid);
  if (statuses.includes(response.status)) {
    if (write && !jsonHasExactKeys(json.value, SUCCESS_KEYS)) return err(invalid);
    const decoded = decode(apiResponseSchema(schema), json.value);
    const expected = response.status === 202 ? "accepted" : "ok";
    return decoded.ok && decoded.value.code === expected
      ? ok({ status: response.status, value: decoded.value.data })
      : err(invalid);
  }
  if (response.status < 400) return err(invalid);
  // 不变量 1：写入只有收到完整错误外壳，才可据状态码和协议码判定原命令。
  if (write && !jsonHasExactKeys(json.value, ERROR_KEYS)) return err(invalid);
  const decoded = decode(ApiErrorResponseSchema, json.value);
  if (!decoded.ok || decoded.value.data !== null) return err(invalid);
  const code = ERROR_CODES.has(decoded.value.code) ? decoded.value.code : "unknown_error";
  return err({ status: response.status, code });
}

async function accepted<T>(
  request: Promise<Result<Reply<T>, RightsError>>,
): Promise<Result<T, RightsError>> {
  const result = await request;
  return result.ok ? ok(result.value.value) : result;
}

/** 按游戏身份找回定位符指引；真实、虚拟与未匹配目标同形。 */
export function locateRight(
  command: LocateRightRequest,
  signal: AbortSignal,
): Promise<Result<RightChallengeAccepted, RightsError>> {
  const decoded = decode(LocateRightRequestSchema, command);
  if (!decoded.ok) return Promise.resolve(err({ status: 400, code: "invalid_request" }));
  return accepted(
    requestRights("/locators", decoded.value, [202], RightChallengeAcceptedSchema, signal),
  );
}

/** 按权利定位符请求挑战码；定位符只进请求正文。 */
export function challengeRight(
  command: ChallengeRightRequest,
  signal: AbortSignal,
): Promise<Result<RightChallengeAccepted, RightsError>> {
  const decoded = decode(ChallengeRightRequestSchema, command);
  if (!decoded.ok) return Promise.resolve(err({ status: 400, code: "invalid_request" }));
  return accepted(
    requestRights("/challenges", decoded.value, [202], RightChallengeAcceptedSchema, signal),
  );
}

/** 用挑战码换取单权利会话；令牌只在 Cookie 里，本函数只返回截止与版本。 */
export function verifyRight(
  command: VerifyRightRequest,
  signal: AbortSignal,
): Promise<Result<RightVerifiedGrant, RightsError>> {
  const decoded = decode(VerifyRightRequestSchema, command);
  if (!decoded.ok) return Promise.resolve(err({ status: 400, code: "invalid_request" }));
  return accepted(
    requestRights("/verify", decoded.value, [200], RightVerifiedGrantSchema, signal, VERSION_PATHS),
  );
}

/** 读取当前会话的权利视图；无会话为 401。 */
export function fetchCurrentRight(signal: AbortSignal): Promise<Result<RightView, RightsError>> {
  return accepted(requestRights("/current", null, [200], RightViewSchema, signal, VERSION_PATHS));
}

/** 续办：同一命令 ID 重放得到同一回执；处置必须与状态码一致。 */
export async function claimRight(
  command: ClaimRightRequest,
  signal: AbortSignal,
): Promise<Result<RightClaimReceipt, RightsError>> {
  const decoded = decode(ClaimRightRequestSchema, command);
  if (!decoded.ok) return err({ status: 400, code: "invalid_request" });
  const result = await requestRights(
    "/current/claim",
    decoded.value,
    [200, 202],
    RightClaimReceiptSchema,
    signal,
  );
  if (!result.ok) return result;
  const { status, value } = result.value;
  const acceptedNow = value.disposition === "claim_accepted";
  return value.command_id !== command.command_id || acceptedNow !== (status === 202)
    ? err({ status, code: "invalid_response" })
    : ok(value);
}

/** 重新验权后查询原续办命令的回执；404 才授权原样重送。 */
export function fetchRightCommandReceipt(
  commandId: string,
  signal: AbortSignal,
): Promise<Result<RightClaimReceipt, RightsError>> {
  if (!decode(ConsignmentUuidSchema, commandId).ok) {
    return Promise.resolve(err({ status: 400, code: "invalid_request" }));
  }
  return accepted(
    requestRights(`/current/commands/${commandId}`, null, [200], RightClaimReceiptSchema, signal),
  );
}
