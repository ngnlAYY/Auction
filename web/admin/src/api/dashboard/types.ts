// Admin dashboard 分区 DTO 的类型、字段闭集 schema 与浏览器类型化投影。
// 来源是 Rust 的 `deny_unknown_fields` 与 `deserialize_required_option`；跨字段语义在
// `valid.ts` 中校验。本模块不发起请求。

import {
  type AdminDashboardHealthResponse,
  AdminDashboardHealthResponseSchema,
  type AdminDashboardWorkerResponse,
} from "@auction/contracts/dto/admin/audit";
import { AdminMailPollingResponseSchema } from "@auction/contracts/dto/admin/mail";
import { int32, integer, requiredNullable, strictStruct } from "@auction/contracts/wire";
import * as v from "valibot";

/** 进程 tracing 安全摘要。 */
export const AdminTracingDataSchema = strictStruct({
  status: v.string(),
  dropped_span_count: integer,
  export_failure_count: integer,
  last_success_at: requiredNullable(v.string()),
  last_failure_at: requiredNullable(v.string()),
  last_error_code: v.string(),
});

/** 进程 tracing 安全摘要。 */
export type AdminTracingData = v.InferOutput<typeof AdminTracingDataSchema>;

/** 单个进程的健康安全投影。 */
export const AdminProcessDataSchema = strictStruct({
  service_name: v.string(),
  process_role: v.string(),
  instance_id: v.string(),
  version: v.string(),
  started_at: v.string(),
  status: v.string(),
  readiness: v.boolean(),
  audit_failure_count: integer,
  tracing: AdminTracingDataSchema,
  observed_at: v.string(),
  expires_at: v.string(),
});

/** 单个进程的健康安全投影。 */
export type AdminProcessData = v.InferOutput<typeof AdminProcessDataSchema>;

/** 进程 dashboard section 的数据。 */
export const AdminProcessSectionDataSchema = strictStruct({
  processes: v.pipe(v.array(AdminProcessDataSchema), v.readonly()),
  summary: AdminDashboardHealthResponseSchema,
  observed_at: v.string(),
});

/** 进程 dashboard section 的数据。 */
export type AdminProcessSectionData = v.InferOutput<typeof AdminProcessSectionDataSchema>;

/** Realtime dashboard 的 lag 条目。 */
export const AdminRealtimeLagSchema = strictStruct({
  stream_key: v.string(),
  consumer_key: v.string(),
  offset: integer,
  latest_sequence: integer,
  lag: integer,
  last_advanced_at: requiredNullable(v.string()),
});

/** Realtime dashboard 的 lag 条目。 */
export type AdminRealtimeLag = v.InferOutput<typeof AdminRealtimeLagSchema>;

/** Realtime 未解决失败的安全摘要。 */
export const AdminRealtimeFailureSchema = strictStruct({
  count: integer,
  oldest_at: requiredNullable(v.string()),
  latest_code: v.string(),
});

/** Realtime 未解决失败的安全摘要。 */
export type AdminRealtimeFailure = v.InferOutput<typeof AdminRealtimeFailureSchema>;

/** Realtime dashboard section 的数据。 */
export const AdminRealtimeDataSchema = strictStruct({
  total_lag: integer,
  lags: v.pipe(v.array(AdminRealtimeLagSchema), v.readonly()),
  failure: AdminRealtimeFailureSchema,
  observed_at: v.string(),
});

/** Realtime dashboard section 的数据。 */
export type AdminRealtimeData = v.InferOutput<typeof AdminRealtimeDataSchema>;

/** 告警投递面板的数据。 */
export const AdminAlertingDataSchema = strictStruct({
  enabled: v.boolean(),
  active_signal_count: integer,
  due_signal_count: integer,
  claimed_batch_count: integer,
  succeeded_batch_count: integer,
  failed_batch_count: integer,
  manual_reconcile_count: integer,
  webhook_pending_count: integer,
  webhook_sending_count: integer,
  webhook_failed_count: integer,
  webhook_manual_reconcile_count: integer,
  last_observed_at: requiredNullable(v.string()),
  last_success_at: requiredNullable(v.string()),
  last_failure_at: requiredNullable(v.string()),
  last_error_code: v.string(),
});

