// Admin 拍卖列表 API 客户端。
// 页面只接收后端的生命周期安全投影；创建命令携带新的 `command_id`，状态与幂等仍由服务端负责。

import {
  ADMIN_AUCTION_LIFECYCLE_RESPONSE_FIELD_POLICY,
  ADMIN_UPDATE_AUCTION_SCHEDULE_RESPONSE_FIELD_POLICY,
  type AdminAuctionCommandResponse,
  type AdminAuctionLifecycleResponse,
  AdminAuctionLifecycleResponseSchema,
  type AdminCreateAuctionRequest,
  type AdminUpdateAuctionScheduleRequest,
  AdminUpdateAuctionScheduleResponseSchema,
} from "@auction/contracts/dto/admin/auction";
import {
  type AdminAuctionCapabilitiesResponse,
  type AdminAuctionListResponse,
  AdminAuctionListResponseSchema,
  type AdminAuctionResponse,
} from "@auction/contracts/dto/admin/settlement";
import { type ApiResponse, apiResponseSchema } from "@auction/contracts/dto/response";
import type { JsonFieldPolicy } from "@auction/web-shared/net/wire";
import { err, ok, type Result } from "@auction/web-shared/result";
import { validUuid } from "@auction/web-shared/ui/validation";
import type * as v from "valibot";

import {
  type AdminApiFailure,
  decodeAdminReceipt,
  invalidResponseError,
  responseError,
} from "./failure";
import { validate as validatePagination, validPageEcho } from "./pagination";
import type { AdminSessionHost } from "./session";
import {
  type PreparedJsonCommand,
  prepareJsonCommandOnce,
  sendGet,
  sendPreparedJsonWrite,
} from "./transport";
import {
  MAX_JSON_SAFE_INTEGER,
  parseAdminTime,
  validAdminTime,
  validCount,
  validOptionalText,
  validOptionalTime,
  validTrimmedText,
} from "./validation";

export type AdminAuctionCapabilities = AdminAuctionCapabilitiesResponse;
export type AdminAuctionPage = AdminAuctionListResponse;
export type AdminAuction = AdminAuctionResponse;

/** 未决命令执行体复用的正文工厂；同一 `command_id` 的重试只会得到同一份文本。 */
export type AdminAuctionCommandPreparation = (
  commandId: string,
) => Result<PreparedJsonCommand, AdminApiFailure>;

/** 列表回执里的版本与计数会决定后续命令，因此按整数词法严格解析。 */
const ADMIN_AUCTION_LIST_RESPONSE_FIELD_POLICY: JsonFieldPolicy = {
  uniqueKeys: "all",
  numbers: "integers",
};

// 旧 Admin 列表和生命周期命令共用同一拍卖 decoder；结构闭集由 contracts 的
// `AdminAuctionResponse` 与 `AdminAuctionCommandResponse`（`deny_unknown_fields` 与必填
// nullable 语义）负责，本文件只保留跨字段业务谓词。

/** 读取一页场次（全部状态）；`world_id` 为空或 `all` 时不按世界筛选。 */
export async function fetchAdminAuctions(
  host: AdminSessionHost,
  worldId: string,
  page: number,
  pageSize: number,
  signal: AbortSignal,
): Promise<Result<AdminAuctionPage, string>> {
  const bounds = validatePagination(page, pageSize, 100, 100_000);
  if (!bounds.ok) {
    return bounds;
  }
  let url = `/api/v1/admin/auctions?page=${page}&page_size=${pageSize}`;
  if (worldId !== "" && worldId !== "all") {
    url += "&world_id=";
    url += worldId;
  }
  const response = await sendGet(host, url, signal);
  if (!response.ok) {
    return err(response.error);
  }
  const received = response.value;
  if (!responseOk(received)) {
    return err(await responseError(received, signal, host));
  }
  const payload = decodeAdminReceipt(
    received,
    apiResponseSchema(AdminAuctionListResponseSchema),
    ADMIN_AUCTION_LIST_RESPONSE_FIELD_POLICY,
  );
  if (!payload.ok) {
    return err(payload.error);
  }
  const decoded = decodeAuctionList(payload.value.data);
  if (!decoded.ok) {
    return err(invalidResponseError(decoded.error));
  }
  const result = decoded.value;
  const pagination = result.pagination;
  if (
    !validPageEcho(
      page,
      pageSize,
      pagination.page,
      pagination.page_size,
      pagination.total,
      result.auctions.length,
    )
  ) {
    return err(invalidResponseError("auction_pagination"));
  }
  return ok(result);
}

/** 冻结 draft 创建输入；执行体第一次运行时才结合 command ID 生成正文。 */
export function prepareAdminAuctionCreateOnce(
  auctionId: string,
  worldId: string,
  sequence: number,
  previewScheduledAt: string,
  scheduledOpenAt: string,
  scheduledEndAt: string,
): AdminAuctionCommandPreparation {
  return prepareJsonCommandOnce((commandId) => {
    const command: AdminCreateAuctionRequest = {
      command_id: commandId,
      auction_id: auctionId,
      world_id: worldId,
      sequence,
      preview_scheduled_at: previewScheduledAt,
      scheduled_open_at: scheduledOpenAt,
      scheduled_end_at: scheduledEndAt,
      expected_state_version: 0,
    };
    return command;
  });
}

