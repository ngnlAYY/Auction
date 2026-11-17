// 账号竞价警告偏好的同源传输；版本按安全整数读取，超出安全范围的响应失败关闭。

import { PUBLIC_EXPECTED_USER_ID_HEADER } from "@auction/contracts/dto/public/commands";
import type { PublicSetLotWatchPreferenceRequest } from "@auction/contracts/dto/public/watch-preference";
import {
  type PublicLotWatchPreferenceResponse,
  PublicLotWatchPreferenceResponseSchema,
} from "@auction/contracts/dto/public/watch-preference";
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
import { observePublicSessionFailure } from "./session";

const PATH = "/api/v1/me/lot-watch-preference";
/** 权威响应必须恰好包含契约字段；漏掉 `pm_available` 会让整页退化为“无法读取偏好”。 */
const RESPONSE_KEYS = ["user_id", "channel", "version", "pm_available"];

/**
 * 渠道偏好的 wire 闭集（`none`/`mm`/`pm`/`both`）与两个独立开关的互换；闭集之外返回 `None`。
 * 与后端 `LotWatchChannels` 同一张表：浏览器只做展示映射，不自行扩展取值。
 */
export function channelFlags(value: string): readonly [boolean, boolean] | null {
  switch (value) {
    case "none":
      return [false, false];
    case "mm":
      return [true, false];
    case "pm":
      return [false, true];
    case "both":
      return [true, true];
    default:
      return null;
  }
}

/** 两个独立开关对应的 wire 值；与 `channelFlags` 互逆。 */
export function channelValue(mm: boolean, pm: boolean): "none" | "mm" | "pm" | "both" {
  if (mm && pm) {
    return "both";
  }
  if (mm) {
    return "mm";
  }
  if (pm) {
    return "pm";
  }
  return "none";
}

/** 只接受当前会话身份的完整权威投影，不推导默认偏好。 */
export async function fetchLotWatchPreference(
  userId: string,
  signal: AbortSignal,
): Promise<Result<PublicLotWatchPreferenceResponse, CommandError>> {
  const response = await sendGet(PATH, signal);
  if (signal.aborted) {
    return err({ code: "network_error", status: 0 });
  }
  if (!response.ok) {
    return err(failure(transportErrorCode(response.error), 0));
  }
  return decode(response.value, userId, null);
}

/** 未知结果重试必须传入原 DTO，不能改写命令 ID、渠道或 CAS 版本。 */
export async function setLotWatchPreference(
  userId: string,
  command: PublicSetLotWatchPreferenceRequest,
  signal: AbortSignal,
): Promise<Result<PublicLotWatchPreferenceResponse, CommandError>> {
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
            channel: command.channel,
            expected_version: command.expected_version,
          }),
        },
      }),
    signal,
  );
  if (signal.aborted) {
    return err({ code: "network_error", status: 0 });
  }
  if (!response.ok) {
    return err(failure(transportErrorCode(response.error), 0));
  }
  return decode(response.value, userId, command);
}

function decode(
  response: TransportResponse,
  userId: string,
  command: PublicSetLotWatchPreferenceRequest | null,
): Result<PublicLotWatchPreferenceResponse, CommandError> {
  const status = response.status;
  if (!responseOk(response)) {
    return err(failure(decodeErrorCode(response), status));
  }
  const result = decodeSuccess(response, PublicLotWatchPreferenceResponseSchema, (value) => {
    const envelope = object(value);
    return (
      envelope !== null &&
      jsonHasExactKeys(envelope, RESPONSE_KEYS) &&
      // 已知差异：JSON.parse 已把 `1.0000000000000001` 舍入成 1，JS 无法还原原始写法；
      // 后端只输出整数版本，这里仍按安全整数失败关闭。
      (int(envelope.version) ?? 0) > 0
    );
  });
  if (
    !result.ok ||
    result.value.user_id !== userId ||
    !validUuid(result.value.user_id) ||
    channelFlags(result.value.channel) === null ||
    (command !== null &&
      (result.value.channel !== command.channel ||
        result.value.version <= command.expected_version))
  ) {
    return err(failure("invalid_response", status));
  }
  return ok(result.value);
}

function responseOk(response: TransportResponse): boolean {
  return response.status >= 200 && response.status <= 299;
}

function failure(code: string, status: number): CommandError {
  observePublicSessionFailure(status, code);
  return { code, status };
}
