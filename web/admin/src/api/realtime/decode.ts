// Admin realtime 响应的严格解码；响应允许兼容扩展，但旧必需字段与组合语义必须全部成立。
// 旧 realtime decoder 允许响应增加字段。contracts 的 struct 已要求全部旧字段显式存在，
// 因此不另建 schema 镜像；`resolved_at` 缺失仍按旧 wire 兼容读作 null。
// 同步义务：上述 DTO 字段集变化时必须重新核对下方每条组合校验。

import {
  type AdminRealtimeEventListResponse,
  AdminRealtimeEventListResponseSchema,
  type AdminRealtimeFailureListResponse,
  AdminRealtimeFailureListResponseSchema,
  type AdminRealtimeFailureResponse,
  type AdminRealtimeLagListResponse,
  AdminRealtimeLagListResponseSchema,
  type AdminRealtimePaginationResponse,
  type AdminRealtimeReplayEventResponse,
  AdminRealtimeReplayResponseSchema,
} from "@auction/contracts/dto/admin/realtime";
import { apiResponseSchema } from "@auction/contracts/dto/response";
import type { TransportResponse } from "@auction/web-shared/net/transport";
import type { JsonFieldPolicy } from "@auction/web-shared/net/wire";
import { err, ok, type Result } from "@auction/web-shared/result";
import { validUuid } from "@auction/web-shared/ui/validation";

import { decodeAdminReceipt, invalidResponseError } from "../failure";
import { validPageEcho, validResponse } from "../pagination";
import {
  MAX_JSON_SAFE_INTEGER,
  validAdminTime,
  validCount,
  validSafeText,
  validTrimmedText,
} from "../validation";

/** 事件列表按整数词法解析固定字段，并拒绝任意层重复键（Rust serde 会拒绝重复字段）；未知扩展字段原样留给页面。 */
const EVENT_PAGE_FIELD_POLICY: JsonFieldPolicy = {
  uniqueKeys: "all",
  integerPaths: [
    ["data", "pagination", "page"],
    ["data", "pagination", "page_size"],
    ["data", "pagination", "total"],
    ["data", "events", null, "stream_sequence"],
    ["data", "events", null, "schema_version"],
  ],
};

const LAG_PAGE_FIELD_POLICY: JsonFieldPolicy = {
  uniqueKeys: "all",
  integerPaths: [
    ["data", "pagination", "page"],
    ["data", "pagination", "page_size"],
    ["data", "pagination", "total"],
    ["data", "lags", null, "offset"],
    ["data", "lags", null, "latest_sequence"],
    ["data", "lags", null, "lag"],
  ],
};

/** 失败列表决定重放按钮是否可用；重复键会让 Rust 的 `StrictFailure` 拒绝整页。 */
const FAILURE_PAGE_FIELD_POLICY: JsonFieldPolicy = {
  uniqueKeys: "all",
  integerPaths: [
    ["data", "pagination", "page"],
    ["data", "pagination", "page_size"],
    ["data", "pagination", "total"],
  ],
};

/** 重放回执决定命令结果；回执没有浮点字段，按 G1 拒绝重复键与整份非整数词法。 */
const REPLAY_RECEIPT_FIELD_POLICY: JsonFieldPolicy = {
  uniqueKeys: "all",
  numbers: "integers",
};

const I64_MAX = 9_223_372_036_854_775_807n;

/** 解码一页实时事件；分页回显、事件字段与筛选回显必须同时成立。 */
export function decodeRealtimeEventPage(
  response: TransportResponse,
  requestedPage: number,
  requestedPageSize: number,
  streamKey: string,
  kind: string,
): Result<AdminRealtimeEventListResponse, string> {
  const receipt = decodeAdminReceipt(
    response,
    apiResponseSchema(AdminRealtimeEventListResponseSchema),
    EVENT_PAGE_FIELD_POLICY,
  );
  if (!receipt.ok) {
    return err(receipt.error);
  }
  const pagination = decodePagination(receipt.value.data.pagination);
  if (!pagination.ok) {
    return err(pagination.error);
  }
  const events = receipt.value.data.events;
  if (
    !validPageEcho(
      requestedPage,
      requestedPageSize,
      pagination.value.page,
      pagination.value.page_size,
      pagination.value.total,
      events.length,
    )
  ) {
    return err("realtime_event_page");
  }
  const decoded: AdminRealtimeEventListResponse["events"] = [];
  for (const candidate of events) {
    const event = decodeEvent(candidate);
    if (!event.ok) {
      return err(event.error);
    }
    decoded.push(event.value);
  }
  if (
    decoded.some(
      (event) =>
        (streamKey !== "" && event.stream_key !== streamKey) ||
        (kind !== "" && event.kind !== kind),
    )
  ) {
    return err("realtime_event_filter");
  }
  return ok({ events: decoded, pagination: pagination.value });
}

