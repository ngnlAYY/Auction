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
import { startPublicSessionIdentityObservation } from "./session/revision";

const MAX_WATCHES = 10_000;

/** 整场集合有界且不能截断；缺项只有在完整权威读取后才表示 false/version 0。 */
export async function fetchLotWatches(
  userId: string,
  auctionId: string,
  signal: AbortSignal,
): Promise<Result<PublicLotWatchesResponse, CommandError>> {
  const query = new URLSearchParams({ auction_id: auctionId }).toString();
  const publish = startPublicSessionIdentityObservation();
  const response = await sendGet(`/api/v1/me/lot-watches?${query}`, signal);
  if (signal.aborted) {
    return err({ code: "network_error", status: 0 });
  }
  if (!response.ok) {
    return err(failure(transportErrorCode(response.error), 0));
  }
  const status = response.value.status;
  if (!responseOk(response.value)) {
    return err(failure(decodeErrorCode(response.value), status));
  }
  const page = decodeIdentitySuccess(response.value, PublicLotWatchesResponseSchema, (value) => {
    const envelope = object(value);
    if (envelope === null || !jsonHasExactKeys(envelope, ["user_id", "auction_id", "watches"])) {
      return false;
    }
    const watches = envelope.watches;
    return (
      Array.isArray(watches) && watches.length <= MAX_WATCHES && watches.every(validWatchValue)
    );
  });
  if (!page.ok || !validWatchCollection(page.value, page.value.user_id, auctionId)) {
    return err(failure("invalid_response", status));
  }
  // 读取用服务端投影确认 Cookie 归属；集合仍只能交给请求绑定的账号。
  publish({ kind: "user", userId: page.value.user_id });
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
  const result = decodeIdentitySuccess(response.value, PublicSetLotWatchResponseSchema, (value) => {
    const envelope = object(value);
    return (
      envelope !== null &&
      jsonHasExactKeys(envelope, ["user_id", "auction_id", "watch"]) &&
      validWatchValue(envelope.watch)
    );
  });
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
  // 已知差异：JSON.parse 会把 `1.0000000000000001` 舍入成 1，JS 无法还原原始写法；
  // 后端只输出整数版本。TS 只能安全读取安全整数；超出范围的 version 与源码的 i64 分支同样失败关闭。
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
