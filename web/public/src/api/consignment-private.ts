// 寄售本人读取与卖家写入共用的会话传输、无缓存响应和安全错误投影。

import { PUBLIC_EXPECTED_USER_ID_HEADER } from "@auction/contracts/dto/public/commands";
import { ConsignmentUuidSchema } from "@auction/contracts/dto/public/consignment/scalar";
import { ApiErrorResponseSchema, apiResponseSchema } from "@auction/contracts/dto/response";
import { decode } from "@auction/contracts/wire";
import { transportErrorCode } from "@auction/web-shared/net/transport";
import { err, ok, type Result } from "@auction/web-shared/result";
import type { GenericSchema } from "valibot";

import { sendRequest } from "./clock";
import { COMMAND_NOT_SENT, type CommandError } from "./commands";
import { observePublicSessionFailure } from "./session";

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
  "unsupported_media_type",
  "unauthenticated",
  "world_unavailable",
]);

/** 只暴露 HTTP 状态及安全的错误分类；保留合同的逐码结果。 */
export type ConsignmentPrivateError = CommandError;

/** 寄售路径和命令使用同一规范 UUID 校验。 */
export function validConsignmentUuid(value: string): boolean {
  return decode(ConsignmentUuidSchema, value).ok;
}

type Request =
  | { readonly method: "GET" }
  | { readonly method: "POST" | "PATCH"; readonly body: object };

/** 不变量 1、6、7：绑定预期身份，只发送同源请求，不暴露正文或底层异常。 */
export async function requestPrivateConsignment<T>(
  userId: string,
  url: string,
  request: Request,
  status: 200 | 202,
  schema: GenericSchema<unknown, T>,
  signal: AbortSignal,
): Promise<Result<T, ConsignmentPrivateError>> {
  if (!validConsignmentUuid(userId)) return err({ status: 400, code: "invalid_expected_user_id" });

  let dispatched = false;
  let unauthorized = false;
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
        observePublicSessionFailure(received, "unauthenticated", userId);
      }
    },
  );
  if (!dispatched) return err(COMMAND_NOT_SENT);
  if (signal.aborted) return err({ status: 0, code: "network_error" });
  if (unauthorized) return err({ status: 401, code: "unauthenticated" });
  if (!sent.ok)
    return err({
      status: sent.error.kind === "network" ? (sent.error.status ?? 0) : 0,
      code: transportErrorCode(sent.error),
    });

  const response = sent.value;
  const invalid = { status: response.status, code: "invalid_response" };
  const json = response.json({ uniqueKeys: "all" });
  if (!json.ok) return err(invalid);
  if (response.status === status) {
    const decoded = decode(apiResponseSchema(schema), json.value);
    return decoded.ok && decoded.value.code === (status === 202 ? "accepted" : "ok")
      ? ok(decoded.value.data)
      : err(invalid);
  }
  if (response.status < 400) return err(invalid);
  const decoded = decode(ApiErrorResponseSchema, json.value);
  if (!decoded.ok || decoded.value.data !== null) return err(invalid);

  const code = HTTP_ERRORS.has(decoded.value.code) ? decoded.value.code : "unknown_error";
  if (response.status === 409) observePublicSessionFailure(response.status, code, userId);
  return err({ status: response.status, code });
}