function decodeEvent(
  value: AdminRealtimeEventListResponse["events"][number],
): Result<AdminRealtimeEventListResponse["events"][number], string> {
  if (
    !validUuid(value.event_id) ||
    !validSafeText(value.stream_key, 256) ||
    !validCount(value.stream_sequence) ||
    value.stream_sequence === 0 ||
    !validSafeText(value.kind, 128) ||
    value.schema_version <= 0 ||
    !validAdminTime(value.created_at)
  ) {
    return err("realtime_event");
  }
  return ok({
    event_id: value.event_id,
    stream_key: value.stream_key,
    stream_sequence: value.stream_sequence,
    kind: value.kind,
    schema_version: value.schema_version,
    created_at: value.created_at,
  });
}

/** 解码一页实时积压；消费者按精确匹配、流按前缀回显筛选条件。 */
export function decodeRealtimeLagPage(
  response: TransportResponse,
  requestedPage: number,
  requestedPageSize: number,
  streamPrefix: string,
  consumerKey: string,
): Result<AdminRealtimeLagListResponse, string> {
  const receipt = decodeAdminReceipt(
    response,
    apiResponseSchema(AdminRealtimeLagListResponseSchema),
    LAG_PAGE_FIELD_POLICY,
  );
  if (!receipt.ok) {
    return err(receipt.error);
  }
  const pagination = decodePagination(receipt.value.data.pagination);
  if (!pagination.ok) {
    return err(pagination.error);
  }
  const lags = receipt.value.data.lags;
  if (
    !validPageEcho(
      requestedPage,
      requestedPageSize,
      pagination.value.page,
      pagination.value.page_size,
      pagination.value.total,
      lags.length,
    )
  ) {
    return err("realtime_lag_page");
  }
  const decoded: AdminRealtimeLagListResponse["lags"] = [];
  for (const candidate of lags) {
    const lag = decodeLag(candidate);
    if (!lag.ok) {
      return err(lag.error);
    }
    decoded.push(lag.value);
  }
  if (
    decoded.some(
      (lag) =>
        (streamPrefix !== "" && !lag.stream_key.startsWith(streamPrefix)) ||
        (consumerKey !== "" && lag.consumer_key !== consumerKey),
    )
  ) {
    return err("realtime_lag_filter");
  }
  return ok({ lags: decoded, pagination: pagination.value });
}

function decodeLag(
  value: AdminRealtimeLagListResponse["lags"][number],
): Result<AdminRealtimeLagListResponse["lags"][number], string> {
  if (
    !validTrimmedText(value.consumer_key) ||
    !validTrimmedText(value.stream_key) ||
    !validCount(value.offset) ||
    !validCount(value.latest_sequence) ||
    !validCount(value.lag)
  ) {
    return err("realtime_lag");
  }
  return ok({
    stream_key: value.stream_key,
    consumer_key: value.consumer_key,
    offset: value.offset,
    latest_sequence: value.latest_sequence,
    lag: value.lag,
  });
}

