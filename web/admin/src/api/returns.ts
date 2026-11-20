// Admin 结算退件案件的同源 HTTP 客户端。
//
// 列表与详情只接收服务端安全投影，按契约闭集失败关闭解码：未知状态、动作、原因或资产位置会
// 让整页读取失败，而不是被浏览器猜测成相近含义。处理命令携带页面命令宿主生成、在重试间保持
// 不变的 `command_id` 与读取时的案件版本；资格、锁与幂等都由服务端判定。

import {
  ADMIN_SETTLEMENT_RETURN_ACTIONS,
  ADMIN_SETTLEMENT_RETURN_ASSET_LOCATIONS,
  ADMIN_SETTLEMENT_RETURN_BLOCKERS,
  ADMIN_SETTLEMENT_RETURN_CLOSED_STATUSES,
  ADMIN_SETTLEMENT_RETURN_COMMAND_RESPONSE_FIELD_POLICY,
  ADMIN_SETTLEMENT_RETURN_EVENT_KINDS,
  ADMIN_SETTLEMENT_RETURN_MATCH_STATES,
  ADMIN_SETTLEMENT_RETURN_MAX_OFFSET,
  ADMIN_SETTLEMENT_RETURN_MAX_PAGE_SIZE,
  ADMIN_SETTLEMENT_RETURN_OBLIGATION_KINDS,
  ADMIN_SETTLEMENT_RETURN_PLANS,
  ADMIN_SETTLEMENT_RETURN_REASONS,
  ADMIN_SETTLEMENT_RETURN_STATUSES,
  ADMIN_SETTLEMENT_RETURN_TAKE_STATES,
  ADMIN_SETTLEMENT_RETURN_TAKE_UNKNOWN_REASONS,
  type AdminSettlementReturnCommandResponse,
  AdminSettlementReturnCommandResponseSchema,
  type AdminSettlementReturnCustodyResponse,
  type AdminSettlementReturnDetailResponse,
  AdminSettlementReturnDetailResponseSchema,
  type AdminSettlementReturnDispatchResponse,
  type AdminSettlementReturnEventResponse,
  type AdminSettlementReturnListResponse,
  AdminSettlementReturnListResponseSchema,
  type AdminSettlementReturnSummaryResponse,
  type AdminSettlementReturnTakeResponse,
} from "@auction/contracts/dto/admin/returns";
import { apiResponseSchema } from "@auction/contracts/dto/response";
import { decode } from "@auction/contracts/wire";
import { err, ok, type Result } from "@auction/web-shared/result";
import { validUuid } from "@auction/web-shared/ui/validation";

import { validStatus } from "./delivery/decode";
import {
  type AdminWriteFailure,
  type AdminWriteNotSent,
  decodeAdminReceipt,
  invalidResponseError,
  responseError,
  writeNotSent,
  writeOutcomeUnknown,
} from "./failure";
import { validate, validPageEcho } from "./pagination";
import type { AdminSessionHost } from "./session";
import {
  type PreparedJsonCommand,
  prepareJsonCommandOnce,
  sendGet,
  sendPreparedJsonWrite,
} from "./transport";
import {
  MAX_JSON_SAFE_INTEGER,
  validAdminTime,
  validCount,
  validOptionalTime,
  validSafeText,
  validSuccessCode,
} from "./validation";

/** 列表筛选；状态、分类、种类与世界都取契约闭集中的值，空串表示不限。 */
export interface SettlementReturnFilters {
  /** 只列未结案案件。 */
  readonly openOnly: boolean;
  readonly status: string;
  readonly matchState: string;
  readonly obligationKind: string;
  readonly world: string;
}

/** 与 Rust `Default` 相同：不限范围与各筛选维度。 */
export function settlementReturnFiltersDefault(): SettlementReturnFilters {
  return {
    openOnly: false,
    status: "",
    matchState: "",
    obligationKind: "",
    world: "",
  };
}

/** 世界筛选闭集。 */
const RETURN_WORLDS: readonly string[] = ["main", "isekai"];

/** Rust 的 `Uuid::nil`；命令宿主只会生成 v4 UUID，发送前还要校验完整 UUID 词法，nil 单独
 * 拒绝以照搬源码。 */
const NIL_UUID = "00000000-0000-0000-0000-000000000000";

/** 列表的具型整数字段与分页都由 serde 的整数 visitor 解析；解析完整响应原文时拒绝重复键，
 * 并拒绝这些字段的小数、指数与 `-0` 词法。 */
