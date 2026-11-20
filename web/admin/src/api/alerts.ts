// Admin 告警台 HTTP 客户端。
// 浏览器只接收告警台的安全投影；确认/静默命令携带稳定 `command_id`，同一输入重试复用
// 同一 ID 与服务端摘要。页面不能把静默当作阈值修改，服务端也不会改变 signal 状态。

import {
  ADMIN_ALERT_COMMAND_RESPONSE_FIELD_POLICY,
  type AdminAlertAcknowledgeRequest,
  AdminAlertCommandResponseSchema,
  type AdminAlertCommandResultResponse,
  type AdminAlertEntryResponse,
  type AdminAlertListResponse,
  AdminAlertListResponseSchema,
  type AdminAlertSilenceRequest,
  type AdminAlertWebhookEntryResponse,
  type AdminAlertWebhookListResponse,
  AdminAlertWebhookListResponseSchema,
  type AdminPendingCountsResponse,
  AdminPendingCountsResponseSchema,
} from "@auction/contracts/dto/admin/alerts";
import { apiResponseSchema } from "@auction/contracts/dto/response";
import { decode } from "@auction/contracts/wire";
import type { TransportResponse } from "@auction/web-shared/net/transport";
import { type JsonFieldPolicy, parseJson } from "@auction/web-shared/net/wire";
import { err, ok, type Result } from "@auction/web-shared/result";

import {
  type AdminApiFailure,
  adminApiFailureClient,
  decodeAdminReceipt,
  invalidResponseError,
  responseFailure,
} from "./failure";
import { validate as validatePagination, validPageEcho } from "./pagination";
import type { AdminSessionHost } from "./session";
import {
  type PreparedJsonCommand,
  prepareJsonCommandOnce,
  sendGet,
  sendPreparedJsonWrite,
} from "./transport";
import { validCount } from "./validation";

/** 告警条目的 wire 投影别名，与 Rust 的 `AdminAlert` 对应。 */
export type AdminAlert = AdminAlertEntryResponse;

/** 告警列表的 wire 投影别名，与 Rust 的 `AdminAlertPage` 对应。 */
export type AdminAlertPage = AdminAlertListResponse;

/** webhook 投递投影的 wire 别名，与 Rust 的 `AdminAlertWebhook` 对应。 */
export type AdminAlertWebhook = AdminAlertWebhookEntryResponse;

/** webhook 列表的 wire 投影别名，与 Rust 的 `AdminAlertWebhookPage` 对应。 */
export type AdminAlertWebhookPage = AdminAlertWebhookListResponse;

/** 导航待办计数的 wire 投影别名，与 Rust 的 `AdminPendingCounts` 对应。 */
export type AdminPendingCounts = AdminPendingCountsResponse;

/** 确认/静默命令的稳定回执别名，与 Rust 的 `AdminAlertCommandResult` 对应。 */
export type AdminAlertCommandResult = AdminAlertCommandResultResponse;

/** 告警状态筛选的线上闭集。 */
export type AdminAlertStatus = "active" | "resolved" | "all";

/** 告警命令动作；同时决定接口后缀与回执检查。 */
export type AdminAlertCommandAction = "acknowledge" | "silence";

/** 准备一条告警命令；返回的函数在第一次运行时生成并缓存固定正文。 */
export type AdminAlertCommandPreparation = (
  commandId: string,
) => Result<PreparedJsonCommand, AdminApiFailure>;

const ALERT_STATUSES: readonly AdminAlertStatus[] = ["active", "resolved", "all"];

const ALERT_SOURCES: ReadonlySet<string> = new Set([
  "process",
  "delivery",
  "fulfillment",
  "recovery",
  "mail",
  "credential",
  "lifecycle",
  "forum",
  "notification",
  "backup",
  "wal",
  "reconciliation",
]);

const WEBHOOK_STATUSES: ReadonlySet<string> = new Set([
  "pending",
  "sending",
  "succeeded",
  "failed",
  "manual_reconcile",
]);

const MIN_SILENCE_SECONDS = 60;
const MAX_SILENCE_SECONDS = 7 * 24 * 3_600;
const NIL_UUID = "00000000-0000-0000-0000-000000000000";

