// Admin 审计查询 HTTP 适配。
// 页面只使用后端的安全审计投影；metadata 作为有界 JSON 展示，不在浏览器端推导状态或编辑
// 历史事实。数字页语义与旧版保持一致，筛选变化会立即重新加载当前第一页。

import type {
  AdminAuditCursorResponse,
  AdminAuditRecordResponse,
  AdminAuditResponse,
} from "@auction/contracts/dto/admin/audit";
import { apiResponseSchema } from "@auction/contracts/dto/response";
import { integer, jsonValue, requiredNullable, struct } from "@auction/contracts/wire";
import type { TransportResponse } from "@auction/web-shared/net/transport";
import type { JsonFieldPolicy } from "@auction/web-shared/net/wire";
import { err, ok, type Result } from "@auction/web-shared/result";
import { validUuid } from "@auction/web-shared/ui/validation";
import * as v from "valibot";

import { decodeAdminReceipt, invalidResponseError, responseError } from "./failure";
import { validate as validatePagination, validPageEcho } from "./pagination";
import type { AdminSessionHost } from "./session";
import { parseStrictRfc3339, trimRustWhitespace } from "./session-time";
import { sendGet } from "./transport";
import { parseLocalDatetime, validAdminTime } from "./validation";

// 旧 audit decoder 允许响应增加字段，但所有旧必需字段仍必须显式存在并满足业务语义。
// contracts 的 DTO 不能表达 `deserialize_required_option`，因此这里保留严格镜像：
// `next_cursor` 与 `request_id` 缺键即失败，显式 null 才算 None；同步义务与源码相同。
const StrictAuditCursorSchema = struct({
  before_id: v.string(),
  before_time: v.string(),
});

const StrictAuditRecordSchema = struct({
  id: v.string(),
  occurred_at: v.string(),
  actor_type: v.string(),
  actor_id: v.string(),
  action: v.string(),
  entity_type: v.string(),
  entity_id: v.string(),
  result: v.string(),
  request_id: requiredNullable(v.string()),
  source: v.string(),
  metadata: jsonValue,
});

const StrictAuditPageSchema = struct({
  records: v.array(StrictAuditRecordSchema),
  page: integer,
  page_size: integer,
  total: integer,
  has_more: v.boolean(),
  next_cursor: requiredNullable(StrictAuditCursorSchema),
  next_cursor_token: v.string(),
});

const AUDIT_PAGE_FIELD_POLICY: JsonFieldPolicy = {
  integerPaths: [
    ["data", "page"],
    ["data", "page_size"],
    ["data", "total"],
  ],
};

const I64_MAX = 9_223_372_036_854_775_807n;

/** 单条审计安全投影；对应 Rust 的 `AdminAuditRecord` 别名。 */
export type AdminAuditRecord = AdminAuditRecordResponse;
/** 一页审计安全投影；对应 Rust 的 `AdminAuditPage` 别名。 */
export type AdminAuditPage = AdminAuditResponse;

/** 审计查询的文本筛选；时间字段保留本地时间或带偏移的旧值，发送前再规范化。 */
export interface AdminAuditFilter {
  actor: string;
  actorType: string;
  action: string;
  entity: string;
  source: string;
  result: string;
  requestId: string;
  occurredFrom: string;
  occurredTo: string;
}

/** 空筛选：所有文本条件都不限制，时间条件为空。 */
export function adminAuditFilterDefault(): AdminAuditFilter {
  return {
    actor: "",
    actorType: "",
    action: "",
    entity: "",
    source: "",
    result: "",
    requestId: "",
    occurredFrom: "",
    occurredTo: "",
  };
}

/**
 * 应用、恢复和发送筛选共用首尾空白规则，避免同一个精确匹配条件产生两个查询身份。
 * 与 Rust 的 `&mut self` 一致，直接改写传入的筛选对象。
 */
