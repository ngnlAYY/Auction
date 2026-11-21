// Admin 邮件轮询与入站审查 HTTP 客户端。
// 浏览器只读取轮询状态和入站安全摘要；暂停动作携带服务端快照中的 state version，
// 立即轮询（`POST /api/v1/admin/mail/polls:trigger`）使用单个 command ID 唤醒全部 Worker
// poller，浏览器不推导邮件处理状态。

import type {
  AdminMailPollingRequest,
  AdminMailPollTriggerRequest,
} from "@auction/contracts/dto/admin/commands";
import {
  ADMIN_MAIL_INTAKE_DETAIL_FIELD_POLICY,
  ADMIN_MAIL_RECEIPT_FIELD_POLICY,
  ADMIN_MAIL_STATE_FIELD_POLICY,
  type AdminMailIntakeDeliveryResponse,
  type AdminMailIntakeDetailResponse,
  AdminMailIntakeDetailResponseSchema,
  type AdminMailIntakeListResponse,
  AdminMailIntakeListResponseSchema,
  type AdminMailIntakeSummaryResponse,
  type AdminMailPollingResponse,
  AdminMailPollTriggerResponseSchema,
} from "@auction/contracts/dto/admin/mail";
import { apiResponseSchema } from "@auction/contracts/dto/response";
import { decode } from "@auction/contracts/wire";
import { err, ok, type Result } from "@auction/web-shared/result";

import {
  type AdminWriteFailure,
  type AdminWriteNotSent,
  decodeAdminReceipt,
  invalidResponseError,
  responseError,
  writeNotSent,
  writeOutcomeUnknown,
} from "./failure";
import {
  acceptedPollers,
  decodeMailIntakeDetail,
  decodeMailIntakePage,
  decodeMailState,
  StrictMailStateSchema,
} from "./mail-response";
import { validate as validatePagination } from "./pagination";
import type { AdminSessionHost } from "./session";
import {
  type PreparedJsonCommand,
  prepareJsonCommandOnce,
  sendGet,
  sendPreparedJsonWrite,
} from "./transport";

/** 入站邮件摘要。 */
export type AdminMailIntake = AdminMailIntakeSummaryResponse;
/** 入站邮件关联的 delivery 安全投影。 */
export type AdminMailIntakeDelivery = AdminMailIntakeDeliveryResponse;
/** 入站邮件详情。 */
export type AdminMailIntakeDetail = AdminMailIntakeDetailResponse;
/** 入站邮件列表的一页。 */
export type AdminMailIntakePage = AdminMailIntakeListResponse;
/** 邮件轮询状态。 */
export type AdminMailPolling = AdminMailPollingResponse;

/** 入站列表的服务端筛选；空串表示该维度不筛选。 */
export interface AdminMailIntakeQuery {
  worldId: string;
  kind: string;
  status: string;
}

/** 入站邮件摘要与 URL 筛选共用的类型闭集。 */
export const ADMIN_MAIL_INTAKE_KIND_VALUES: readonly string[] = [
  "equipment",
  "bid",
  "delivery",
  "registration",
  "rebind",
  "rekey",
  "unknown",
];

/** 与 Rust `Default` 相同：三个筛选维度都为空。 */
export function adminMailIntakeQueryDefault(): AdminMailIntakeQuery {
  return { worldId: "", kind: "", status: "" };
}

/** 读取轮询状态并强制显式要求诊断扩展。 */
export async function fetchMailState(
  host: AdminSessionHost,
  mailboxKey: string,
  signal: AbortSignal,
): Promise<Result<AdminMailPolling, string>> {
  const encoded = encodeQuery(mailboxKey);
  const response = await sendGet(
    host,
    `/api/v1/admin/mail?mailbox_key=${encoded}&include_diagnostics=true`,
    signal,
  );
  if (!response.ok) {
    return err(response.error);
  }
  const received = response.value;
  if (!responseOk(received)) {
    return err(await responseError(received, signal, host));
  }
  const payload = decodeAdminReceipt(
    received,
    apiResponseSchema(StrictMailStateSchema),
    ADMIN_MAIL_STATE_FIELD_POLICY,
  );
  if (!payload.ok) {
    return err(payload.error);
  }
  const polling = decodeMailState(payload.value.data);
  if (!polling.ok) {
    return err(invalidResponseError(polling.error));
  }
  if (polling.value.mailbox_key !== mailboxKey || polling.value.diagnostics === null) {
    return err("invalid_response");
  }
  return ok(polling.value);
}