/** 告警投递面板的数据。 */
export type AdminAlertingData = v.InferOutput<typeof AdminAlertingDataSchema>;

/** 按世界汇总的 Cookie 状态。 */
export const AdminCredentialAvailabilitySchema = strictStruct({
  world: v.string(),
  available: integer,
  invalid: integer,
  updated_at: requiredNullable(v.string()),
  restart_required: requiredNullable(v.boolean()),
});

/** 按世界汇总的 Cookie 状态。 */
export type AdminCredentialAvailability = v.InferOutput<typeof AdminCredentialAvailabilitySchema>;

/** Cookie 状态分区数据。 */
export const AdminCredentialDataSchema = strictStruct({
  availability: v.pipe(v.array(AdminCredentialAvailabilitySchema), v.readonly()),
  observed_at: v.string(),
  last_admin_change_at: requiredNullable(v.string()),
});

/** Cookie 状态分区数据。 */
export type AdminCredentialData = v.InferOutput<typeof AdminCredentialDataSchema>;

/** 外部账号分区中由既有 owner 投影的子列表状态；`unavailable` 不表示空列表。 */
export const AdminExternalPollerSchema = strictStruct({
  mailbox_key: v.string(),
  paused: v.boolean(),
  paused_since: requiredNullable(v.string()),
  consecutive_failures: integer,
  last_poll_at: v.string(),
  next_poll_at: v.string(),
});

/** 外部账号分区中由既有 owner 投影的子列表状态；`unavailable` 不表示空列表。 */
export type AdminExternalPoller = v.InferOutput<typeof AdminExternalPollerSchema>;

/** 外部邮箱轮询列表。 */
export const AdminExternalPollerListSchema = strictStruct({
  status: v.string(),
  items: v.pipe(v.array(AdminExternalPollerSchema), v.readonly()),
});

/** 外部邮箱轮询列表。 */
export type AdminExternalPollerList = v.InferOutput<typeof AdminExternalPollerListSchema>;

/** 外部账号分区的邮件运行时摘要。 */
export const AdminExternalMailDataSchema = strictStruct({
  busy: v.boolean(),
  busy_age_seconds: requiredNullable(integer),
  gate_wait_count: integer,
  gate_wait_seconds_total: integer,
  request_count: integer,
  last_request_at: requiredNullable(v.string()),
  last_success_at: requiredNullable(v.string()),
  last_failure_at: requiredNullable(v.string()),
  pollers: AdminExternalPollerListSchema,
});

/** 外部账号分区的邮件运行时摘要。 */
export type AdminExternalMailData = v.InferOutput<typeof AdminExternalMailDataSchema>;

/** 外部账号分区的论坛同步摘要。 */
export const AdminExternalForumDataSchema = strictStruct({
  last_request_at: requiredNullable(v.string()),
  last_success_at: requiredNullable(v.string()),
  last_failure_at: requiredNullable(v.string()),
  write_unknown_count: integer,
});

/** 外部账号分区的论坛同步摘要。 */
export type AdminExternalForumData = v.InferOutput<typeof AdminExternalForumDataSchema>;

/** 外部账号分区的维护状态。 */
export const AdminExternalMaintenanceDataSchema = strictStruct({
  state: v.string(),
  last_started_at: requiredNullable(v.string()),
  last_success_at: requiredNullable(v.string()),
  last_failure_at: requiredNullable(v.string()),
  last_canceled_at: requiredNullable(v.string()),
  running_age_seconds: requiredNullable(integer),
});

/** 外部账号分区的维护状态。 */
export type AdminExternalMaintenanceData = v.InferOutput<typeof AdminExternalMaintenanceDataSchema>;

/** 外部账号分区的一条凭据计数。 */
export const AdminExternalCredentialSchema = strictStruct({
  world: v.string(),
  kind: v.string(),
  available: integer,
  invalid: integer,
});

/** 外部账号分区的一条凭据计数。 */
export type AdminExternalCredential = v.InferOutput<typeof AdminExternalCredentialSchema>;

/** 外部账号分区的凭据列表。 */
export const AdminExternalCredentialListSchema = strictStruct({
  status: v.string(),
  items: v.pipe(v.array(AdminExternalCredentialSchema), v.readonly()),
});