/** 提交 Admin draft 创建命令；调用组件负责在同一输入重试时复用两个稳定标识。 */
export async function createAdminAuction(
  host: AdminSessionHost,
  commandId: string,
  auctionId: string,
  prepare: AdminAuctionCommandPreparation,
  signal: AbortSignal,
): Promise<Result<AdminAuctionCommandResponse, string>> {
  const prepared = prepare(commandId);
  if (!prepared.ok) {
    return err(prepared.error.code);
  }
  const response = await sendPreparedJsonWrite(
    host,
    "/api/v1/admin/auctions",
    "POST",
    prepared.value,
    signal,
  );
  if (!response.ok) {
    return err(response.error.code);
  }
  const payload = decodeAdminReceipt(
    response.value,
    apiResponseSchema(AdminAuctionLifecycleResponseSchema),
    ADMIN_AUCTION_LIFECYCLE_RESPONSE_FIELD_POLICY,
  );
  if (!payload.ok) {
    return err(payload.error);
  }
  const decoded = decodeAuctionCommandResponse(payload.value);
  if (!decoded.ok) {
    return err(decoded.error);
  }
  const auction = decoded.value;
  if (auction.id !== auctionId || auction.status !== "draft") {
    return err("invalid_response");
  }
  return ok(auction);
}

/** 冻结计划修改输入；执行体第一次运行时才结合 command ID 生成正文。 */
export function prepareAdminAuctionScheduleOnce(
  previewScheduledAt: string,
  scheduledOpenAt: string,
  scheduledEndAt: string,
  expectedStateVersion: number,
): AdminAuctionCommandPreparation {
  return prepareJsonCommandOnce((commandId) => {
    const command: AdminUpdateAuctionScheduleRequest = {
      command_id: commandId,
      preview_scheduled_at: previewScheduledAt,
      scheduled_open_at: scheduledOpenAt,
      scheduled_end_at: scheduledEndAt,
      expected_state_version: expectedStateVersion,
    };
    return command;
  });
}

/** 修改 Draft/Preview 拍卖的三项计划时间；同一输入重试时由组件复用命令 ID。 */
export async function updateAdminAuctionSchedule(
  host: AdminSessionHost,
  commandId: string,
  auctionId: string,
  previewScheduledAt: string,
  scheduledOpenAt: string,
  scheduledEndAt: string,
  expectedStateVersion: number,
  prepare: AdminAuctionCommandPreparation,
  signal: AbortSignal,
): Promise<Result<AdminAuction, string>> {
  if (!validUuid(auctionId) || expectedStateVersion <= 0) {
    return err("invalid_request");
  }
  const expectedPreview = parseAdminTime(previewScheduledAt);
  const expectedOpen = parseAdminTime(scheduledOpenAt);
  const expectedEnd = parseAdminTime(scheduledEndAt);
  const prepared = prepare(commandId);
  if (!prepared.ok) {
    return err(prepared.error.code);
  }
  const url = `/api/v1/admin/auctions/${auctionId}/schedule`;
  const payload = await sendAuctionEdit(
    host,
    "PATCH",
    url,
    prepared.value,
    apiResponseSchema(AdminUpdateAuctionScheduleResponseSchema),
    ADMIN_UPDATE_AUCTION_SCHEDULE_RESPONSE_FIELD_POLICY,
    signal,
  );
  if (!payload.ok) {
    return err(payload.error);
  }
  const decoded = decodeAuction(payload.value.data.auction);
  if (!decoded.ok) {
    return err(invalidResponseError(decoded.error));
  }
  const auction = decoded.value;
  if (
    auction.id !== auctionId ||
    (auction.status !== "draft" && auction.status !== "preview") ||
    auction.state_version < expectedStateVersion ||
    parseAdminTime(auction.preview_scheduled_at) !== expectedPreview ||
    parseAdminTime(auction.scheduled_open_at) !== expectedOpen ||
    (auction.scheduled_ends_at === null ? null : parseAdminTime(auction.scheduled_ends_at)) !==
      expectedEnd
  ) {
    return err("invalid_response");
  }
  return ok(auction);
}

/** 生命周期与创建命令共用旧版 `{ auction }` 闭集 decoder。 */
export function decodeAuctionCommandResponse(
  payload: ApiResponse<AdminAuctionLifecycleResponse>,
): Result<AdminAuctionCommandResponse, string> {
  const decoded = decodeAuction(commandIntoAuction(payload.data.auction));
  return decoded.ok
    ? ok(auctionIntoCommand(decoded.value))
    : err(invalidResponseError(decoded.error));
}