export function adminAuditFilterNormalizeText(filter: AdminAuditFilter): void {
  filter.actor = trimRustWhitespace(filter.actor);
  filter.actorType = trimRustWhitespace(filter.actorType);
  filter.action = trimRustWhitespace(filter.action);
  filter.entity = trimRustWhitespace(filter.entity);
  filter.source = trimRustWhitespace(filter.source);
  filter.result = trimRustWhitespace(filter.result);
  filter.requestId = trimRustWhitespace(filter.requestId);
}

/**
 * 将本地时间或带偏移的旧值规范化，保证每个绝对时刻有唯一的筛选身份。
 */
export function adminAuditFilterAbsoluteTime(value: string): string | null {
  const trimmed = trimRustWhitespace(value);
  if (trimmed === "") {
    return "";
  }
  const instant = parseStrictRfc3339(trimmed);
  if (instant !== null) {
    return formatRfc3339Nanos(instant);
  }
  const local = parseLocalDatetime(trimmed);
  return local.ok ? local.value : null;
}

/** 读取一页 Admin 审计安全投影。 */
export async function fetchAdminAudit(
  host: AdminSessionHost,
  page: number,
  filter: AdminAuditFilter,
  pageSize: number,
  signal: AbortSignal,
): Promise<Result<AdminAuditPage, string>> {
  const pagination = validatePagination(page, pageSize, 200, 2_147_483_647);
  if (!pagination.ok) {
    return err(pagination.error);
  }
  const normalized: AdminAuditFilter = { ...filter };
  adminAuditFilterNormalizeText(normalized);
  const query = new URLSearchParams();
  query.set("page", String(Math.max(page, 1)));
  query.set("page_size", String(pageSize));
  appendFilter(query, "actor", normalized.actor);
  appendFilter(query, "actor_type", normalized.actorType);
  appendFilter(query, "action", normalized.action);
  appendFilter(query, "entity", normalized.entity);
  appendFilter(query, "source", normalized.source);
  appendFilter(query, "result", normalized.result);
  appendFilter(query, "request_id", normalized.requestId);
  if (!appendTimeFilter(query, "occurred_from", normalized.occurredFrom)) {
    return err("invalid_query");
  }
  if (!appendTimeFilter(query, "occurred_to", normalized.occurredTo)) {
    return err("invalid_query");
  }
  try {
    const response = await sendGet(host, `/api/v1/admin/audit?${query.toString()}`, signal);
    if (!response.ok) {
      return err(response.error);
    }
    if (response.value.status < 200 || response.value.status >= 300) {
      return err(await responseError(response.value, signal, host));
    }
    const decoded = decodeAuditPage(response.value, page, pageSize);
    return decoded.ok ? ok(decoded.value) : err(decodeFailureCode(decoded.error));
  } catch {
    return err("network_error");
  }
}

/** `decodeAdminReceipt` 已上报并返回 `invalid_response`；组合校验失败在这里补报一次。 */
function decodeFailureCode(error: string): string {
  return error === "invalid_response" ? error : invalidResponseError(error);
}

function decodeAuditPage(
  response: TransportResponse,
  requestedPage: number,
  requestedSize: number,
): Result<AdminAuditPage, string> {
  const receipt = decodeAdminReceipt(
    response,
    apiResponseSchema(StrictAuditPageSchema),
    AUDIT_PAGE_FIELD_POLICY,
  );
  if (!receipt.ok) {
    return err(receipt.error);
  }
  const data = receipt.value.data;
  if (
    !validPageEcho(
      requestedPage,
      requestedSize,
      data.page,
      data.page_size,
      data.total,
      data.records.length,
    )
  ) {
    return err("audit_page");
  }
  const end = BigInt(data.page) * BigInt(data.page_size);
  const hasMore = end <= I64_MAX && end < BigInt(data.total);
  if (data.has_more !== hasMore) {
    return err("audit_page");
  }
  if (data.next_cursor === null && data.next_cursor_token !== "") {
    return err("audit_page");
  }
  const records: AdminAuditRecord[] = [];
  for (const candidate of data.records) {
    const record = decodeAuditRecord(candidate);
    if (!record.ok) {
      return err(record.error);
    }
    records.push(record.value);
  }
  let nextCursor: AdminAuditCursorResponse | null = null;
  if (data.next_cursor !== null) {
    const cursor = decodeAuditCursor(data.next_cursor);
    if (!cursor.ok) {
      return err(cursor.error);
    }
    nextCursor = cursor.value;
  }
  return ok({
    records,
    page: data.page,
    page_size: data.page_size,
    total: data.total,
    has_more: data.has_more,
    next_cursor: nextCursor,
    next_cursor_token: data.next_cursor_token,
  });
}