// 告警安全投影的数值列都是计数、代数或分页整数；没有自由 JSON，整份文档都可用整数词法收口。
const ADMIN_ALERT_LIST_FIELD_POLICY: JsonFieldPolicy = {
  uniqueKeys: "all",
  numbers: "integers",
};

const ADMIN_ALERT_WEBHOOK_FIELD_POLICY: JsonFieldPolicy = {
  uniqueKeys: "all",
  numbers: "integers",
};

const ADMIN_PENDING_COUNTS_FIELD_POLICY: JsonFieldPolicy = {
  uniqueKeys: "all",
  numbers: "integers",
};

/** 读取一页告警台条目；`status` 为 active/resolved/all。 */
export async function fetchAdminAlerts(
  host: AdminSessionHost,
  status: string,
  page: number,
  pageSize: number,
  signal: AbortSignal,
): Promise<Result<AdminAlertPage, string>> {
  const url = alertListUrl(status, page, pageSize);
  if (!url.ok) {
    return err(url.error);
  }
  const response = await sendAlertRequest(host, url.value, null, signal);
  if (!response.ok) {
    return err(response.error.code);
  }
  return decodeAlertPage(response.value.text(), status, page, pageSize);
}

function alertListUrl(status: string, page: number, pageSize: number): Result<string, string> {
  const pagination = validatePagination(page, pageSize, 100, 100_000);
  if (!pagination.ok) {
    return err(pagination.error);
  }
  if (!ALERT_STATUSES.some((candidate) => candidate === status)) {
    return err("invalid_request");
  }
  return ok(
    `/api/v1/admin/alerts?status=${status}&page=${page}&page_size=${pageSize}&include=incident_generation`,
  );
}

function decodeAlertPage(
  body: string,
  status: string,
  page: number,
  pageSize: number,
): Result<AdminAlertPage, string> {
  const parsed = parseJson(body, ADMIN_ALERT_LIST_FIELD_POLICY);
  if (!parsed.ok) {
    return err(invalidResponseError(parsed.error));
  }
  const decoded = decode(apiResponseSchema(AdminAlertListResponseSchema), parsed.value);
  if (!decoded.ok) {
    return err(invalidResponseError(decoded));
  }
  const pageData = decoded.value.data;
  if (!validAlertPage(status, page, pageSize, pageData)) {
    return err(invalidResponseError("alert page"));
  }
  return ok(pageData);
}

/**
 * 告警台分页响应的失败关闭校验。`total` 是所选筛选下的条数：active 取 `active_total`，
 * resolved 取 `resolved_total`，all 取两者之和；两个分类计数不随筛选变化，只用于页头提示。
 * 条目也必须落在所选筛选内，已恢复的告警不能混进活动视图（反之亦然）。
 */
function validAlertPage(
  status: string,
  page: number,
  pageSize: number,
  data: AdminAlertPage,
): boolean {
  let expectedTotal: number | null;
  let requiredActive: boolean | null;
  switch (status) {
    case "active":
      expectedTotal = data.active_total;
      requiredActive = true;
      break;
    case "resolved":
      expectedTotal = data.resolved_total;
      requiredActive = false;
      break;
    case "all":
      // 两类之和溢出说明计数已损坏，按不匹配处理；两个分类计数稍后仍由 `validCount` 收紧。
      {
        const sum = data.active_total + data.resolved_total;
        expectedTotal = Number.isSafeInteger(sum) ? sum : null;
      }
      requiredActive = null;
      break;
    default:
      return false;
  }
  return (
    validPageEcho(page, pageSize, data.page, data.page_size, data.total, data.alerts.length) &&
    validCount(data.active_total) &&
    validCount(data.resolved_total) &&
    expectedTotal === data.total &&
    data.alerts.every(
      (alert) => validAlert(alert) && (requiredActive === null || alert.active === requiredActive),
    )
  );
}

/** 读取最近的 webhook 投递安全投影；只读，不触发重试。 */
export async function fetchAdminWebhooks(
  host: AdminSessionHost,
  limit: number,
  signal: AbortSignal,
): Promise<Result<readonly AdminAlertWebhook[], string>> {
  if (!Number.isSafeInteger(limit) || limit < 1 || limit > 100) {
    return err("invalid_request");
  }
  const url = `/api/v1/admin/alerts/webhooks?limit=${limit}`;
  const response = await sendAlertRequest(host, url, null, signal);
  if (!response.ok) {
    return err(response.error.code);
  }
  return decodeWebhookPage(response.value.text(), limit);
}

