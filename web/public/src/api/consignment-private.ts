// 寄售本人读取与卖家写入共用的会话传输、无缓存响应和安全错误投影。

import { PUBLIC_EXPECTED_USER_ID_HEADER } from "@auction/contracts/dto/public/commands";
import { ConsignmentUuidSchema } from "@auction/contracts/dto/public/consignment/scalar";
import { ApiErrorResponseSchema, apiResponseSchema } from "@auction/contracts/dto/response";
import { decode } from "@auction/contracts/wire";
import { type TransportResponse, transportErrorCode } from "@auction/web-shared/net/transport";
import { type JsonFieldPolicy, jsonHasExactKeys } from "@auction/web-shared/net/wire";
import { err, ok, type Result } from "@auction/web-shared/result";
import type { GenericSchema } from "valibot";

import { sendRequest } from "./clock";
import { COMMAND_NOT_SENT, type CommandError } from "./commands";
import { observePublicSessionFailure } from "./session";
import { publicSessionIdentityRevision } from "./session/revision";

const HTTP_ERRORS = new Set([
  "already_open",
  "body_too_large",
  "command_id_conflict",
  "command_not_found",
  "consignment_disabled",
  "consignment_unavailable",
  "equipment_busy",
  "fee_changed",
  "history_alias_conflict",
  "history_owner_mismatch",
  "history_unproven",
  "invalid_command_id",
  "invalid_equipment_url",
  "invalid_expected_user_id",
  "invalid_listing_id",
  "invalid_listing_request",
  "invalid_price",
  "invalid_query",
  "invalid_reason",
  "listing_limit_reached",
  "listing_not_active",
  "listing_not_found",
  "origin_forbidden",
  "rate_limited",
  "outcome_unknown",
  "participant_ineligible",
  "registration_rate_limited",
  "session_changed",
  "stale_version",
  "storage_unavailable",
  "timeout",
  "unsupported_media_type",
  "unauthenticated",
  "world_unavailable",
]);
const ERROR_KEYS = ["code", "message", "data", "request_id"];
const SUCCESS_KEYS = ["code", "message", "data"];

/** 只暴露 HTTP 状态及安全的错误分类；保留合同的逐码结果。 */
export type ConsignmentPrivateError = CommandError;

/** 寄售路径和命令使用同一规范 UUID 校验。 */
export function validConsignmentUuid(value: string): boolean {
  return decode(ConsignmentUuidSchema, value).ok;
}

type Request =
  | { readonly method: "GET" }
  | {
      readonly method: "POST" | "PATCH";
      readonly body: object;
      readonly onSessionGuardRejected: (() => void) | undefined;
    };

/** 不变量 1、6、7：绑定预期身份，只发送同源请求，不暴露正文或底层异常。 */
export async function requestPrivateConsignment<T>(
  userId: string,
  url: string,
  request: Request,
  status: 200 | 202,
  schema: GenericSchema<unknown, T>,
  signal: AbortSignal,
  integerPaths: NonNullable<JsonFieldPolicy["integerPaths"]>,
): Promise<Result<T, ConsignmentPrivateError>> {
  if (!validConsignmentUuid(userId)) return err({ status: 400, code: "invalid_expected_user_id" });

  const identityAtStart = publicSessionIdentityRevision();
  let dispatched = false;
  let unauthorized = false;
  const deferUnauthorized =
    request.method !== "GET" && request.onSessionGuardRejected !== undefined;
  const sent = await sendRequest(
    (requestSignal) => {
      if (requestSignal.aborted) return err({ kind: "network", status: null });
      dispatched = true;
      const headers = { [PUBLIC_EXPECTED_USER_ID_HEADER]: userId, Accept: "application/json" };
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
    ? decodePrivateResponse(sent.value, status, request.method, schema, integerPaths)
    : err({
        // 响应头已到但正文断流时没有可信错误外壳：状态归一为未知，命令结果保持不确定。
        status: 0,
        code: transportErrorCode(sent.error),
      });
  if (!result.ok) {
    const { status: rejectedStatus, code } = result.error;
    const sessionChanged = rejectedStatus === 409 && code === "session_changed";
    // 不变量 1：只有完整守卫外壳可了结首次写入；通知会话前同步完成，避免宿主卸载取消清理。
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

/** 不变量 1、6：成功外壳按约定状态解码，错误只在完整外壳里取安全码；免登录登记复用同一口径。 */
export function decodePrivateResponse<T>(
  response: TransportResponse,
  status: 200 | 202,
  method: Request["method"],
  schema: GenericSchema<unknown, T>,
  integerPaths: NonNullable<JsonFieldPolicy["integerPaths"]>,
): Result<T, ConsignmentPrivateError> {
  const invalid = { status: response.status, code: "invalid_response" };
  const json = response.json({ uniqueKeys: "all", integerPaths });
  if (!json.ok) return err(invalid);
  if (response.status === status) {
    if (method !== "GET" && !jsonHasExactKeys(json.value, SUCCESS_KEYS)) return err(invalid);
    const decoded = decode(apiResponseSchema(schema), json.value);
    return decoded.ok && decoded.value.code === (status === 202 ? "accepted" : "ok")
      ? ok(decoded.value.data)
      : err(invalid);
  }
  if (response.status < 400) return err(invalid);
  // 不变量 1：写入只有收到完整错误外壳，才可据状态码和协议码判定原命令。
  if (method !== "GET" && !jsonHasExactKeys(json.value, ERROR_KEYS)) return err(invalid);
  const decoded = decode(ApiErrorResponseSchema, json.value);
  if (!decoded.ok || decoded.value.data !== null) return err(invalid);

  const code = HTTP_ERRORS.has(decoded.value.code) ? decoded.value.code : "unknown_error";
  return err({ status: response.status, code });
}
