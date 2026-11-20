// Admin 实时事件、积压、失败与重放接口。
//
// 浏览器只接收事件元数据与 lag/failure 的安全投影；replay 的公开 payload 由管理操作员
// 显式输入，并由服务端事件注册表、命令账本和数据库事务做最终校验。

import type {
  AdminRealtimeEventListResponse,
  AdminRealtimeFailureListResponse,
  AdminRealtimeFailureResponse,
  AdminRealtimeLagListResponse,
} from "@auction/contracts/dto/admin/realtime";
import { err, ok, type Result } from "@auction/web-shared/result";

import { invalidResponseError, responseError } from "../failure";
import { validate as validatePagination } from "../pagination";
import type { AdminSessionHost } from "../session";
import { trimRustWhitespace } from "../session-time";
import { sendGet } from "../transport";
import {
  decodeRealtimeEventPage,
  decodeRealtimeFailurePage,
  decodeRealtimeLagPage,
} from "./decode";

/** 实时事件页；对应 Rust 的 `RealtimeEventPage` 别名。 */
export type RealtimeEventPage = AdminRealtimeEventListResponse;
/** 实时积压页；对应 Rust 的 `RealtimeLagPage` 别名。 */
export type RealtimeLagPage = AdminRealtimeLagListResponse;
/** 实时失败页；对应 Rust 的 `RealtimeFailurePage` 别名。 */
export type RealtimeFailurePage = AdminRealtimeFailureListResponse;
/** 单条实时失败；对应 Rust 的 `RealtimeFailure` 别名。 */
export type RealtimeFailure = AdminRealtimeFailureResponse;

export {
  type PreparedRealtimeReplay,
  prepareRealtimeReplay,
  replayRealtimeFailure,
} from "./replay";

/** 读取按事件流与类型过滤的实时事件页。 */
export function fetchRealtimeEventPage(
  host: AdminSessionHost,
  page: number,
  streamKey: string,
  kind: string,
  pageSize: number,
  signal: AbortSignal,
): Promise<Result<RealtimeEventPage, string>> {
  return fetchRealtimeEventPageWithSize(host, page, pageSize, streamKey, kind, signal);
}

/** 只取全局事件总数，避免概览卡片被列表筛选条件改写。 */
export async function fetchRealtimeEventTotal(
  host: AdminSessionHost,
  signal: AbortSignal,
): Promise<Result<number, string>> {
  const page = await fetchRealtimeEventPageWithSize(host, 1, 1, "", "", signal);
  return page.ok ? ok(page.value.pagination.total) : err(page.error);
}

async function fetchRealtimeEventPageWithSize(
  host: AdminSessionHost,
  page: number,
  pageSize: number,
  streamKey: string,
  kind: string,
  signal: AbortSignal,
): Promise<Result<RealtimeEventPage, string>> {
  const pagination = validatePagination(page, pageSize, 200, 1_000_000);
  if (!pagination.ok) {
    return err(pagination.error);
  }
  const requestedPage = page;
  const requestedPageSize = Math.max(pageSize, 1);
  const query = new URLSearchParams();
  query.set("page", String(requestedPage));
  query.set("page_size", String(requestedPageSize));
  appendFilter(query, "stream_key", streamKey);
  appendFilter(query, "kind", kind);
  const response = await sendGet(host, `/api/v1/admin/realtime/events?${query.toString()}`, signal);
  if (!response.ok) {
    return err(response.error);
  }
  if (response.value.status < 200 || response.value.status >= 300) {
    return err(await responseError(response.value, signal, host));
  }
  const decoded = decodeRealtimeEventPage(
    response.value,
    requestedPage,
    requestedPageSize,
    trimRustWhitespace(streamKey),
    trimRustWhitespace(kind),
  );
  return decoded.ok ? ok(decoded.value) : err(decodeFailureCode(decoded.error));
}

/** 读取实时积压页，可按消费者前缀过滤。 */
export async function fetchRealtimeLagPage(
  host: AdminSessionHost,
  page: number,
  streamPrefix: string,
  consumerKey: string,
  pageSize: number,
  signal: AbortSignal,
): Promise<Result<RealtimeLagPage, string>> {
  const pagination = validatePagination(page, pageSize, 200, 1_000_000);
  if (!pagination.ok) {
    return err(pagination.error);
  }
  const query = new URLSearchParams();
  query.set("page", String(Math.max(page, 1)));
  query.set("page_size", String(Math.max(pageSize, 1)));
  appendFilter(query, "stream_prefix", streamPrefix);
  appendFilter(query, "consumer_key", consumerKey);
  const response = await sendGet(host, `/api/v1/admin/realtime/lag?${query.toString()}`, signal);
  if (!response.ok) {
    return err(response.error);
  }
  if (response.value.status < 200 || response.value.status >= 300) {
    return err(await responseError(response.value, signal, host));
  }
  const decoded = decodeRealtimeLagPage(
    response.value,
    Math.max(page, 1),
    Math.max(pageSize, 1),
    trimRustWhitespace(streamPrefix),
    trimRustWhitespace(consumerKey),
  );
  return decoded.ok ? ok(decoded.value) : err(decodeFailureCode(decoded.error));
}

/** 读取实时失败页。 */
export async function fetchRealtimeFailurePage(
  host: AdminSessionHost,
  page: number,
  streamKey: string,
  consumerKey: string,
  code: string,
  state: string,
  pageSize: number,
  signal: AbortSignal,
): Promise<Result<RealtimeFailurePage, string>> {
  const pagination = validatePagination(page, pageSize, 200, 1_000_000);
  if (!pagination.ok) {
    return err(pagination.error);
  }
  const requestedPage = Math.max(page, 1);
  const requestedPageSize = Math.max(pageSize, 1);
  const query = new URLSearchParams();
  query.set("page", String(requestedPage));
  query.set("page_size", String(requestedPageSize));
  appendFilter(query, "stream_key", streamKey);
  appendFilter(query, "consumer_key", consumerKey);
  appendFilter(query, "code", code);
  const resolved = resolvedFromLegacyState(state);
  if (resolved !== null) {
    query.set("resolved", resolved ? "true" : "false");
  }
  const response = await sendGet(
    host,
    `/api/v1/admin/realtime/failures?${query.toString()}`,
    signal,
  );
  if (!response.ok) {
    return err(response.error);
  }
  if (response.value.status < 200 || response.value.status >= 300) {
    return err(await responseError(response.value, signal, host));
  }
  const decoded = decodeRealtimeFailurePage(
    response.value,
    requestedPage,
    requestedPageSize,
    trimRustWhitespace(streamKey),
    trimRustWhitespace(consumerKey),
    trimRustWhitespace(code),
    resolved,
  );
  return decoded.ok ? ok(decoded.value) : err(decodeFailureCode(decoded.error));
}

/** 将旧版 Select 的状态 token 转换为后端的布尔筛选；未知值安全地视为未筛选。 */
function resolvedFromLegacyState(state: string): boolean | null {
  if (state === "dead_letter") {
    return false;
  }
  if (state === "completed") {
    return true;
  }
  return null;
}

/** `decodeAdminReceipt` 已上报并返回 `invalid_response`；组合校验失败在这里补报一次。 */
function decodeFailureCode(error: string): string {
  return error === "invalid_response" ? error : invalidResponseError(error);
}

function appendFilter(query: URLSearchParams, key: string, value: string): void {
  const trimmed = trimRustWhitespace(value);
  if (trimmed !== "") {
    query.append(key, trimmed);
  }
}