const ADMIN_SETTLEMENT_RETURN_LIST_FIELD_POLICY = {
  uniqueKeys: "all",
  integerPaths: [
    ["data", "pagination", "page"],
    ["data", "pagination", "page_size"],
    ["data", "pagination", "total"],
    ["data", "returns", null, "case_version"],
    ["data", "returns", null, "credits"],
    ["data", "returns", null, "cod"],
    ["data", "returns", null, "attachment_count"],
    ["data", "returns", null, "resend_limit"],
    ["data", "returns", null, "dispatch_no"],
    ["data", "returns", null, "resends_used"],
  ],
} as const;

/** 详情的全部具型整数字段都会参与页面决策或作为下一次命令的输入；解析完整响应原文时拒绝
 * 重复键，并拒绝这些字段的小数、指数与 `-0` 词法。事件 `details` 仍是安全投影中的自由对象，
 * 只对其路径上的具型字段施加整数词法。 */
const ADMIN_SETTLEMENT_RETURN_DETAIL_FIELD_POLICY = {
  uniqueKeys: "all",
  integerPaths: [
    ["data", "summary", "case_version"],
    ["data", "summary", "credits"],
    ["data", "summary", "cod"],
    ["data", "summary", "attachment_count"],
    ["data", "summary", "resend_limit"],
    ["data", "summary", "dispatch_no"],
    ["data", "summary", "resends_used"],
    ["data", "events", null, "case_version"],
    ["data", "dispatches", null, "dispatch_no"],
    ["data", "resend_dispatch_no"],
    ["data", "custody", "credits"],
  ],
} as const;

/** 读取一页结算退件案件。 */
export async function fetchSettlementReturns(
  host: AdminSessionHost,
  filters: SettlementReturnFilters,
  page: bigint,
  pageSize: number,
  signal: AbortSignal,
): Promise<Result<AdminSettlementReturnListResponse, string>> {
  // 地址栏允许完整 i64；超过安全整数时不能先转成 number 再发送，否则页码会被舍入。
  if (page < 1n || page > BigInt(MAX_JSON_SAFE_INTEGER)) {
    return err("admin_pagination_bounds");
  }
  const bounds = validate(
    Number(page),
    pageSize,
    ADMIN_SETTLEMENT_RETURN_MAX_PAGE_SIZE,
    ADMIN_SETTLEMENT_RETURN_MAX_OFFSET,
  );
  if (!bounds.ok) {
    return err(bounds.error);
  }
  const url = listUrl(filters, page, pageSize);
  if (url === null) {
    return err("invalid_request");
  }
  const response = await sendGet(host, url, signal);
  if (!response.ok) {
    return err(response.error);
  }
  const received = response.value;
  if (!responseOk(received)) {
    return err(await responseError(received, signal, host));
  }
  const parsed = received.json(ADMIN_SETTLEMENT_RETURN_LIST_FIELD_POLICY);
  if (!parsed.ok) {
    return err(invalidResponseError(parsed.error));
  }
  const payload = decode(apiResponseSchema(AdminSettlementReturnListResponseSchema), parsed.value);
  if (!payload.ok) {
    return err(invalidResponseError(payload));
  }
  if (
    !validSuccessCode(payload.value.code) ||
    !validPage(payload.value.data, filters, page, pageSize)
  ) {
    return err(invalidResponseError("settlement return page"));
  }
  return ok(payload.value.data);
}

/** 读取单个案件详情；响应必须正是请求的案件。 */
export async function fetchSettlementReturn(
  host: AdminSessionHost,
  returnId: string,
  signal: AbortSignal,
): Promise<Result<AdminSettlementReturnDetailResponse, string>> {
  if (!validUuid(returnId)) {
    return err("invalid_settlement_return_id");
  }
  const response = await sendGet(host, `/api/v1/admin/settlement-returns/${returnId}`, signal);
  if (!response.ok) {
    return err(response.error);
  }
  const received = response.value;
  if (!responseOk(received)) {
    return err(await responseError(received, signal, host));
  }
  const parsed = received.json(ADMIN_SETTLEMENT_RETURN_DETAIL_FIELD_POLICY);
  if (!parsed.ok) {
    return err(invalidResponseError(parsed.error));
  }
  const payload = decode(
    apiResponseSchema(AdminSettlementReturnDetailResponseSchema),
    parsed.value,
  );
  if (!payload.ok) {
    return err(invalidResponseError(payload));
  }
  if (!validSuccessCode(payload.value.code) || !validDetail(payload.value.data, returnId)) {
    return err(invalidResponseError("settlement return detail"));
  }
  return ok(payload.value.data);
}

/** 冻结的一次退件处理意图；路由、正文与回执期望都由这一份值派生。 */
export interface SettlementReturnActionIntent {
  readonly returnId: string;
  readonly expectedVersion: number;
  readonly action: string;
}

