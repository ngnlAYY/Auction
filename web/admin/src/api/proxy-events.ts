// Admin 代理历史 API 客户端。
// 浏览器只解码后端的安全代理事件投影；游标、命令账本结果和代理状态机仍由服务端拥有。

import {
  type AdminProxyEventListResponse,
  AdminProxyEventListResponseSchema,
  type AdminProxyEventResponse,
} from "@auction/contracts/dto/admin/proxy";
import { apiResponseSchema } from "@auction/contracts/dto/response";
import { err, ok, type Result } from "@auction/web-shared/result";
import { validUuid } from "@auction/web-shared/ui/validation";

import { decodeAdminReceipt, invalidResponseError, responseError } from "./failure";
import { validate, validPageEcho } from "./pagination";
import type { AdminSessionHost } from "./session";
import { sendGet } from "./transport";
import { MAX_JSON_SAFE_INTEGER, validAdminTime, validTrimmedText } from "./validation";

/** 一条代理事件安全投影；不含代理密钥或原始请求正文。 */
export type AdminProxyEvent = AdminProxyEventResponse;
/** 一个 lot 的代理事件分页回执；页码模式下游标字段必须为空。 */
export type AdminProxyEventPage = AdminProxyEventListResponse;

/** 结构闭集由 contracts 的 `AdminProxyEventListResponse`（含 event 与混合分页，
 * `deny_unknown_fields`）负责；这里只补字段策略与跨字段谓词。 */
export const ADMIN_PROXY_EVENT_LIST_FIELD_POLICY = {
  uniqueKeys: "all",
  integerPaths: [
    ["data", "pagination", "page"],
    ["data", "pagination", "page_size"],
    ["data", "pagination", "total"],
    ["data", "proxy_events", null, "authorization_sequence"],
    ["data", "proxy_events", null, "usage_number"],
    ["data", "proxy_events", null, "maximum_amount"],
    ["data", "proxy_events", null, "state_version"],
  ],
} as const;

/** 拉取一个 lot 的代理历史页码；页码模式下游标字段必须为空。 */
export async function fetchAdminProxyEvents(
  host: AdminSessionHost,
  lotId: string,
  page: number,
  pageSize: number,
  signal: AbortSignal,
): Promise<Result<AdminProxyEventPage, string>> {
  const bounds = validate(page, pageSize, 100, 100_000);
  if (!bounds.ok) {
    return err(bounds.error);
  }
  const url = `/api/v1/admin/lots/${lotId}/proxy-events?page=${page}&page_size=${pageSize}`;
  const response = await sendGet(host, url, signal);
  if (!response.ok) {
    return err(response.error);
  }
  if (response.value.status < 200 || response.value.status > 299) {
    return err(await responseError(response.value, signal, host));
  }
  const receipt = decodeAdminReceipt(
    response.value,
    apiResponseSchema(AdminProxyEventListResponseSchema),
    ADMIN_PROXY_EVENT_LIST_FIELD_POLICY,
  );
  if (!receipt.ok) {
    return err(receipt.error);
  }
  const decoded = decodeProxyEventPage(receipt.value.data, lotId, page, pageSize);
  return decoded.ok ? decoded : err(invalidResponseError(decoded));
}

function decodeProxyEventPage(
  data: AdminProxyEventPage,
  requestedLotId: string,
  requestedPage: number,
  requestedSize: number,
): Result<AdminProxyEventPage, string> {
  const pagination = data.pagination;
  // 页码模式下游标字段必须为空，页码与总数必须显式回显。
  const page = pagination.page;
  const total = pagination.total;
  if (page == null || total == null) {
    return err("proxy_pagination");
  }
  if (
    !validUuid(requestedLotId) ||
    pagination.cursor != null ||
    pagination.next_cursor != null ||
    pagination.has_more != null ||
    !validPageEcho(
      requestedPage,
      requestedSize,
      page,
      pagination.page_size,
      total,
      data.proxy_events.length,
    )
  ) {
    return err("proxy_pagination");
  }
  const proxyEvents: AdminProxyEvent[] = [];
  for (const event of data.proxy_events) {
    const decoded = decodeProxyEvent(event, requestedLotId);
    if (!decoded.ok) {
      return err(decoded.error);
    }
    proxyEvents.push(decoded.value);
  }
  return ok({ proxy_events: proxyEvents, pagination });
}

/** 跨字段谓词保留为独立校验：usage 序号不超过授权序号、请求 lot 回显一致。 */
function decodeProxyEvent(
  event: AdminProxyEvent,
  requestedLotId: string,
): Result<AdminProxyEvent, "proxy_event"> {
  if (
    !validUuid(event.command_id) ||
    !validUuid(event.authorization_id) ||
    event.authorization_sequence < 1 ||
    event.authorization_sequence > MAX_JSON_SAFE_INTEGER ||
    event.usage_number < 1 ||
    event.usage_number > MAX_JSON_SAFE_INTEGER ||
    event.usage_number > event.authorization_sequence ||
    !validUuid(event.lot_id) ||
    event.lot_id !== requestedLotId ||
    !validUuid(event.user_id) ||
    !validTrimmedText(event.username) ||
    (event.mode !== "fast" && event.mode !== "slow") ||
    event.maximum_amount < 1 ||
    event.maximum_amount > MAX_JSON_SAFE_INTEGER ||
    (event.status !== "active" &&
      event.status !== "exhausted" &&
      event.status !== "frozen" &&
      event.status !== "closed") ||
    event.state_version < 1 ||
    event.state_version > MAX_JSON_SAFE_INTEGER ||
    !validAdminTime(event.created_at)
  ) {
    return err("proxy_event");
  }
  return ok(event);
}