function decodeWebhookPage(
  body: string,
  limit: number,
): Result<readonly AdminAlertWebhook[], string> {
  const parsed = parseJson(body, ADMIN_ALERT_WEBHOOK_FIELD_POLICY);
  if (!parsed.ok) {
    return err(invalidResponseError(parsed.error));
  }
  const decoded = decode(apiResponseSchema(AdminAlertWebhookListResponseSchema), parsed.value);
  if (!decoded.ok) {
    return err(invalidResponseError(decoded));
  }
  const page = decoded.value.data;
  if (
    page.entries.length > limit ||
    page.entries.some(
      (entry) =>
        !WEBHOOK_STATUSES.has(entry.status) ||
        entry.id === "" ||
        entry.kind === "" ||
        entry.scope === "" ||
        entry.source === "",
    )
  ) {
    return err(invalidResponseError("webhook page"));
  }
  return ok(page.entries);
}

/** 导航待办计数（显式请求未结案结算退件数）；失败时导航不显示徽标而不是猜测数量。 */
export async function fetchAdminPendingCounts(
  host: AdminSessionHost,
  signal: AbortSignal,
): Promise<Result<AdminPendingCounts, string>> {
  const response = await sendGet(
    host,
    "/api/v1/admin/pending-counts?include=settlement_returns",
    signal,
  );
  if (!response.ok) {
    return err(response.error);
  }
  if (!responseOk(response.value.status)) {
    return err((await responseFailure(response.value, signal, host)).code);
  }
  const parsed = parseJson(response.value.text(), ADMIN_PENDING_COUNTS_FIELD_POLICY);
  if (!parsed.ok) {
    return err(invalidResponseError(parsed.error));
  }
  const decoded = decode(apiResponseSchema(AdminPendingCountsResponseSchema), parsed.value);
  if (!decoded.ok) {
    return err(invalidResponseError(decoded));
  }
  const counts = decoded.value.data;
  if (!validPendingCounts(counts)) {
    return err(invalidResponseError("pending counts"));
  }
  return ok(counts);
}

/** 请求了退件计数时它必须出现；全部计数是非负安全整数。 */
function validPendingCounts(counts: AdminPendingCounts): boolean {
  return (
    validCount(counts.delivery_manual_reconcile) &&
    validCount(counts.recovery_confirmation) &&
    validCount(counts.active_alerts) &&
    counts.settlement_returns !== null &&
    validCount(counts.settlement_returns)
  );
}

/** 冻结确认命令的输入；重试只复用原命令 ID 与所处置的事件代数。 */
export function prepareAdminAlertAcknowledge(
  incidentGeneration: number,
  reason: string,
): AdminAlertCommandPreparation {
  return prepareJsonCommandOnce(
    (commandId): AdminAlertAcknowledgeRequest => ({
      command_id: commandId,
      incident_generation: incidentGeneration,
      reason,
    }),
  );
}

/**
 * 冻结静默命令的输入；静默时长越界在发送前失败关闭，不交给服务端裁剪。
 * 越界输入仍返回确定失败，确保未决命令不会发出与原命令不同的载荷。
 */
export function prepareAdminAlertSilence(
  incidentGeneration: number,
  silenceSeconds: number,
  reason: string,
): AdminAlertCommandPreparation {
  if (
    !Number.isInteger(silenceSeconds) ||
    silenceSeconds < MIN_SILENCE_SECONDS ||
    silenceSeconds > MAX_SILENCE_SECONDS
  ) {
    return () => err(adminApiFailureClient("invalid_request"));
  }
  return prepareJsonCommandOnce(
    (commandId): AdminAlertSilenceRequest => ({
      command_id: commandId,
      incident_generation: incidentGeneration,
      reason,
      silence_seconds: silenceSeconds,
    }),
  );
}

/** 确认一条活动告警。 */
export async function acknowledgeAdminAlert(
  host: AdminSessionHost,
  alertId: string,
  commandId: string,
  prepare: AdminAlertCommandPreparation,
  signal: AbortSignal,
): Promise<Result<AdminAlertCommandResult, AdminApiFailure>> {
  return submitAlertCommand(host, commandId, alertId, "acknowledge", prepare, signal);
}

