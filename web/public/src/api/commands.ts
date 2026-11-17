// Public 状态命令的 HTTP 请求。
// 浏览器只生成稳定重试所需的 `command_id` 并提交契约 DTO；金额、资格、锁和幂等结果仍由
// 服务端应用端口决定。

import type {
  PlaceBidResultResponse as BidResult,
  PublicProxyCommandAuthorizationResponse as ProxyCommandAuthorization,
} from "@auction/contracts/dto/public/account";

export type { BidResult, ProxyCommandAuthorization };

import {
  type CreateProxyAuthorizationRequest,
  type LowerProxyAuthorizationRequest,
  type PlaceBidRequest,
  PUBLIC_EXPECTED_USER_ID_HEADER,
} from "@auction/contracts/dto/public/commands";
import { type ApiErrorResponse, ApiErrorResponseSchema } from "@auction/contracts/dto/response";
import { decode } from "@auction/contracts/wire";
import {
  sendRequest,
  type TransportResponse,
  transportErrorCode,
} from "@auction/web-shared/net/transport";
import { err, ok, type Result } from "@auction/web-shared/result";
import { utf8ByteLength } from "@auction/web-shared/text";
import { decodeBidCommand, decodeCreateProxyCommand, decodeLowerProxyCommand } from "./decoder/lot";
import { observePublicSessionFailure } from "./session";
import { invalidateAfterCommand } from "./swr-cache";

/** Public 命令失败的稳定错误投影；不把服务端说明正文带入浏览器展示。 */
export interface CommandError {
  code: string;
  status: number;
}

/** 缺少明确业务结果时只允许重放原命令；新输入仍须经过正常表单校验。 */
export function commandErrorOutcomeUncertain(error: CommandError): boolean {
  // 浏览器会话锁失败发生在 HTTP 发送前，不把首次拒绝误报为未知外写。
  return (
    error.code !== "session_lock_unavailable" &&
    error.code !== "session_lock_timeout" &&
    (error.status === 0 || error.status >= 500 || error.code === "invalid_response")
  );
}

function invalidResponse(status: number): CommandError {
  return { code: "invalid_response", status };
}

function trimRustWhitespace(value: string): string {
  let start = 0;
  let end = value.length;
  while (start < end && isRustWhitespace(value.charCodeAt(start))) {
    start += 1;
  }
  while (end > start && isRustWhitespace(value.charCodeAt(end - 1))) {
    end -= 1;
  }
  return value.slice(start, end);
}

function isRustWhitespace(code: number): boolean {
  return (
    code === 0x09 ||
    code === 0x0a ||
    code === 0x0b ||
    code === 0x0c ||
    code === 0x0d ||
    code === 0x20 ||
    code === 0x85 ||
    code === 0xa0 ||
    code === 0x1680 ||
    (code >= 0x2000 && code <= 0x200a) ||
    code === 0x2028 ||
    code === 0x2029 ||
    code === 0x202f ||
    code === 0x205f ||
    code === 0x3000
  );
}

export async function placeBid(
  userId: string,
  lotId: string,
  amount: number,
  commandId: string,
  signal: AbortSignal,
): Promise<Result<BidResult, CommandError>> {
  const url = `/api/v1/lots/${lotId}/bids`;
  const command: PlaceBidRequest = { command_id: commandId, amount };
  const response = await sendRequest(
    () =>
      ok({
        url,
        init: {
          method: "POST",
          headers: {
            [PUBLIC_EXPECTED_USER_ID_HEADER]: userId,
            "content-type": "application/json",
          },
          body: JSON.stringify(command),
        },
      }),
    signal,
  );
  if (signal.aborted) {
    return err({ code: "network_error", status: 0 });
  }
  if (!response.ok) {
    return err({ code: transportErrorCode(response.error), status: 0 });
  }
  if (!responseOk(response.value)) {
    return err(await readCommandError(response.value));
  }
  const status = response.value.status;
  const result = decodeBidCommand(response.value, lotId);
  if (!result.ok) {
    return err(invalidResponse(status));
  }
  // 竞价已提交：目录价格、领先者、详情与竞价日志都可能有新权威快照，下一次读取不得命中 TTL 缓存。
  invalidateAfterCommand();
  return ok(result.value);
}

