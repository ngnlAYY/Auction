// Admin 论坛状态只读请求；与 Public 共用 contracts DTO，不依赖 Public Web。

import {
  type PublicForumResponse,
  PublicForumResponseSchema,
  type PublicForumSyncStatusResponse,
  PublicForumSyncStatusResponseSchema,
} from "@auction/contracts/dto/public/catalog/forum";
import { apiResponseSchema } from "@auction/contracts/dto/response";
import type { TransportResponse } from "@auction/web-shared/net/transport";
import type { JsonFieldPolicy } from "@auction/web-shared/net/wire";
import { err, ok, type Result } from "@auction/web-shared/result";
import { utf8ByteLength } from "@auction/web-shared/text";
import { validUuid } from "@auction/web-shared/ui/validation";

import { decodeAdminReceipt, invalidResponseError, responseError } from "./failure";
import type { AdminSessionHost } from "./session";
import { sendGet } from "./transport";
import { validAdminTime, validSuccessCode } from "./validation";

/** 论坛回执用于决定页面能否继续同步，按整数词法并拒绝重复键。 */
const ADMIN_FORUM_RESPONSE_FIELD_POLICY: JsonFieldPolicy = {
  uniqueKeys: "all",
  numbers: "integers",
};

/** 同步启用状态与当前拍卖的安全投影；没有镜像不等于同步失败。 */
export interface AdminForumStatus {
  readonly enabled: boolean;
  readonly thread: PublicForumResponse | null;
}

/** 只重读本地状态，不创建恢复命令，不触发远端 POST。 */
export async function fetchAdminForumStatus(
  host: AdminSessionHost,
  auctionId: string,
  signal: AbortSignal,
): Promise<Result<AdminForumStatus, string>> {
  if (!validUuid(auctionId)) {
    return err("invalid_request");
  }
  const syncResponse = await sendGet(host, "/api/v1/admin/forum/sync-status", signal);
  if (!syncResponse.ok) {
    return err(syncResponse.error);
  }
  if (!responseOk(syncResponse.value)) {
    return err(await responseError(syncResponse.value, signal, host));
  }
  const enabled = decodeSyncStatus(syncResponse.value);
  if (!enabled.ok) {
    return err(enabled.error);
  }
  if (!enabled.value) {
    return ok({ enabled: false, thread: null });
  }

  const threadResponse = await sendGet(host, `/api/v1/admin/auctions/${auctionId}/forum`, signal);
  if (!threadResponse.ok) {
    return err(threadResponse.error);
  }
  if (threadResponse.value.status === 404) {
    return ok({ enabled: true, thread: null });
  }
  if (!responseOk(threadResponse.value)) {
    return err(await responseError(threadResponse.value, signal, host));
  }
  const thread = decodeThread(threadResponse.value, auctionId);
  if (!thread.ok) {
    return err(thread.error);
  }
  return ok({ enabled: true, thread: thread.value });
}

function decodeSyncStatus(response: TransportResponse): Result<boolean, string> {
  const payload = decodeAdminReceipt(
    response,
    apiResponseSchema(PublicForumSyncStatusResponseSchema),
    ADMIN_FORUM_RESPONSE_FIELD_POLICY,
  );
  if (!payload.ok) {
    return err(payload.error);
  }
  const status: PublicForumSyncStatusResponse = payload.value.data;
  if (!validSuccessCode(payload.value.code)) {
    return err(invalidResponseError("forum_sync_status"));
  }
  return ok(status.enabled);
}

function decodeThread(
  response: TransportResponse,
  auctionId: string,
): Result<PublicForumResponse, string> {
  const payload = decodeAdminReceipt(
    response,
    apiResponseSchema(PublicForumResponseSchema),
    ADMIN_FORUM_RESPONSE_FIELD_POLICY,
  );
  if (!payload.ok) {
    return err(payload.error);
  }
  const thread = payload.value.data;
  const contentSyncedAt = thread.content_synced_at ?? null;
  const contentLastSuccessAt = thread.content_last_success_at ?? null;
  const contentLastErrorAt = thread.content_last_error_at ?? null;
  const contentLastErrorCode = thread.content_last_error_code ?? null;
  const threadUrl = thread.thread_url ?? null;
  const title = thread.title ?? null;
  const body = thread.body ?? null;
  if (
    !validSuccessCode(payload.value.code) ||
    thread.auction_id !== auctionId ||
    !(
      thread.status === "pending" ||
      thread.status === "publishing" ||
      thread.status === "published" ||
      thread.status === "failed"
    ) ||
    thread.source_revision < 0 ||
    thread.content_revision < 0 ||
    thread.processed_through_post_number < 2 ||
    !validAdminTime(thread.updated_at) ||
    ![contentSyncedAt, contentLastSuccessAt, contentLastErrorAt].every(
      (value) => value === null || validAdminTime(value),
    ) ||
    (contentLastErrorAt === null) !== (contentLastErrorCode === null) ||
    (contentLastErrorCode !== null && !validForumErrorCode(contentLastErrorCode)) ||
    (threadUrl !== null && !validForumUrl(threadUrl)) ||
    (thread.status === "published" &&
      (thread.content_revision === 0 ||
        contentSyncedAt === null ||
        threadUrl === null ||
        title === null ||
        title === "" ||
        body === null ||
        body === ""))
  ) {
    return err(invalidResponseError("forum_thread"));
  }
  return ok(thread);
}

function validForumErrorCode(code: string): boolean {
  return code !== "" && utf8ByteLength(code) <= 64 && /^[a-z_]*$/u.test(code);
}

function validForumUrl(value: string): boolean {
  try {
    const url = new URL(value);
    return (
      url.protocol === "https:" && url.hostname !== "" && url.username === "" && url.password === ""
    );
  } catch {
    return false;
  }
}

/** gloo 的 `Response::ok`：状态码 200 到 299。 */
function responseOk(response: { readonly status: number }): boolean {
  return response.status >= 200 && response.status <= 299;
}