// 只有本模块的工厂能构造提交；外部同形对象无法把另一份意图的正文混进本次路由与回执核对。
const settlementReturnActionSubmissionBrand: unique symbol = Symbol(
  "admin.settlement-return-action-submission",
);

/** 一次未决命令的提交：正文在首次执行时由同一意图固定，重试复用同一份字节。 */
export interface SettlementReturnActionSubmission {
  readonly [settlementReturnActionSubmissionBrand]: true;
  readonly intent: SettlementReturnActionIntent;
  readonly prepare: (commandId: string) => Result<PreparedJsonCommand, AdminWriteNotSent>;
}

/** 冻结提交意图并创建正文工厂；执行体第一次运行时结合 command ID 生成正文。 */
export function prepareSettlementReturnActionOnce(
  intent: SettlementReturnActionIntent,
): SettlementReturnActionSubmission {
  // 复制并冻结，提交后不再读取确认框草稿。
  const frozen = Object.freeze({ ...intent });
  return Object.freeze({
    [settlementReturnActionSubmissionBrand]: true as const,
    intent: frozen,
    prepare: prepareJsonCommandOnce((commandId) => {
      const command = {
        command_id: commandId,
        expected_version: frozen.expectedVersion,
      };
      return frozen.action === "recheck" ? command : { ...command, action: frozen.action };
    }),
  });
}

/**
 * 提交处理动作；`recheck` 走只读重新核验路由，其余动作走 `handle`。同一 `command_id` 原样重试
 * 由服务端重放首次结果。
 *
 * 路由、正文与回执期望都取自同一份提交意图，命令 UUID 在发送前校验。
 */
export async function submitSettlementReturnAction(
  host: AdminSessionHost,
  commandId: string,
  submission: SettlementReturnActionSubmission,
  signal: AbortSignal,
): Promise<Result<AdminSettlementReturnCommandResponse, AdminWriteFailure>> {
  const intent = submission.intent;
  if (
    commandId === NIL_UUID ||
    !validUuid(commandId) ||
    !validUuid(intent.returnId) ||
    !Number.isSafeInteger(intent.expectedVersion) ||
    intent.expectedVersion < 1 ||
    intent.expectedVersion > MAX_JSON_SAFE_INTEGER ||
    !inClosedSet(ADMIN_SETTLEMENT_RETURN_ACTIONS, intent.action)
  ) {
    return err(writeNotSent("invalid_settlement_return_command"));
  }
  const prepared = submission.prepare(commandId);
  if (!prepared.ok) {
    return err(prepared.error);
  }
  const result =
    intent.action === "recheck"
      ? await postCommand(
          host,
          `/api/v1/admin/settlement-returns/${intent.returnId}/recheck`,
          prepared.value,
          signal,
        )
      : await postCommand(
          host,
          `/api/v1/admin/settlement-returns/${intent.returnId}/handle`,
          prepared.value,
          signal,
        );
  if (!result.ok) {
    return result;
  }
  if (
    !validCommandResult(
      result.value,
      commandId,
      intent.returnId,
      intent.action,
      intent.expectedVersion,
    )
  ) {
    return err(writeOutcomeUnknown(invalidResponseError("settlement return command")));
  }
  return result;
}

async function postCommand(
  host: AdminSessionHost,
  url: string,
  prepared: PreparedJsonCommand,
  signal: AbortSignal,
): Promise<Result<AdminSettlementReturnCommandResponse, AdminWriteFailure>> {
  const response = await sendPreparedJsonWrite(
    host,
    { url, method: "POST", endpoint: "general" },
    prepared,
    signal,
  );
  if (!response.ok) {
    return response;
  }
  const payload = decodeAdminReceipt(
    response.value,
    apiResponseSchema(AdminSettlementReturnCommandResponseSchema),
    ADMIN_SETTLEMENT_RETURN_COMMAND_RESPONSE_FIELD_POLICY,
  );
  if (!payload.ok) {
    return err(writeOutcomeUnknown(payload.error));
  }
  if (!validSuccessCode(payload.value.code)) {
    return err(writeOutcomeUnknown(invalidResponseError("settlement return command")));
  }
  return ok(payload.value.data);
}

