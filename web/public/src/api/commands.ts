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
import {
  type TransportError,
  type TransportRequestBuilder,
  type TransportResponse,
  transportErrorCode,
} from "@auction/web-shared/net/transport";
import { err, ok, type Result } from "@auction/web-shared/result";
import { utf8ByteLength } from "@auction/web-shared/text";
import { sendRequest } from "./clock";
import { decodeErrorCode } from "./decoder";
import { decodeBidCommand, decodeCreateProxyCommand, decodeLowerProxyCommand } from "./decoder/lot";
import { observePublicSessionFailure } from "./session";
import { publicSessionIdentityRevision } from "./session/revision";
import { invalidateAfterCommand } from "./swr-cache";

/** Public 命令失败的稳定错误投影；不把服务端说明正文带入浏览器展示。 */
export interface CommandError {
  code: string;
  status: number;
}

/** 请求还未交给 fetch，不能产生结果未知。 */
export const COMMAND_NOT_SENT: CommandError = { code: "command_not_sent", status: 0 };

/** 只有本地传输边界可确认请求未发出。 */
export function commandNotSent(error: CommandError): boolean {
  return error.status === 0 && error.code === COMMAND_NOT_SENT.code;
}

/** 缺少明确业务结果时只允许重放原命令；新输入仍须经过正常表单校验。 */
export function commandErrorOutcomeUncertain(error: CommandError): boolean {
  // 浏览器会话锁失败发生在 HTTP 发送前，不把首次拒绝误报为未知外写。
  return (
    !commandNotSent(error) &&
    error.code !== "session_lock_unavailable" &&
    error.code !== "session_lock_timeout" &&
    (error.status === 0 || error.status >= 500 || error.code === "invalid_response")
  );
}

function invalidResponse(status: number): CommandError {
  return { code: "invalid_response", status };
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
  const identityAtStart = publicSessionIdentityRevision();
  let dispatched = false;
  const build: TransportRequestBuilder = (requestSignal) => {
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
  };
  let sent: Result<TransportResponse, TransportError>;
  try {
    sent = await sendRequest(build, signal, (status) => {
      // 401 一到就复核会话。正文断流或超时也要继续暂停未知命令的重试。
      if (status === 401 && !signal.aborted) {
        observePublicSessionFailure(status, "invalid_response");
      }
    });
  } catch {
    // 传输层只在构造或观察回调抛错时拒绝 Promise；命令可能已发出，按结果未知失败关闭。
    if (dispatched) {
      invalidateAfterCommand();
    }
    return err(dispatched ? invalidResponse(0) : COMMAND_NOT_SENT);
  }
  // 成功、拒绝和结果未知都要求竞价视图向服务端再确认，旧快照不能作为命令后的权威结果。
  if (!dispatched) {
    return err(COMMAND_NOT_SENT);
  }
  invalidateAfterCommand();
  if (signal.aborted) {
    return err({ code: "network_error", status: 0 });
  }
  if (!sent.ok) {
    if (sent.error.kind === "network" && sent.error.status === 401) {
      return err(invalidResponse(401));
    }
    return err({ code: transportErrorCode(sent.error), status: 0 });
  }
  const response = sent.value;
  if (!responseOk(response)) {
    return err(readCommandError(response, userId, identityAtStart));
  }
  const result = readReceipt(response);
  return result.ok ? result : err(invalidResponse(response.status));
}

function responseOk(response: { status: number }): boolean {
  return response.status >= 200 && response.status <= 299;
}

function readCommandError(
  response: TransportResponse,
  expectedUser: string,
  observedRevision: number,
): CommandError {
  const status = response.status;
  const code = decodeErrorCode(response);
  const commandError: CommandError =
    code.length > 0 && utf8ByteLength(code) <= 64 && !/[^a-z0-9_]/u.test(code)
      ? { code, status }
      : invalidResponse(status);
  // 否定预期用户必须取得完整错误正文。401 已在收到响应头时触发复核。
  if (status === 409)
    observePublicSessionFailure(status, commandError.code, expectedUser, observedRevision);
  return commandError;
}