/** 解码一页实时失败；`resolved` 与 `resolved_at` 必须互相一致。 */
export function decodeRealtimeFailurePage(
  response: TransportResponse,
  requestedPage: number,
  requestedPageSize: number,
  streamKey: string,
  consumerKey: string,
  code: string,
  resolved: boolean | null,
): Result<AdminRealtimeFailureListResponse, string> {
  const receipt = decodeAdminReceipt(
    response,
    apiResponseSchema(AdminRealtimeFailureListResponseSchema),
    FAILURE_PAGE_FIELD_POLICY,
  );
  if (!receipt.ok) {
    return err(receipt.error);
  }
  const pagination = receipt.value.data.pagination;
  const failures = receipt.value.data.failures;
  if (
    !validPageEcho(
      requestedPage,
      requestedPageSize,
      pagination.page,
      pagination.page_size,
      pagination.total,
      failures.length,
    )
  ) {
    return err("realtime_failure_page");
  }
  const end = BigInt(pagination.page) * BigInt(pagination.page_size);
  const hasMore = end <= I64_MAX && end < BigInt(pagination.total);
  if (pagination.has_more !== hasMore) {
    return err("realtime_failure_page");
  }
  const decoded: AdminRealtimeFailureListResponse["failures"] = [];
  for (const candidate of failures) {
    const failure = decodeFailure(candidate);
    if (!failure.ok) {
      return err(failure.error);
    }
    decoded.push(failure.value);
  }
  if (
    decoded.some(
      (failure) =>
        (streamKey !== "" && failure.stream_key !== streamKey) ||
        (consumerKey !== "" && failure.consumer_key !== consumerKey) ||
        (code !== "" && failure.code !== code) ||
        (resolved !== null && failure.resolved !== resolved),
    )
  ) {
    return err("realtime_failure_filter");
  }
  return ok({
    failures: decoded,
    pagination: {
      page: pagination.page,
      page_size: pagination.page_size,
      has_more: pagination.has_more,
      total: pagination.total,
    },
  });
}

function decodeFailure(
  value: AdminRealtimeFailureResponse,
): Result<AdminRealtimeFailureResponse, string> {
  const resolvedAt = value.resolved_at ?? null;
  if (
    !validUuid(value.id) ||
    !validUuid(value.event_id) ||
    !validTrimmedText(value.stream_key) ||
    !validTrimmedText(value.consumer_key) ||
    !validTrimmedText(value.code) ||
    !validSafeText(value.safe_detail, 2_048) ||
    !validAdminTime(value.failed_at) ||
    (resolvedAt !== null && !validAdminTime(resolvedAt)) ||
    value.resolved !== (resolvedAt !== null)
  ) {
    return err("realtime_failure");
  }
  return ok({
    id: value.id,
    event_id: value.event_id,
    stream_key: value.stream_key,
    consumer_key: value.consumer_key,
    code: value.code,
    safe_detail: value.safe_detail,
    failed_at: value.failed_at,
    resolved_at: resolvedAt,
    resolved: value.resolved,
  });
}

function decodePagination(
  value: AdminRealtimePaginationResponse,
): Result<AdminRealtimePaginationResponse, string> {
  if (!validResponse(value.page, value.page_size, value.total)) {
    return err("realtime_pagination");
  }
  return ok(value);
}

/** 解码重放成功回执，核对冻结的原事件、流和提交的 schema 版本。 */
export function decodeRealtimeReplayReceipt(
  response: TransportResponse,
  failure: Readonly<Pick<AdminRealtimeFailureResponse, "event_id" | "stream_key">>,
  requestedSchema: number,
): Result<void, string> {
  const receipt = decodeAdminReceipt(
    response,
    apiResponseSchema(AdminRealtimeReplayResponseSchema),
    REPLAY_RECEIPT_FIELD_POLICY,
  );
  if (!receipt.ok) {
    return err(receipt.error);
  }
  return validReplay(receipt.value.data.replay, failure, requestedSchema)
    ? ok(undefined)
    : err(invalidResponseError("realtime_replay"));
}

function validReplay(
  replay: AdminRealtimeReplayEventResponse,
  failure: Readonly<Pick<AdminRealtimeFailureResponse, "event_id" | "stream_key">>,
  requestedSchema: number,
): boolean {
  return (
    validUuid(replay.event_id) &&
    validUuid(replay.replay_of_event_id) &&
    replay.event_id !== replay.replay_of_event_id &&
    replay.replay_of_event_id === failure.event_id &&
    validTrimmedText(replay.stream_key) &&
    replay.stream_key === failure.stream_key &&
    replay.stream_sequence >= 1 &&
    replay.stream_sequence <= MAX_JSON_SAFE_INTEGER &&
    validTrimmedText(replay.kind) &&
    replay.schema_version > 0 &&
    replay.schema_version === requestedSchema &&
    validAdminTime(replay.created_at)
  );
}
