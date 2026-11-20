// 告警台 HTTP 契约：活动/历史列表、确认、静默与导航待办计数。

import * as v from "valibot";
import { integer, optionalNullable, strictStruct, uint32 } from "../../wire";

/** 单条告警的安全投影；不含原始错误正文、收件人或凭据。 */
export const AdminAlertEntryResponseSchema = strictStruct({
  /** 64 位小写十六进制 fingerprint。 */
  id: v.string(),
  kind: v.string(),
  scope: v.string(),
  source: v.string(),
  /** `urgent` 或 `digest`。 */
  severity: v.string(),
  active: v.boolean(),
  /** 仅 `include=incident_generation` 投影返回。同一指纹的告警恢复后再出现时递增。
   * DTO 按 Rust 的 `Option` 规则接受缺键；旧 API 在列表投影里要求它存在且非负。 */
  incident_generation: optionalNullable(integer),
  count: integer,
  age_seconds: integer,
  first_seen_at: v.string(),
  last_seen_at: v.string(),
  resolved_at: optionalNullable(v.string()),
  last_notified_at: optionalNullable(v.string()),
  next_notify_at: optionalNullable(v.string()),
  acknowledged_at: optionalNullable(v.string()),
  acknowledged_by: optionalNullable(v.string()),
  silenced_until: optionalNullable(v.string()),
  /** `ops/README.md` 处置手册锚点（不含 `#`）。 */
  runbook_anchor: v.string(),
});

/** 单条告警的安全投影；不含原始错误正文、收件人或凭据。 */
export type AdminAlertEntryResponse = v.InferOutput<typeof AdminAlertEntryResponseSchema>;

/** Admin 告警列表页。 */
export const AdminAlertListResponseSchema = strictStruct({
  alerts: v.pipe(v.array(AdminAlertEntryResponseSchema), v.readonly()),
  page: integer,
  page_size: integer,
  total: integer,
  active_total: integer,
  resolved_total: integer,
});

/** 一页告警条目与页头分类计数。 */
export type AdminAlertListResponse = v.InferOutput<typeof AdminAlertListResponseSchema>;

/** webhook 投递的安全运营投影；不含冻结 JSON、收件人或凭据。 */
export const AdminAlertWebhookEntryResponseSchema = strictStruct({
  id: v.string(),
  status: v.string(),
  kind: v.string(),
  scope: v.string(),
  source: v.string(),
  attempt_count: uint32,
  next_attempt_at: optionalNullable(v.string()),
  sending_at: optionalNullable(v.string()),
  completed_at: optionalNullable(v.string()),
  failure_code: optionalNullable(v.string()),
  created_at: v.string(),
});

/** webhook 投递的安全运营投影；不含冻结 JSON、收件人或凭据。 */
export type AdminAlertWebhookEntryResponse = v.InferOutput<
  typeof AdminAlertWebhookEntryResponseSchema
>;

/** Admin webhook 投递投影列表。 */
export const AdminAlertWebhookListResponseSchema = strictStruct({
  entries: v.pipe(v.array(AdminAlertWebhookEntryResponseSchema), v.readonly()),
});

/** 最近一页 webhook 投递安全投影。 */
export type AdminAlertWebhookListResponse = v.InferOutput<
  typeof AdminAlertWebhookListResponseSchema
>;

/** 确认告警请求；`command_id`、`reason` 缺失或为 `null` 时按空串处理。 */
export interface AdminAlertAcknowledgeRequest {
  command_id: string;
  /** 列表中所见的事件代数。 */
  incident_generation: number;
  reason: string;
}

/** 静默告警请求；`command_id`、`reason`、`silence_seconds` 缺失或为 `null` 时按零值处理。 */
export interface AdminAlertSilenceRequest {
  command_id: string;
  /** 列表中所见的事件代数。 */
  incident_generation: number;
  reason: string;
  silence_seconds: number;
}

/** 确认/静默的稳定结果；`replayed` 表示同 ID 同参重放。 */
export const AdminAlertCommandResultResponseSchema = strictStruct({
  id: v.string(),
  action: v.string(),
  replayed: v.boolean(),
  silenced_until: optionalNullable(v.string()),
});

/** 确认/静默的稳定结果；`replayed` 表示同 ID 同参重放。 */
export type AdminAlertCommandResultResponse = v.InferOutput<
  typeof AdminAlertCommandResultResponseSchema
>;

/** 告警确认/静默命令的响应信封。 */
export const AdminAlertCommandResponseSchema = strictStruct({
  alert: AdminAlertCommandResultResponseSchema,
});

/** 解析 `AdminAlertCommandResponseSchema` 的告警确认/静默命令回执必须传入的字段策略：
 * 任意一层出现重复键或非整数词法即失败。回执本身没有数字字段，仍按命令结果统一收紧。 */
export const ADMIN_ALERT_COMMAND_RESPONSE_FIELD_POLICY = {
  uniqueKeys: "all",
  numbers: "integers",
} as const;

/** 告警确认/静默命令响应。 */
export type AdminAlertCommandResponse = v.InferOutput<typeof AdminAlertCommandResponseSchema>;

/** 导航待办计数：人工对账、待确认恢复、活动告警，以及按需输出的未结案结算退件。 */
export const AdminPendingCountsResponseSchema = strictStruct({
  delivery_manual_reconcile: integer,
  recovery_confirmation: integer,
  active_alerts: integer,
  /** 未结案的结算退件案件数；只在请求带 `include=settlement_returns` 时输出，缺省响应保持
   * 已发布的三字段闭集，按闭集解码的旧页面照常读取。DTO 接受缺省；旧 API 在请求带
   * `include` 时要求它存在且有效。 */
  settlement_returns: optionalNullable(integer),
});

/** 导航待办计数。 */
export type AdminPendingCountsResponse = v.InferOutput<typeof AdminPendingCountsResponseSchema>;