/** 静默一条活动告警到 `silenceSeconds` 之后。 */
export async function silenceAdminAlert(
  host: AdminSessionHost,
  alertId: string,
  commandId: string,
  prepare: AdminAlertCommandPreparation,
  signal: AbortSignal,
): Promise<Result<AdminAlertCommandResult, AdminApiFailure>> {
  return submitAlertCommand(host, commandId, alertId, "silence", prepare, signal);
}

async function submitAlertCommand(
  host: AdminSessionHost,
  commandId: string,
  alertId: string,
  action: AdminAlertCommandAction,
  prepare: AdminAlertCommandPreparation,
  signal: AbortSignal,
): Promise<Result<AdminAlertCommandResult, AdminApiFailure>> {
  if (!validAlertId(alertId)) {
    return err(adminApiFailureClient("invalid_request"));
  }
  const prepared = prepare(commandId);
  if (!prepared.ok) {
    return err(prepared.error);
  }
  const url = `/api/v1/admin/alerts/${alertId}/${action}`;
  const response = await sendAlertRequest(host, url, prepared.value, signal);
  if (!response.ok) {
    return err(response.error);
  }
  return decodeAlertReceipt(response.value, commandId, alertId, action);
}

function decodeAlertReceipt(
  response: TransportResponse,
  commandId: string,
  alertId: string,
  action: AdminAlertCommandAction,
): Result<AdminAlertCommandResult, AdminApiFailure> {
  const decoded = decodeAdminReceipt(
    response,
    apiResponseSchema(AdminAlertCommandResponseSchema),
    ADMIN_ALERT_COMMAND_RESPONSE_FIELD_POLICY,
  );
  if (!decoded.ok) {
    return err(adminApiFailureClient(decoded.error));
  }
  const result = decoded.value.data.alert;
  if (
    result.id !== alertId ||
    result.action !== action ||
    (action === "silence" && result.silenced_until === null) ||
    (action === "acknowledge" && result.silenced_until !== null) ||
    result.id === "" ||
    commandId === NIL_UUID
  ) {
    return err(adminApiFailureClient(invalidResponseError("alert command")));
  }
  return ok(result);
}

async function sendAlertRequest(
  host: AdminSessionHost,
  url: string,
  body: PreparedJsonCommand | null,
  signal: AbortSignal,
): Promise<Result<TransportResponse, AdminApiFailure>> {
  if (body !== null) {
    return sendPreparedJsonWrite(host, url, "POST", body, signal);
  }
  const response = await sendGet(host, url, signal);
  if (!response.ok) {
    return err(adminApiFailureClient(response.error));
  }
  if (!responseOk(response.value.status)) {
    return err(await responseFailure(response.value, signal, host));
  }
  return ok(response.value);
}

/** 告警列表条目的失败关闭谓词；未知严重度、来源或缺失事件代数都不进入活动视图。 */
function validAlert(alert: AdminAlert): boolean {
  return (
    validAlertId(alert.id) &&
    (alert.severity === "urgent" || alert.severity === "digest") &&
    ALERT_SOURCES.has(alert.source) &&
    alert.kind !== "" &&
    alert.scope !== "" &&
    alert.runbook_anchor !== "" &&
    alert.incident_generation !== null &&
    alert.incident_generation >= 0 &&
    alert.count >= 0 &&
    alert.age_seconds >= 0 &&
    alert.active === (alert.resolved_at === null) &&
    alert.first_seen_at !== "" &&
    alert.last_seen_at !== ""
  );
}

/** 64 位十六进制 fingerprint；`is_ascii_hexdigit` 同时接受大写。 */
function validAlertId(value: string): boolean {
  if (value.length !== 64) {
    return false;
  }
  for (let index = 0; index < value.length; index += 1) {
    const code = value.charCodeAt(index);
    const digit = code >= 0x30 && code <= 0x39;
    const upper = code >= 0x41 && code <= 0x46;
    const lower = code >= 0x61 && code <= 0x66;
    if (!digit && !upper && !lower) {
      return false;
    }
  }
  return true;
}

function responseOk(status: number): boolean {
  return status >= 200 && status <= 299;
}
