// 账号登录时长偏好的同源传输与严格解码；版本按安全整数读取，超出安全范围的响应失败关闭。

import {
  type PublicSessionPreferenceResponse,
  PublicSessionPreferenceResponseSchema,
  type PublicSetSessionPreferenceRequest,
} from "@auction/contracts/dto/public/account";
import { PUBLIC_EXPECTED_USER_ID_HEADER } from "@auction/contracts/dto/public/commands";
import {
  sendGet,
  sendRequest,
  type TransportResponse,
  transportErrorCode,
} from "@auction/web-shared/net/transport";
import { jsonHasExactKeys } from "@auction/web-shared/net/wire";
import { err, ok, type Result } from "@auction/web-shared/result";
import { validUuid } from "@auction/web-shared/ui/validation";

import type { CommandError } from "./commands";
import { decodeErrorCode, decodeSuccess, int, object } from "./decoder";
import {
  invalidateCookieSync,
  notifyPublicSessionChange,
  observePublicSessionFailure,
  parseStrictRfc3339,
} from "./session";
import { lock as lockCookie } from "./session/cookie";

const PATH = "/api/v1/me/session-preference";

/** 永久不设固定截止；有限偏好限制为整分钟，范围为一分钟到 365 天。 */
export function validSessionDuration(durationSeconds: number | null): boolean {
  return (
    durationSeconds === null ||
    (Number.isSafeInteger(durationSeconds) &&
      durationSeconds >= 60 &&
      durationSeconds <= 31_536_000 &&
      durationSeconds % 60 === 0)
  );
}

/** 读取账号偏好和当前 Cookie 的实际截止；两者允许因旧会话或其他设备保存而不同。 */
export async function fetchSessionPreference(
  userId: string,
  signal: AbortSignal,
): Promise<Result<PublicSessionPreferenceResponse, CommandError>> {
  const response = await sendGet(PATH, signal);
  if (signal.aborted) {
    return err({ code: "network_error", status: 0 });
  }
  if (!response.ok) {
    return err(failure(transportErrorCode(response.error), 0));
  }
  return decode(response.value, userId, null);
}

/** 未知结果只重发原始命令及 CAS 版本；响应成功后通过外壳重读身份，不伪造登录结果。 */
export async function setSessionPreference(
  userId: string,
  command: PublicSetSessionPreferenceRequest,
  signal: AbortSignal,
): Promise<Result<PublicSessionPreferenceResponse, CommandError>> {
  // 成功、重放和未知结果的重发都须与登录共用原生锁，防止迟到 Set-Cookie 换回旧身份。
  return lockCookie(
    async (lockSignal) => {
      invalidateCookieSync();
      const response = await sendRequest(
        () =>
          ok({
            url: PATH,
            init: {
              method: "PUT",
              headers: {
                [PUBLIC_EXPECTED_USER_ID_HEADER]: userId,
                "content-type": "application/json",
              },
              body: JSON.stringify({
                command_id: command.command_id,
                duration_seconds: command.duration_seconds,
                expected_version: command.expected_version,
              }),
            },
          }),
        lockSignal,
      );
      if (lockSignal.aborted) {
        return err({ code: "network_error", status: 0 });
      }
      if (!response.ok) {
        return err(failure(transportErrorCode(response.error), 0));
      }
      if (responseOk(response.value)) {
        // 即使正文损坏，Cookie 和服务端截止也可能已经更新，仍须通知本页及其他标签页。
        notifyPublicSessionChange("refresh");
      }
      return decode(response.value, userId, command);
    },
    (code) => failure(code, 0),
    signal,
  );
}

function decode(
  response: TransportResponse,
  userId: string,
  command: PublicSetSessionPreferenceRequest | null,
): Result<PublicSessionPreferenceResponse, CommandError> {
  const status = response.status;
  if (!responseOk(response)) {
    return err(failure(decodeErrorCode(response), status));
  }
  const result = decodeSuccess(response, PublicSessionPreferenceResponseSchema, (value) => {
    const envelope = object(value);
    return (
      envelope !== null &&
      jsonHasExactKeys(envelope, ["user_id", "duration_seconds", "version", "expires_at"]) &&
      // 已知差异：JSON.parse 已把 `1.0000000000000001` 舍入成 1，JS 无法还原原始写法；
      // 后端只输出整数版本，这里仍按安全整数失败关闭。
      (int(envelope.version) ?? 0) > 0 &&
      (envelope.duration_seconds === null || int(envelope.duration_seconds) !== null)
    );
  });
  if (
    !result.ok ||
    result.value.user_id !== userId ||
    !validUuid(result.value.user_id) ||
    !validSessionDuration(result.value.duration_seconds) ||
    (result.value.expires_at !== null && parseStrictRfc3339(result.value.expires_at) === null) ||
    (command !== null && !matchesCommand(result.value, command))
  ) {
    return err(failure("invalid_response", status));
  }
  return ok(result.value);
}

function matchesCommand(
  result: PublicSessionPreferenceResponse,
  command: PublicSetSessionPreferenceRequest,
): boolean {
  const expectedVersion = Number.isSafeInteger(command.expected_version + 1)
    ? command.expected_version + 1
    : null;
  return (
    result.duration_seconds === command.duration_seconds &&
    expectedVersion !== null &&
    result.version === expectedVersion &&
    (result.expires_at === null) === (command.duration_seconds === null)
  );
}

function responseOk(response: TransportResponse): boolean {
  return response.status >= 200 && response.status <= 299;
}

function failure(code: string, status: number): CommandError {
  observePublicSessionFailure(status, code);
  return { code, status };
}