/** 读取一页入站邮件摘要；筛选条件必须在响应中原样回显。 */
export async function fetchMailIntakes(
  host: AdminSessionHost,
  query: AdminMailIntakeQuery,
  page: bigint,
  pageSize: number,
  signal: AbortSignal,
): Promise<Result<AdminMailIntakePage, string>> {
  if (page < 1n || page > 2_147_483_647n) {
    return err("invalid_pagination");
  }
  const requestedPage = Number(page);
  const bounds = validatePagination(requestedPage, pageSize, 100, 2_147_483_647);
  if (!bounds.ok) {
    return bounds;
  }
  // 响应校验必须使用与请求相同的筛选值；await 期间调用方修改原对象不能改写本次预期。
  const filter: AdminMailIntakeQuery = Object.freeze({
    worldId: query.worldId,
    kind: query.kind,
    status: query.status,
  });
  let url = `/api/v1/admin/mail/intake?page=${page}&page_size=${pageSize}`;
  url = appendQuery(url, "world_id", filter.worldId);
  url = appendQuery(url, "kind", filter.kind);
  url = appendQuery(url, "status", filter.status);
  const response = await sendGet(host, url, signal);
  if (!response.ok) {
    return err(response.error);
  }
  const received = response.value;
  if (!responseOk(received)) {
    return err(await responseError(received, signal, host));
  }
  const parsed = received.json();
  if (!parsed.ok) {
    return err(invalidResponseError(parsed.error));
  }
  const payload = decode(apiResponseSchema(AdminMailIntakeListResponseSchema), parsed.value);
  if (!payload.ok) {
    return err(invalidResponseError(payload));
  }
  const decoded = decodeMailIntakePage(
    payload.value.data,
    filter,
    requestedPage,
    pageSize,
    ADMIN_MAIL_INTAKE_KIND_VALUES,
  );
  if (!decoded.ok) {
    return err(invalidResponseError(decoded.error));
  }
  return ok(decoded.value);
}

/** 读取入站详情；versions 会参与后续重排确认，因此按严格词法解析。 */
export async function fetchMailIntakeDetail(
  host: AdminSessionHost,
  intakeId: string,
  signal: AbortSignal,
): Promise<Result<AdminMailIntakeDetail, string>> {
  const response = await sendGet(
    host,
    `/api/v1/admin/mail/intake/${encodeURIComponent(intakeId)}`,
    signal,
  );
  if (!response.ok) {
    return err(response.error);
  }
  const received = response.value;
  if (!responseOk(received)) {
    return err(await responseError(received, signal, host));
  }
  const payload = decodeAdminReceipt(
    received,
    apiResponseSchema(AdminMailIntakeDetailResponseSchema),
    ADMIN_MAIL_INTAKE_DETAIL_FIELD_POLICY,
  );
  if (!payload.ok) {
    return err(payload.error);
  }
  const detail = decodeMailIntakeDetail(payload.value.data, ADMIN_MAIL_INTAKE_KIND_VALUES);
  if (!detail.ok) {
    return err(invalidResponseError(detail.error));
  }
  if (detail.value.id !== intakeId) {
    return err("invalid_response");
  }
  return ok(detail.value);
}

/** 暂停或恢复命令的冻结参数；发送正文与回执校验只读这一份值。 */
export interface AdminMailPauseCommand {
  readonly mailboxKey: string;
  readonly expectedStateVersion: number;
  readonly paused: boolean;
}

/** 已冻结正文与期望值的暂停或恢复命令。 */
export interface PreparedMailPauseCommand {
  readonly command: AdminMailPauseCommand;
  readonly body: PreparedJsonCommand;
}

/**
 * 冻结暂停或恢复命令的参数；执行体第一次运行时才结合 command ID 生成并缓存正文。
 * 正文与回执校验共用同一个不可变参数对象，避免为邮箱 A 准备正文却按邮箱 B 校验回执。
 */
