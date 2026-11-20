// Admin mail DTO：邮件轮询状态、入站邮件摘要与详情、展示正文与装备读取诊断。

import * as v from "valibot";
import {
  int32,
  integer,
  optionalNullable,
  strictStruct,
  struct,
  uint8,
  uint16,
  uint32,
  unsignedInteger,
} from "../../../wire";
import type { AdminPaginationResponse } from "../pagination";
import { AdminPaginationResponseSchema } from "../pagination";
import { AdminMailPollingDiagnosticsResponseSchema } from "./progress";

/** Admin 邮件轮询状态安全响应；不包含外部 Cookie 或原始游标。 */
export const AdminMailPollingResponseSchema = strictStruct({
  mailbox_key: v.string(),
  last_poll_at: v.string(),
  next_poll_at: v.string(),
  consecutive_failures: int32,
  paused: v.boolean(),
  state_version: integer,
  updated_at: v.string(),
  /** 仅显式选择诊断时出现；旧响应和暂停/恢复命令保持原字段集合。 */
  diagnostics: optionalNullable(AdminMailPollingDiagnosticsResponseSchema),
});

export type AdminMailPollingResponse = v.InferOutput<typeof AdminMailPollingResponseSchema>;

export const AdminMailStateResponseSchema = struct({
  polling: AdminMailPollingResponseSchema,
});

export type AdminMailStateResponse = v.InferOutput<typeof AdminMailStateResponseSchema>;

/** 邮件状态读取中的 `paused` 与 `state_version` 决定暂停或恢复及其预期版本；
 * 解析完整响应原文时必须传入。轮询响应允许扩展字段，因此只按路径拒绝已知整数字段的
 * 小数、指数与 `-0`，不整份限制未知值的词法。 */
export const ADMIN_MAIL_STATE_FIELD_POLICY = {
  uniqueKeys: "all",
  integerPaths: [
    ["data", "polling", "consecutive_failures"],
    ["data", "polling", "state_version"],
  ],
} as const;

export const AdminMailPollTriggerResponseSchema = struct({
  command_id: v.string(),
  pollers: integer,
});

export type AdminMailPollTriggerResponse = v.InferOutput<typeof AdminMailPollTriggerResponseSchema>;

/** 暂停/恢复与立即轮询回执用于判定命令是否被接受；解析原文时必须传入。
 * 回执里的具型数字全部是整数，因此可以整份拒绝小数、指数与 `-0`。 */
export const ADMIN_MAIL_RECEIPT_FIELD_POLICY = {
  uniqueKeys: "all",
  numbers: "integers",
} as const;

/** Admin 入站邮件列表安全摘要。 */
export const AdminMailIntakeSummaryResponseSchema = struct({
  id: v.string(),
  mailbox_key: v.string(),
  external_mail_id: v.string(),
  world_id: v.string(),
  kind: v.string(),
  status: v.string(),
  decision_code: v.string(),
  received_at: v.string(),
  processed_at: v.string(),
  sender: optionalNullable(v.string()),
  subject: optionalNullable(v.string()),
  has_message_display: v.boolean(),
});

export type AdminMailIntakeSummaryResponse = v.InferOutput<
  typeof AdminMailIntakeSummaryResponseSchema
>;

/** Admin 邮件入站列表数字分页。 */
export type AdminMailPaginationResponse = AdminPaginationResponse;

export const AdminMailIntakeListResponseSchema = struct({
  intakes: v.pipe(v.array(AdminMailIntakeSummaryResponseSchema), v.readonly()),
  pagination: AdminPaginationResponseSchema,
});

export type AdminMailIntakeListResponse = v.InferOutput<typeof AdminMailIntakeListResponseSchema>;

/** Admin 入站邮件详情的固定元数据。
 * 严格闭集与 Admin 浏览器端详情 decoder 的闭集语义对齐；只收紧反序列化。 */