/** 外部账号分区的凭据列表。 */
export type AdminExternalCredentialList = v.InferOutput<typeof AdminExternalCredentialListSchema>;

/** 外部账号分区数据。 */
export const AdminExternalAccountsDataSchema = strictStruct({
  observed_at: v.string(),
  mail: AdminExternalMailDataSchema,
  forum: AdminExternalForumDataSchema,
  maintenance: AdminExternalMaintenanceDataSchema,
  credentials: AdminExternalCredentialListSchema,
});

/** 外部账号分区数据。 */
export type AdminExternalAccountsData = v.InferOutput<typeof AdminExternalAccountsDataSchema>;

/** 投递水位数据。 */
export const AdminDeliveryWaterSchema = strictStruct({
  pending: integer,
  failed: integer,
  manual_reconcile: integer,
  oldest_pending_age_seconds: integer,
  oldest_manual_reconcile_age_seconds: integer,
  last_update_at: requiredNullable(v.string()),
});

/** 投递水位数据。 */
export type AdminDeliveryWater = v.InferOutput<typeof AdminDeliveryWaterSchema>;

/** 投递分区数据。 */
export const AdminDeliveryDataSchema = strictStruct({
  observed_at: v.string(),
  water: AdminDeliveryWaterSchema,
});

/** 投递分区数据。 */
export type AdminDeliveryData = v.InferOutput<typeof AdminDeliveryDataSchema>;

/** 单个 world 的资产对账差异摘要。 */
export const AdminReconciliationWorldSchema = strictStruct({
  world: v.string(),
  custody_missing_count: integer,
  /** 已寄出、等待收件人领取的在途装备件数；只展示，不是差异。 */
  custody_in_transit_count: integer,
  solvency_shortfall_credits: integer,
  /** 已寄出且资金已支出的在途货款与退款笔数及金额；不计入缺口。 */
  solvency_in_transit_count: integer,
  solvency_in_transit_credits: integer,
});

/** 单个 world 的资产对账差异摘要。 */
export type AdminReconciliationWorld = v.InferOutput<typeof AdminReconciliationWorldSchema>;

/** 资产对账分区数据。 */
export const AdminReconciliationDataSchema = strictStruct({
  observed_at: v.string(),
  custody_report_age_seconds: integer,
  solvency_report_age_seconds: integer,
  worlds: v.pipe(v.array(AdminReconciliationWorldSchema), v.readonly()),
});

/** 资产对账分区数据。 */
export type AdminReconciliationData = v.InferOutput<typeof AdminReconciliationDataSchema>;

/** 长期未领取清单的一项；只含业务标识与展示字段，不含投递载荷、凭据或外部错误原文。 */
export const AdminCollectionOverdueItemSchema = strictStruct({
  obligation_id: v.string(),
  /** `seller_payout`、`buyer_refund`、`unsold_return`、`withdrawn_return` 或
   * `winner_equipment_handoff`。 */
  obligation_kind: v.string(),
  world: v.string(),
  auction_sequence: integer,
  /** 分类内编号；与 `equipment_category` 组成拍品代码（如 `Shd02`），不是全场编号。 */
  lot_number: int32,
  equipment_category: v.string(),
  equipment_name: v.string(),
  recipient: v.string(),
  sent_at: v.string(),
  age_seconds: integer,
  /** 最近一次 Sent 读取失败；`false` 只表示没有记录读取失败，不代表已读到或已确认未领取。 */
  last_read_failed: v.boolean(),
});

/** 长期未领取清单的一项；只含业务标识与展示字段，不含投递载荷、凭据或外部错误原文。 */
export type AdminCollectionOverdueItem = v.InferOutput<typeof AdminCollectionOverdueItemSchema>;

