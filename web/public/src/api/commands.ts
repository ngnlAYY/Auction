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
import { type TransportResponse, transportErrorCode } from "@auction/web-shared/net/transport";
import { err, ok, type Result } from "@auction/web-shared/result";
import { utf8ByteLength } from "@auction/web-shared/text";
import { sendRequest } from "./clock";
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

/** 提交同一份普通出价命令并校验服务端回执。 */
export function placeBid(
  userId: string,
  lotId: string,
  amount: number,
  commandId: string,
  signal: AbortSignal,
): Promise<Result<BidResult, CommandError>> {
  const command: PlaceBidRequest = { command_id: commandId, amount };
  return sendCompetitionCommand(
    `/api/v1/lots/${lotId}/bids`,
    userId,
    command,
    (response) => decodeBidCommand(response, lotId),
    signal,
  );
}

/** 创建代理授权并解码旧契约要求的完整命令投影。 */
export function createProxyAuthorization(
  userId: string,
  lotId: string,
  maximumAmount: number,
  mode: string,
  commandId: string,
  signal: AbortSignal,
): Promise<Result<ProxyCommandAuthorization, CommandError>> {
  const command: CreateProxyAuthorizationRequest = {
    command_id: commandId,
    maximum_amount: maximumAmount,
    mode,
  };
  return sendCompetitionCommand(
    `/api/v1/lots/${lotId}/proxy-authorizations`,
    userId,
    command,
    (response) => decodeCreateProxyCommand(response, lotId, maximumAmount, mode),
    signal,
  );
}

/** 降低代理授权上限并解码旧契约要求的完整命令投影。 */
export function lowerProxyAuthorization(
  userId: string,
  lotId: string,
  authorizationId: string,
  maximumAmount: number,
  commandId: string,
  signal: AbortSignal,
): Promise<Result<ProxyCommandAuthorization, CommandError>> {
  const command: LowerProxyAuthorizationRequest = {
    command_id: commandId,
    maximum_amount: maximumAmount,
  };
  return sendCompetitionCommand(
    `/api/v1/lots/${lotId}/proxy-authorizations/${authorizationId}/lower`,
    userId,
    command,
    (response) => decodeLowerProxyCommand(response, lotId, authorizationId, maximumAmount),
    signal,
  );
}

async function sendCompetitionCommand<T>(
  url: string,
  userId: string,
  command: PlaceBidRequest | CreateProxyAuthorizationRequest | LowerProxyAuthorizationRequest,
  readReceipt: (response: TransportResponse) => Result<T, string>,
  signal: AbortSignal,
): Promise<Result<T, CommandError>> {
  let dispatched = false;
  const sent = await sendRequest((requestSignal) => {
    if (requestSignal.aborted) {
      return err({ kind: "network", status: null });
    }
    const body = JSON.stringify(command);
    // 共享传输在构造成功后同步调用 fetch，之后断流、取消或超时无法确认命令未到达。
    dispatched = true;
    return ok({
      url,
      init: {
        method: "POST",
        headers: {
          [PUBLIC_EXPECTED_USER_ID_HEADER]: userId,
          "content-type": "application/json",
        },
        body,
      },
    });
  }, signal);
  // 成功、拒绝和结果未知都要求竞价视图向服务端再确认，旧快照不能作为命令后的权威结果。
  if (dispatched) {
    invalidateAfterCommand();
  }
  if (signal.aborted) {
    return err({ code: "network_error", status: 0 });
  }
  if (!sent.ok) {
    return err({ code: transportErrorCode(sent.error), status: 0 });
  }
  const response = sent.value;
  if (!responseOk(response)) {
    return err(readCommandError(response));
  }
  const result = readReceipt(response);
  return result.ok ? result : err(invalidResponse(response.status));
}

function responseOk(response: { status: number }): boolean {
  return response.status >= 200 && response.status <= 299;
}

function readCommandError(response: TransportResponse): CommandError {
  const status = response.status;
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