/** 列表请求地址；筛选只接受契约闭集取值，闭集外的值返回 `null` 而不是被拼进查询串。 */
export function listUrl(
  filters: SettlementReturnFilters,
  page: bigint,
  pageSize: number,
): string | null {
  let url = `/api/v1/admin/settlement-returns?page=${page}&page_size=${pageSize}`;
  if (filters.openOnly) {
    url += "&open_only=true";
  }
  const values: readonly (readonly [string, string, readonly string[]])[] = [
    ["status", filters.status, ADMIN_SETTLEMENT_RETURN_STATUSES],
    ["match_state", filters.matchState, ADMIN_SETTLEMENT_RETURN_MATCH_STATES],
    ["obligation_kind", filters.obligationKind, ADMIN_SETTLEMENT_RETURN_OBLIGATION_KINDS],
    ["world", filters.world, RETURN_WORLDS],
  ];
  for (const [key, value, allowed] of values) {
    if (value === "") {
      continue;
    }
    if (!inClosedSet(allowed, value)) {
      return null;
    }
    url += `&${key}=${value}`;
  }
  return url;
}

/** 分页回显、每行摘要与筛选回显都必须成立：服务端不能返回不在筛选内的案件。 */
export function validPage(
  data: AdminSettlementReturnListResponse,
  filters: SettlementReturnFilters,
  page: bigint,
  pageSize: number,
): boolean {
  if (page < 1n || page > BigInt(MAX_JSON_SAFE_INTEGER)) {
    return false;
  }
  const ids = new Set<string>();
  return (
    validPageEcho(
      Number(page),
      pageSize,
      data.pagination.page,
      data.pagination.page_size,
      data.pagination.total,
      data.returns.length,
    ) &&
    data.returns.every((item) => {
      if (ids.has(item.return_id)) {
        return false;
      }
      ids.add(item.return_id);
      return (
        validSummary(item) &&
        (!filters.openOnly || item.closed_at === null) &&
        (filters.status === "" || item.status === filters.status) &&
        (filters.matchState === "" || item.match_state === filters.matchState) &&
        (filters.obligationKind === "" || item.obligation_kind === filters.obligationKind) &&
        (filters.world === "" || item.world_id === filters.world)
      );
    })
  );
}

/** 案件摘要的闭集与形状校验；结案时刻与已结案状态必须一致。 */
export function validSummary(item: AdminSettlementReturnSummaryResponse): boolean {
  const closed = inClosedSet(ADMIN_SETTLEMENT_RETURN_CLOSED_STATUSES, item.status);
  return (
    validUuid(item.return_id) &&
    item.case_version >= 1 &&
    item.case_version <= MAX_JSON_SAFE_INTEGER &&
    inClosedSet(ADMIN_SETTLEMENT_RETURN_STATUSES, item.status) &&
    inClosedSet(ADMIN_SETTLEMENT_RETURN_MATCH_STATES, item.match_state) &&
    inClosedSet(ADMIN_SETTLEMENT_RETURN_REASONS, item.reason_code) &&
    (item.world_id === "main" || item.world_id === "isekai") &&
    validSafeText(item.external_mail_id, 256) &&
    validAdminTime(item.received_at) &&
    (item.obligation_id === null || validUuid(item.obligation_id)) &&
    (item.obligation_kind === null ||
      inClosedSet(ADMIN_SETTLEMENT_RETURN_OBLIGATION_KINDS, item.obligation_kind)) &&
    (item.auction_id === null || validUuid(item.auction_id)) &&
    (item.lot_id === null || validUuid(item.lot_id)) &&
    (item.delivery_id === null || validUuid(item.delivery_id)) &&
    (item.dispatch_no === null || item.dispatch_no >= 1) &&
    (item.recipient === null || validSafeText(item.recipient, 256)) &&
    validCount(item.credits) &&
    validCount(item.cod) &&
    (item.plan === null || inClosedSet(ADMIN_SETTLEMENT_RETURN_PLANS, item.plan)) &&
    inClosedSet(ADMIN_SETTLEMENT_RETURN_ASSET_LOCATIONS, item.asset_location) &&
    validAdminTime(item.created_at) &&
    validAdminTime(item.updated_at) &&
    validOptionalTime(item.closed_at) &&
    (item.closed_at !== null) === closed
  );
}

/**
 * 详情：摘要正是请求的案件；事件与派发按版本/编号严格递增；动作闭集被允许动作与阻断原因恰好
 * 覆盖一次。
 */