export function prepareMailPauseCommand(
  mailboxKey: string,
  expectedStateVersion: number,
  paused: boolean,
): (commandId: string) => Result<PreparedMailPauseCommand, AdminWriteNotSent> {
  const parameters: AdminMailPauseCommand = Object.freeze({
    mailboxKey,
    expectedStateVersion:
      typeof expectedStateVersion === "number" ? expectedStateVersion : Number.NaN,
    paused,
  });
  const prepare = prepareJsonCommandOnce((commandId) => {
    const command: AdminMailPollingRequest = {
      command_id: commandId,
      mailbox_key: parameters.mailboxKey,
      paused: parameters.paused,
      expected_state_version: parameters.expectedStateVersion,
    };
    return command;
  });
  return (commandId) => {
    if (
      !Number.isSafeInteger(parameters.expectedStateVersion) ||
      parameters.expectedStateVersion < 0 ||
      parameters.expectedStateVersion >= Number.MAX_SAFE_INTEGER
    ) {
      return err(writeNotSent("invalid_mail_pause_command"));
    }
    const prepared = prepare(commandId);
    if (!prepared.ok) {
      return err(prepared.error);
    }
    return ok(Object.freeze({ command: parameters, body: prepared.value }));
  };
}

/** 暂停或恢复轮询；成功回执必须推进一个版本并保持目标邮箱与目标状态。 */
export async function setMailPaused(
  host: AdminSessionHost,
  prepared: PreparedMailPauseCommand,
  signal: AbortSignal,
): Promise<Result<AdminMailPolling, AdminWriteFailure>> {
  const response = await sendPreparedJsonWrite(
    host,
    { url: "/api/v1/admin/mail/polling", method: "PUT", endpoint: "general" },
    prepared.body,
    signal,
  );
  if (!response.ok) {
    return err(response.error);
  }
  const payload = decodeAdminReceipt(
    response.value,
    apiResponseSchema(StrictMailStateSchema),
    ADMIN_MAIL_RECEIPT_FIELD_POLICY,
  );
  if (!payload.ok) {
    return err(writeOutcomeUnknown(payload.error));
  }
  const updated = decodeMailState(payload.value.data);
  if (!updated.ok) {
    return err(writeOutcomeUnknown(invalidResponseError(updated.error)));
  }
  const command = prepared.command;
  if (
    updated.value.mailbox_key !== command.mailboxKey ||
    updated.value.paused !== command.paused ||
    updated.value.state_version !== command.expectedStateVersion + 1
  ) {
    return err(writeOutcomeUnknown("invalid_response"));
  }
  return ok(updated.value);
}

/** 已冻结正文与命令 ID 的立即轮询命令。 */
export interface PreparedMailPollTriggerCommand {
  readonly commandId: string;
  readonly body: PreparedJsonCommand;
}

/** 冻结立即轮询命令；执行体第一次运行时才结合 command ID 生成并缓存正文。 */
export function prepareMailPollTriggerCommand(): (
  commandId: string,
) => Result<PreparedMailPollTriggerCommand, AdminWriteNotSent> {
  const prepare = prepareJsonCommandOnce((commandId) => {
    const command: AdminMailPollTriggerRequest = { command_id: commandId };
    return command;
  });
  return (commandId) => {
    const prepared = prepare(commandId);
    if (!prepared.ok) {
      return err(prepared.error);
    }
    return ok(Object.freeze({ commandId, body: prepared.value }));
  };
}

/** 立即唤醒全部 Worker poller；只接受 202 与回显本命令 ID 的正式回执。 */
export async function triggerMailPoll(
  host: AdminSessionHost,
  prepared: PreparedMailPollTriggerCommand,
  signal: AbortSignal,
): Promise<Result<number, AdminWriteFailure>> {
  const response = await sendPreparedJsonWrite(
    host,
    { url: "/api/v1/admin/mail/polls:trigger", method: "POST", endpoint: "general" },
    prepared.body,
    signal,
  );
  if (!response.ok) {
    return err(response.error);
  }
  const received = response.value;
  if (received.status !== 202) {
    return err(writeOutcomeUnknown(invalidResponseError("mail_poll_trigger_status")));
  }
  const payload = decodeAdminReceipt(
    received,
    apiResponseSchema(AdminMailPollTriggerResponseSchema),
    ADMIN_MAIL_RECEIPT_FIELD_POLICY,
  );
  if (!payload.ok) {
    return err(writeOutcomeUnknown(payload.error));
  }
  const pollers = acceptedPollers(payload.value, prepared.commandId);
  if (pollers === null) {
    return err(writeOutcomeUnknown(invalidResponseError("mail_poll_trigger_receipt")));
  }
  return ok(pollers);
}

function encodeQuery(value: string): string {
  return new URLSearchParams([["", value]]).toString().slice(1);
}

function appendQuery(url: string, key: string, value: string): string {
  return value === "" ? url : `${url}&${key}=${encodeQuery(value)}`;
}

function responseOk(response: { readonly status: number }): boolean {
  return response.status >= 200 && response.status <= 299;
}
