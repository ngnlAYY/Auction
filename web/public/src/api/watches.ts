// 装备标记同源传输；身份、集合完整性和版本在边界严格校验。

import { PUBLIC_EXPECTED_USER_ID_HEADER } from "@auction/contracts/dto/public/commands";

import {
  type PublicLotWatchesResponse,
  PublicLotWatchesResponseSchema,
  type PublicSetLotWatchRequest,
  type PublicSetLotWatchResponse,
  PublicSetLotWatchResponseSchema,
} from "@auction/contracts/dto/public/watch";
import { type TransportResponse, transportErrorCode } from "@auction/web-shared/net/transport";
import { jsonHasExactKeys } from "@auction/web-shared/net/wire";
import { err, ok, type Result } from "@auction/web-shared/result";
import { validUuid } from "@auction/web-shared/ui/validation";
import { sendGet, sendRequest } from "./clock";
import type { CommandError } from "./commands";
import { boolValue, decodeErrorCode, int, object } from "./decoder";
import { decodeIdentitySuccess, observePublicSessionFailure } from "./session";
import {
  publishPublicSessionIdentity,
  startPublicSessionIdentityObservation,
} from "./session/revision";

const MAX_WATCHES = 10_000;

/** 整场集合有界且不能截断；缺项只有在完整权威读取后才表示 false/version 0。
 * 整场资源提供账号与 Cookie 代次的核对，用于交付集合和处理会话证据。 */
export async function fetchLotWatches(
  userId: string,
  auctionId: string,
  signal: AbortSignal,
  observation?: { readonly current: () => boolean },
): Promise<Result<PublicLotWatchesResponse, CommandError>> {
  const isCurrent = observation?.current ?? startPublicSessionIdentityObservation().current;
  const query = new URLSearchParams({ auction_id: auctionId }).toString();
  const response = await sendGet(`/api/v1/me/lot-watches?${query}`, signal);
  if (signal.aborted) {
    return err({ code: "network_error", status: 0 });
  }
  if (!isCurrent()) {
    return err({ code: "session_observation_stale", status: 0 });
  }
  if (!response.ok) {
    return err(failure(transportErrorCode(response.error), 0));
  }
  const status = response.value.status;
  if (!responseOk(response.value)) {
    const code = decodeErrorCode(response.value);
    // 旧代响应不能暂停新会话的授权。
    if (!isCurrent()) {
      return err({ code: "session_observation_stale", status: 0 });
    }
    return err(failure(code, status));
  }
  const page = decodeIdentitySuccess(
    response.value,
    PublicLotWatchesResponseSchema,
    (value) => {
      const envelope = object(value);
      if (envelope === null || !jsonHasExactKeys(envelope, ["user_id", "auction_id", "watches"])) {
        return false;
      }
      const watches = envelope.watches;
      return (
        Array.isArray(watches) && watches.length <= MAX_WATCHES && watches.every(validWatchValue)
      );
    },
    { integerPaths: [["data", "watches", null, "version"]] },
  );
  if (!page.ok || !validWatchCollection(page.value, page.value.user_id, auctionId)) {
    return err(failure("invalid_response", status));
  }
  if (!isCurrent()) {
    return err({ code: "session_observation_stale", status: 0 });
  }
  // 同代响应可确认 Cookie 的实际归属；集合仍绑定发起账号。
  publishPublicSessionIdentity({ kind: "user", userId: page.value.user_id });
  return page.value.user_id === userId ? page : err(failure("invalid_response", status));
}

/** 重试直接复用原 DTO，不重新生成 ID、目标状态或 CAS 版本。 */
export async function setLotWatch(
  userId: string,
  auctionId: string,
  lotId: string,
  command: PublicSetLotWatchRequest,
  signal: AbortSignal,
): Promise<Result<PublicSetLotWatchResponse, CommandError>> {
  const publish = startPublicSessionIdentityObservation();
  const response = await sendRequest(
    () =>
      ok({
        url: `/api/v1/lots/${lotId}/watch`,
        init: {
          method: "PUT",
          headers: {
            [PUBLIC_EXPECTED_USER_ID_HEADER]: userId,
            "content-type": "application/json",
          },
          body: JSON.stringify({
            command_id: command.command_id,
            watched: command.watched,
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
  const status = response.value.status;
  if (!responseOk(response.value)) {
    return err(failure(decodeErrorCode(response.value), status, userId));
  }
  const result = decodeIdentitySuccess(
    response.value,
    PublicSetLotWatchResponseSchema,
    (value) => {
      const envelope = object(value);
      return (
        envelope !== null &&
        jsonHasExactKeys(envelope, ["user_id", "auction_id", "watch"]) &&
        validWatchValue(envelope.watch)
      );
    },
    { integerPaths: [["data", "watch", "version"]] },
  );
  if (
    !result.ok ||
    result.value.user_id !== userId ||
    !validUuid(result.value.user_id) ||
    result.value.auction_id !== auctionId ||
    result.value.watch.lot_id !== lotId ||
    !validWatch(result.value.watch) ||
    result.value.watch.watched !== command.watched ||
    result.value.watch.channel !== command.channel
  ) {
    return err(failure("invalid_response", status));
  }
  publish({ kind: "user", userId: result.value.user_id });
  return result;
}

/** 缓存与 HTTP 共用校验；重复 lot、跨身份、超限和无效墓碑都失败关闭。 */
export function validWatchCollection(
  page: PublicLotWatchesResponse,
  userId: string,
  auctionId: string,
): boolean {
  const lots = new Set<string>();
  return (
    page.user_id === userId &&
    page.auction_id === auctionId &&
    validUuid(userId) &&
    validUuid(auctionId) &&
    page.watches.length <= MAX_WATCHES &&
    page.watches.every(
      (watch) => validWatch(watch) && !lots.has(watch.lot_id) && lots.add(watch.lot_id),
    )
  );
}

function validWatch(watch: PublicLotWatchesResponse["watches"][number]): boolean {
  return (
    validUuid(watch.lot_id) &&
    watch.channel === "mm" &&
    Number.isSafeInteger(watch.version) &&
    watch.version > 0
  );
}

function validWatchValue(value: unknown): boolean {
  const watch = object(value);
  // 传输层已经检查版本词法；这里仍须拒绝不安全的整数与非正版本。
  return (
    watch !== null &&
    jsonHasExactKeys(watch, ["lot_id", "watched", "channel", "version"]) &&
    boolValue(watch.watched) !== null &&
    (int(watch.version) ?? 0) > 0
  );
}

function responseOk(response: TransportResponse): boolean {
  return response.status >= 200 && response.status <= 299;
}

function failure(code: string, status: number, expectedUser?: string): CommandError {
  observePublicSessionFailure(status, code, expectedUser);
  return { code, status };
}