/** 创建代理授权并解码旧契约要求的完整命令投影。 */
export async function createProxyAuthorization(
  userId: string,
  lotId: string,
  maximumAmount: number,
  mode: string,
  commandId: string,
  signal: AbortSignal,
): Promise<Result<ProxyCommandAuthorization, CommandError>> {
  const url = `/api/v1/lots/${lotId}/proxy-authorizations`;
  const command: CreateProxyAuthorizationRequest = {
    command_id: commandId,
    maximum_amount: maximumAmount,
    mode,
  };
  const response = await sendRequest(
    () =>
      ok({
        url,
        init: {
          method: "POST",
          headers: {
            [PUBLIC_EXPECTED_USER_ID_HEADER]: userId,
            "content-type": "application/json",
          },
          body: JSON.stringify(command),
        },
      }),
    signal,
  );
  if (signal.aborted) {
    return err({ code: "network_error", status: 0 });
  }
  if (!response.ok) {
    return err({ code: transportErrorCode(response.error), status: 0 });
  }
  if (!responseOk(response.value)) {
    return err(await readCommandError(response.value));
  }
  const status = response.value.status;
  const result = decodeCreateProxyCommand(response.value, lotId, maximumAmount, mode);
  if (!result.ok) {
    return err(invalidResponse(status));
  }
  // 代理授权成功会改变 lot 的权威投影；公开读取缓存随之失效。
  invalidateAfterCommand();
  return ok(result.value);
}

/** 降低代理授权上限并解码旧契约要求的完整命令投影。 */
export async function lowerProxyAuthorization(
  userId: string,
  lotId: string,
  authorizationId: string,
  maximumAmount: number,
  commandId: string,
  signal: AbortSignal,
): Promise<Result<ProxyCommandAuthorization, CommandError>> {
  const url = `/api/v1/lots/${lotId}/proxy-authorizations/${authorizationId}/lower`;
  const command: LowerProxyAuthorizationRequest = {
    command_id: commandId,
    maximum_amount: maximumAmount,
  };
  const response = await sendRequest(
    () =>
      ok({
        url,
        init: {
          method: "POST",
          headers: {
            [PUBLIC_EXPECTED_USER_ID_HEADER]: userId,
            "content-type": "application/json",
          },
          body: JSON.stringify(command),
        },
      }),
    signal,
  );
  if (signal.aborted) {
    return err({ code: "network_error", status: 0 });
  }
  if (!response.ok) {
    return err({ code: transportErrorCode(response.error), status: 0 });
  }
  if (!responseOk(response.value)) {
    return err(await readCommandError(response.value));
  }
  const status = response.value.status;
  const result = decodeLowerProxyCommand(response.value, lotId, authorizationId, maximumAmount);
  if (!result.ok) {
    return err(invalidResponse(status));
  }
  // 降低上限同样改变 lot 的权威投影；公开读取缓存随之失效。
  invalidateAfterCommand();
  return ok(result.value);
}

function responseOk(response: { status: number }): boolean {
  return response.status >= 200 && response.status <= 299;
}

async function readCommandError(response: TransportResponse): Promise<CommandError> {
  const status = response.status;
  // 服务端已对命令作答：被拒绝通常正是因为价格、领先者或拍品状态已经变化。表单随后会重读
  // 权威快照并据此提示下一口价，这次重读不得命中 TTL 内的旧条目，遇到暂时性失败也不得退回
  // 命令之前的快照（否则提示会引用过期的最低价）；命令的传输失败不经过这里。
  invalidateAfterCommand();
  const payload = response.json();
  const decoded = payload.ok ? decode(ApiErrorResponseSchema, payload.value) : null;
  const error: ApiErrorResponse | null = decoded?.ok === true ? decoded.value : null;
  const commandError: CommandError =
    error !== null &&
    error.code.length > 0 &&
    utf8ByteLength(error.code) <= 64 &&
    error.code === trimRustWhitespace(error.code) &&
    /^[a-z0-9_]+$/.test(error.code)
      ? { code: error.code, status }
      : { code: "invalid_response", status };
  observePublicSessionFailure(status, commandError.code);
  return commandError;
}