/** 场次与拍品的可变命令共用写入边界；成功回执按调用方给出的闭集与字段策略解析。 */
export async function sendAuctionEdit<const TSchema extends v.GenericSchema>(
  host: AdminSessionHost,
  method: string,
  url: string,
  prepared: PreparedJsonCommand,
  responseSchema: TSchema,
  responsePolicy: JsonFieldPolicy,
  signal: AbortSignal,
): Promise<Result<v.InferOutput<TSchema>, string>> {
  const response = await sendPreparedJsonWrite(host, url, method, prepared, signal);
  if (!response.ok) {
    return err(response.error.code);
  }
  return decodeAdminReceipt(response.value, responseSchema, responsePolicy);
}

function decodeAuctionList(data: AdminAuctionPage): Result<AdminAuctionPage, string> {
  const auctions: AdminAuction[] = [];
  for (const auction of data.auctions) {
    const decoded = decodeAuction(auction);
    if (!decoded.ok) {
      return err(decoded.error);
    }
    auctions.push(decoded.value);
  }
  return ok({ auctions, capabilities: data.capabilities, pagination: data.pagination });
}

/** 旧版命令投影缺少列表扩展字段；补为缺失值后复用同一闭集谓词。 */
function commandIntoAuction(value: AdminAuctionCommandResponse): AdminAuction {
  return {
    id: value.id,
    world_id: value.world_id,
    sequence: value.sequence,
    status: value.status,
    preview_scheduled_at: value.preview_scheduled_at,
    scheduled_open_at: value.scheduled_open_at,
    preview_opened_at: value.preview_opened_at,
    preview_manual_open: value.preview_manual_open,
    preview_revision: value.preview_revision,
    opened_at: value.opened_at,
    final_phase_at: value.final_phase_at,
    settled_at: value.settled_at,
    cancelled_at: value.cancelled_at,
    cancelled_reason: value.cancelled_reason,
    cancelled_by: value.cancelled_by,
    state_version: value.state_version,
    scheduled_ends_at: null,
    lot_count: null,
    created_at: value.created_at,
    updated_at: value.updated_at,
  };
}

/** 跨字段谓词保留为独立校验：时间顺序、cancelled 字段组合与值域仍在浏览器边界 fail-closed。 */
export function decodeAuction(value: AdminAuction): Result<AdminAuction, string> {
  const previewScheduledAt = parseAdminTime(value.preview_scheduled_at);
  if (previewScheduledAt === null) {
    return err("auction_preview_time");
  }
  const scheduledOpenAt = parseAdminTime(value.scheduled_open_at);
  if (scheduledOpenAt === null) {
    return err("auction_open_time");
  }
  const cancellationFieldsPresent =
    value.cancelled_at !== null && value.cancelled_by !== null && value.cancelled_reason !== null;
  const cancellationFieldsAbsent =
    value.cancelled_at === null && value.cancelled_by === null && value.cancelled_reason === null;
  if (
    !validUuid(value.id) ||
    !validTrimmedText(value.world_id) ||
    !(value.sequence >= 1 && value.sequence <= MAX_JSON_SAFE_INTEGER) ||
    !(
      value.status === "draft" ||
      value.status === "preview" ||
      value.status === "live" ||
      value.status === "final" ||
      value.status === "settling" ||
      value.status === "settled" ||
      value.status === "cancelled"
    ) ||
    scheduledOpenAt < previewScheduledAt ||
    !(value.preview_revision >= 0 && value.preview_revision <= MAX_JSON_SAFE_INTEGER) ||
    !(value.state_version >= 1 && value.state_version <= MAX_JSON_SAFE_INTEGER) ||
    !validAdminTime(value.created_at) ||
    !validAdminTime(value.updated_at) ||
    !validOptionalTime(value.cancelled_at) ||
    !validOptionalText(value.cancelled_by) ||
    !validOptionalText(value.cancelled_reason) ||
    !validOptionalTime(value.preview_opened_at) ||
    !validOptionalTime(value.opened_at) ||
    !validOptionalTime(value.final_phase_at) ||
    !validOptionalTime(value.settled_at) ||
    !validOptionalTime(value.scheduled_ends_at) ||
    (value.lot_count !== null && !validCount(value.lot_count)) ||
    (value.status === "cancelled" ? !cancellationFieldsPresent : !cancellationFieldsAbsent)
  ) {
    return err("auction");
  }
  return ok(value);
}

function auctionIntoCommand(value: AdminAuction): AdminAuctionCommandResponse {
  return {
    id: value.id,
    world_id: value.world_id,
    sequence: value.sequence,
    status: value.status,
    preview_scheduled_at: value.preview_scheduled_at,
    scheduled_open_at: value.scheduled_open_at,
    preview_opened_at: value.preview_opened_at,
    preview_manual_open: value.preview_manual_open,
    preview_revision: value.preview_revision,
    opened_at: value.opened_at,
    final_phase_at: value.final_phase_at,
    settled_at: value.settled_at,
    cancelled_at: value.cancelled_at,
    cancelled_reason: value.cancelled_reason,
    cancelled_by: value.cancelled_by,
    state_version: value.state_version,
    created_at: value.created_at,
    updated_at: value.updated_at,
  };
}

/** gloo 的 `Response::ok`：状态码 200 到 299。 */
function responseOk(response: { readonly status: number }): boolean {
  return response.status >= 200 && response.status <= 299;
}