export const AdminMailIntakeMetadataResponseSchema = strictStruct({
  auction_id: v.string(),
  submission_id: v.string(),
  user_id: v.string(),
  added_count: int32,
  existing_count: int32,
  preview_revision: integer,
  attachment_count: int32,
  owner_result_code: v.string(),
  reason_code: v.string(),
  warning_codes: v.pipe(v.array(v.string()), v.readonly()),
  delivery_ids: v.pipe(v.array(v.string()), v.readonly()),
});

export type AdminMailIntakeMetadataResponse = v.InferOutput<
  typeof AdminMailIntakeMetadataResponseSchema
>;

/** Admin 入站邮件关联 delivery 的安全重发响应；不包含 payload 或密钥材料。 */
export const AdminMailIntakeDeliveryResponseSchema = strictStruct({
  id: v.string(),
  operation: v.string(),
  recipient: v.string(),
  status: v.string(),
  fencing_version: integer,
  attempt_count: uint32,
  failure_code: optionalNullable(v.string()),
  /** 具名隔离原因独立于人工填写的失败码；旧响应省略时按 None 读取。 */
  reason_code: optionalNullable(v.string()),
  external_reference: optionalNullable(v.string()),
});

export type AdminMailIntakeDeliveryResponse = v.InferOutput<
  typeof AdminMailIntakeDeliveryResponseSchema
>;

/** Admin 入站邮件原始信息的安全展示响应。 */
export const AdminMailDisplayResponseSchema = strictStruct({
  schema_version: uint8,
  redaction_version: uint8,
  body: v.string(),
  attachment_ids: v.pipe(v.array(v.string()), v.readonly()),
  credits: integer,
  cod: integer,
});

export type AdminMailDisplayResponse = v.InferOutput<typeof AdminMailDisplayResponseSchema>;

/** Admin 入站邮件装备读取诊断的独立安全响应。 */
export const AdminMailEquipmentDiagnosticResponseSchema = strictStruct({
  schema_version: uint8,
  equipment_id: v.string(),
  attachment_position: uint16,
  stage: v.string(),
  category: v.string(),
  detail_code: optionalNullable(v.string()),
  http_status: optionalNullable(uint16),
  elapsed_millis: optionalNullable(unsignedInteger),
  evidence_id: optionalNullable(v.string()),
  evidence_status: v.string(),
  retention_error: optionalNullable(v.string()),
});

export type AdminMailEquipmentDiagnosticResponse = v.InferOutput<
  typeof AdminMailEquipmentDiagnosticResponseSchema
>;

/** Admin 入站邮件详情响应。
 * 详情 wire 是闭集（与列表摘要的兼容扩展语义不同），未知字段一律拒绝。 */
export const AdminMailIntakeDetailResponseSchema = strictStruct({
  id: v.string(),
  mailbox_key: v.string(),
  external_mail_id: v.string(),
  world_id: v.string(),
  kind: v.string(),
  status: v.string(),
  decision_code: v.string(),
  received_at: v.string(),
  processed_at: v.string(),
  sender: optionalNullable(v.string()),
  subject: optionalNullable(v.string()),
  metadata: AdminMailIntakeMetadataResponseSchema,
  deliveries: v.pipe(v.array(AdminMailIntakeDeliveryResponseSchema), v.readonly()),
  message_display: optionalNullable(AdminMailDisplayResponseSchema),
  equipment_diagnostic: optionalNullable(AdminMailEquipmentDiagnosticResponseSchema),
});

export type AdminMailIntakeDetailResponse = v.InferOutput<
  typeof AdminMailIntakeDetailResponseSchema
>;

/** 入站详情中的 `deliveries[*].fencing_version` 参与重排确认；解析完整响应原文时必须传入，
 * 拒绝重复键与小数、指数、`-0` 等非整数词法。 */
export const ADMIN_MAIL_INTAKE_DETAIL_FIELD_POLICY = {
  uniqueKeys: "all",
  numbers: "integers",
} as const;