/** 履约分区数据。 */
export const AdminFulfillmentDataSchema = strictStruct({
  pending: integer,
  blocked: integer,
  failed: integer,
  manual_reconcile: integer,
  waiting_payment: integer,
  waiting_prerequisite: integer,
  /** 已寄出、只等收件人领取的义务；不计入 `pending`。 */
  awaiting_collection: integer,
  /** 等待领取中最近一次 Sent 读取失败的数量；是 `awaiting_collection` 的子集。 */
  awaiting_collection_unreadable: integer,
  observed_at: v.string(),
  oldest_backlog_age_seconds: integer,
  oldest_prerequisite_wait_age_seconds: integer,
  /** 最早寄出的等待领取义务自寄出起的秒数；页面按天展示。 */
  oldest_awaiting_collection_age_seconds: integer,
  /** 服务端按告警策略阈值计的长期未领取件数（超期件数），是 `awaiting_collection` 的子集；
   * 页面不自建阈值。 */
  collection_overdue_count: integer,
  /** 与超期件数同一快照的长期未领取清单：寄出最早优先，条数有上限，不多于超期件数。 */
  collection_overdue_items: v.pipe(v.array(AdminCollectionOverdueItemSchema), v.readonly()),
});

/** 履约分区数据。 */
export type AdminFulfillmentData = v.InferOutput<typeof AdminFulfillmentDataSchema>;

/** 单个 world 的回收证据摘要。 */
export const AdminRecoveryWorldSchema = strictStruct({
  world: v.string(),
  balance_credits: integer,
  balance_status: v.string(),
  evidence_status: v.string(),
  balance_observed_at: requiredNullable(v.string()),
  evidence_last_success_at: requiredNullable(v.string()),
  updated_at: v.string(),
});

/** 单个 world 的回收证据摘要。 */
export type AdminRecoveryWorld = v.InferOutput<typeof AdminRecoveryWorldSchema>;

/** 回收分区数据。 */
export const AdminRecoveryDataSchema = strictStruct({
  observed_at: v.string(),
  worlds: v.pipe(v.array(AdminRecoveryWorldSchema), v.readonly()),
  payment_overdue_count: integer,
  relist_pending_count: integer,
  failed_action_count: integer,
  manual_reconcile_count: integer,
  oldest_overdue_age_seconds: integer,
});

/** 回收分区数据。 */
export type AdminRecoveryData = v.InferOutput<typeof AdminRecoveryDataSchema>;

/** Dashboard section 的浏览器类型化投影；wire 层仍保留 owner 构造的有界 JSON。 */
export interface AdminDashboardSection<T> {
  status: string;
  observed_at: string | null;
  error_code: string | null;
  data: T | null;
}

/** Dashboard 的健康聚合；与 contracts 的 `AdminDashboardHealthResponse` 相同。 */
export type AdminDashboardHealth = AdminDashboardHealthResponse;

/** Dashboard 的 Worker control 安全投影；与 contracts 的 `AdminDashboardWorkerResponse` 相同。 */
export type AdminDashboardWorker = AdminDashboardWorkerResponse;

export type AdminAlertingSection = AdminDashboardSection<AdminAlertingData>;

export type AdminCredentialSection = AdminDashboardSection<AdminCredentialData>;

/** 邮件轮询分区数据；轮询 DTO 复用邮件页的 contracts 定义。 */
export const AdminMailDataSchema = strictStruct({
  observed_at: v.string(),
  pollers: v.pipe(v.array(AdminMailPollingResponseSchema), v.readonly()),
});

export type AdminMailData = v.InferOutput<typeof AdminMailDataSchema>;

export type AdminMailSection = AdminDashboardSection<AdminMailData>;

export type AdminExternalAccountsSection = AdminDashboardSection<AdminExternalAccountsData>;

export type AdminDeliverySection = AdminDashboardSection<AdminDeliveryData>;

export type AdminReconciliationSection = AdminDashboardSection<AdminReconciliationData>;

export type AdminFulfillmentSection = AdminDashboardSection<AdminFulfillmentData>;

export type AdminRecoverySection = AdminDashboardSection<AdminRecoveryData>;

/** Admin dashboard 全量安全快照；前端只消费这里声明的稳定字段。 */
export interface AdminDashboardSnapshot {
  health: AdminDashboardHealth;
  worker: AdminDashboardWorker;
  process: AdminDashboardSection<AdminProcessSectionData>;
  mail: AdminMailSection;
  realtime: AdminDashboardSection<AdminRealtimeData>;
  credentials: AdminCredentialSection;
  external_accounts: AdminExternalAccountsSection;
  delivery: AdminDeliverySection;
  fulfillment: AdminFulfillmentSection;
  recovery: AdminRecoverySection;
  alerting: AdminAlertingSection;
  reconciliation: AdminReconciliationSection;
}