export function validDetail(
  detail: AdminSettlementReturnDetailResponse,
  requested: string,
): boolean {
  const actions = new Set<string>();
  let partition = true;
  for (const action of [
    ...detail.allowed_actions,
    ...detail.blockers.map((blocker) => blocker.action),
  ]) {
    if (!inClosedSet(ADMIN_SETTLEMENT_RETURN_ACTIONS, action) || actions.has(action)) {
      partition = false;
      break;
    }
    actions.add(action);
  }
  if (partition) {
    partition =
      actions.size === ADMIN_SETTLEMENT_RETURN_ACTIONS.length &&
      detail.blockers.every((blocker) =>
        inClosedSet(ADMIN_SETTLEMENT_RETURN_BLOCKERS, blocker.blocker),
      );
  }
  return (
    validSummary(detail.summary) &&
    detail.summary.return_id === requested &&
    partition &&
    increasing(detail.events.map((event) => event.case_version)) &&
    detail.events.every(validEvent) &&
    increasing(detail.dispatches.map((dispatch) => dispatch.dispatch_no)) &&
    detail.dispatches.every(validDispatch) &&
    (detail.take === null || validTake(detail.take)) &&
    (detail.resend_dispatch_no === null || detail.resend_dispatch_no >= 2) &&
    (detail.custody === null || validCustody(detail.custody)) &&
    (!detail.recheck_unavailable || detail.summary.status === "recheck_pending") &&
    (!detail.take_unavailable || detail.summary.status === "take_pending") &&
    // 结果未知必有闭集内的原因，其他状态没有。
    (detail.take_unknown_reason !== null) === (detail.summary.status === "take_unknown") &&
    (detail.take_unknown_reason === null ||
      inClosedSet(ADMIN_SETTLEMENT_RETURN_TAKE_UNKNOWN_REASONS, detail.take_unknown_reason))
  );
}

/** 命令结果必须对应本次命令身份、案件与动作，版本只会前进。 */
export function validCommandResult(
  result: AdminSettlementReturnCommandResponse,
  commandId: string,
  returnId: string,
  action: string,
  expectedVersion: number,
): boolean {
  return (
    result.command_id === commandId &&
    result.return_id === returnId &&
    result.action === action &&
    result.case_version > expectedVersion &&
    result.case_version <= MAX_JSON_SAFE_INTEGER &&
    inClosedSet(ADMIN_SETTLEMENT_RETURN_STATUSES, result.status)
  );
}

function increasing(values: Iterable<number>): boolean {
  let previous = 0;
  for (const value of values) {
    if (!(value > previous)) {
      return false;
    }
    previous = value;
  }
  return true;
}

function validEvent(event: AdminSettlementReturnEventResponse): boolean {
  return (
    inClosedSet(ADMIN_SETTLEMENT_RETURN_EVENT_KINDS, event.event_kind) &&
    validAdminTime(event.occurred_at) &&
    (event.actor_id === null || validSafeText(event.actor_id, 128)) &&
    (event.command_id === null || validUuid(event.command_id)) &&
    isJsonObject(event.details)
  );
}

function validDispatch(dispatch: AdminSettlementReturnDispatchResponse): boolean {
  return (
    validUuid(dispatch.delivery_id) &&
    (dispatch.delivery_status === null || validStatus(dispatch.delivery_status)) &&
    validOptionalTime(dispatch.sending_at) &&
    validOptionalTime(dispatch.completed_at) &&
    (dispatch.authorized_by === null || validUuid(dispatch.authorized_by)) &&
    (dispatch.dispatch_no === 1) === (dispatch.authorized_by === null)
  );
}

function validTake(take: AdminSettlementReturnTakeResponse): boolean {
  return (
    inClosedSet(ADMIN_SETTLEMENT_RETURN_TAKE_STATES, take.execution_state) &&
    (take.delivery_id === null || validUuid(take.delivery_id)) &&
    (take.delivery_status === null || validStatus(take.delivery_status)) &&
    (take.failure_code === null || validSafeText(take.failure_code, 96)) &&
    validOptionalTime(take.proven_at)
  );
}

function validCustody(custody: AdminSettlementReturnCustodyResponse): boolean {
  return (
    validSafeText(custody.beneficiary, 256) &&
    validCount(custody.credits) &&
    custody.equipment_ids.every(validEquipmentId)
  );
}

/** 装备编号是 1 到 32 字节的 ASCII 十进制文本。 */
function validEquipmentId(id: string): boolean {
  if (id.length < 1 || id.length > 32) {
    return false;
  }
  for (let index = 0; index < id.length; index += 1) {
    const code = id.charCodeAt(index);
    if (code < 0x30 || code > 0x39) {
      return false;
    }
  }
  return true;
}

function inClosedSet(values: readonly string[], value: string): boolean {
  return values.some((candidate) => candidate === value);
}

/** `serde_json::Value::is_object`：普通对象，不是 `null` 或数组。 */
function isJsonObject(value: unknown): value is Readonly<Record<string, unknown>> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** gloo 的 `Response::ok`：状态码 200 到 299。 */
function responseOk(response: { readonly status: number }): boolean {
  return response.status >= 200 && response.status <= 299;
}