function decodeAuditCursor(
  cursor: v.InferOutput<typeof StrictAuditCursorSchema>,
): Result<AdminAuditCursorResponse, string> {
  if (!validUuid(cursor.before_id) || !validAdminTime(cursor.before_time)) {
    return err("audit_cursor");
  }
  return ok({ before_time: cursor.before_time, before_id: cursor.before_id });
}

function decodeAuditRecord(
  record: v.InferOutput<typeof StrictAuditRecordSchema>,
): Result<AdminAuditRecord, string> {
  if (
    !validUuid(record.id) ||
    !validAdminTime(record.occurred_at) ||
    !(
      record.actor_type === "public_user" ||
      record.actor_type === "admin" ||
      record.actor_type === "worker" ||
      record.actor_type === "system"
    ) ||
    record.actor_id === "" ||
    record.action === "" ||
    record.entity_type === "" ||
    record.entity_id === "" ||
    !(record.result === "success" || record.result === "rejected" || record.result === "failure") ||
    !(
      record.source === "public" ||
      record.source === "admin" ||
      record.source === "worker" ||
      record.source === "system" ||
      record.source === "cli"
    ) ||
    !isJsonObject(record.metadata)
  ) {
    return err("audit_record");
  }
  return ok({
    id: record.id,
    occurred_at: record.occurred_at,
    actor_type: record.actor_type,
    actor_id: record.actor_id,
    action: record.action,
    entity_type: record.entity_type,
    entity_id: record.entity_id,
    result: record.result,
    request_id: record.request_id,
    source: record.source,
    metadata: record.metadata,
  });
}

function appendFilter(query: URLSearchParams, key: string, value: string): void {
  if (value !== "") {
    query.append(key, value);
  }
}

function appendTimeFilter(query: URLSearchParams, key: string, value: string): boolean {
  const absolute = adminAuditFilterAbsoluteTime(value);
  if (absolute === null) {
    return false;
  }
  appendFilter(query, key, absolute);
  return true;
}

function isJsonObject(value: unknown): value is Readonly<Record<string, unknown>> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

const NANOS_PER_SECOND = 1_000_000_000n;

/** 与 Rust `SecondsFormat::AutoSi` 一致：小数秒只保留 0、3、6 或 9 位。 */
function formatRfc3339Nanos(nanos: bigint): string {
  let seconds = nanos / NANOS_PER_SECOND;
  let fraction = nanos % NANOS_PER_SECOND;
  if (fraction < 0n) {
    seconds -= 1n;
    fraction += NANOS_PER_SECOND;
  }
  const date = new Date(Number(seconds) * 1000);
  const base = date.toISOString().replace(/\.\d{3}Z$/u, "");
  if (fraction === 0n) {
    return `${base}Z`;
  }
  let digits = fraction.toString().padStart(9, "0");
  if (fraction % 1_000_000n === 0n) {
    digits = digits.slice(0, 3);
  } else if (fraction % 1_000n === 0n) {
    digits = digits.slice(0, 6);
  }
  return `${base}.${digits}Z`;
}
