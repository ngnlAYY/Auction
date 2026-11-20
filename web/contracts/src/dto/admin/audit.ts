// audit 相关 DTO。

import * as v from "valibot";
import { integer, jsonValue, optionalNullable, strictStruct, struct } from "../../wire";

/** Admin 审计记录安全投影。`metadata` 的 DTO 类型是任意 JSON；旧审计 API 另行要求它是对象，
 * 并要求 `request_id` 键显式出现（可为 `null`），这些键形状与值域由路由 API 在 DTO 补
 * `null` 前校验。该结构允许服务端新增字段。 */
export const AdminAuditRecordResponseSchema = struct({
  id: v.string(),
  occurred_at: v.string(),
  actor_type: v.string(),
  actor_id: v.string(),
  action: v.string(),
  entity_type: v.string(),
  entity_id: v.string(),
  result: v.string(),
  /** 旧 API 要求键必须显式出现且可为 `null`；DTO 沿用 Rust 的 `Option` 规则，缺键也能解码。 */
  request_id: optionalNullable(v.string()),
  source: v.string(),
  /** 安全 JSON 值；对象形状由路由 API 的严格镜像另行校验。 */
  metadata: jsonValue,
});

/** Admin 审计记录安全投影；`metadata` 的对象形状与 `request_id` 的显式键属于旧 API 的
 * 校验义务，本类型本身允许新增字段。 */
export type AdminAuditRecordResponse = v.InferOutput<typeof AdminAuditRecordResponseSchema>;

/** Admin 审计 keyset 下一页边界。 */
export const AdminAuditCursorResponseSchema = struct({
  before_time: v.string(),
  before_id: v.string(),
});

/** Admin 审计 keyset 下一页边界。 */
export type AdminAuditCursorResponse = v.InferOutput<typeof AdminAuditCursorResponseSchema>;

/** Admin 审计查询响应；数字页与 keyset 页保留旧契约的互斥字段形状。
 * `page`/`total` 只在数字页出现；`next_cursor` 和 `next_cursor_token` 在两种页形状中
 * 都必须出现，即使当前页没有下一页。这样可以保留旧版客户端依赖的必需键，同时不影响
 * 应用层继续使用 opaque cursor。反序列化沿用 Rust DTO 的 `Option` 规则，缺键可被 DTO
 * 接受；必需键的显式出现由路由 API 的严格镜像另行校验，DTO 与服务端结构本身允许新增字段。 */
export const AdminAuditResponseSchema = struct({
  records: v.pipe(v.array(AdminAuditRecordResponseSchema), v.readonly()),
  page: optionalNullable(integer),
  page_size: integer,
  total: optionalNullable(integer),
  has_more: v.boolean(),
  /** 旧 API 要求键必须显式出现且可为 `null`；DTO 沿用 Rust 的 `Option` 规则，缺键也能解码。 */
  next_cursor: optionalNullable(AdminAuditCursorResponseSchema),
  next_cursor_token: v.string(),
});

/** Admin 审计查询响应；数字页与 keyset 页保留旧契约的互斥字段形状，允许服务端新增字段。 */
export type AdminAuditResponse = v.InferOutput<typeof AdminAuditResponseSchema>;

/** Admin dashboard 的健康聚合。 */
export const AdminDashboardHealthResponseSchema = strictStruct({
  status: v.string(),
  readiness: v.boolean(),
  /** 累计审计失败；发布进程为 classified 模式时只计阻断就绪的失败。 */
  audit_failure_count: integer,
  missing_services: v.pipe(v.array(v.string()), v.readonly()),
});

/** Admin dashboard 的健康聚合。 */
export type AdminDashboardHealthResponse = v.InferOutput<typeof AdminDashboardHealthResponseSchema>;

/** Admin dashboard 的 Worker control 安全投影。 */
export const AdminDashboardWorkerResponseSchema = strictStruct({
  condition: v.string(),
  state: v.string(),
  ready: v.boolean(),
  version: v.string(),
  mail_pollers: integer,
});

/** Admin dashboard 的 Worker control 安全投影。 */
export type AdminDashboardWorkerResponse = v.InferOutput<typeof AdminDashboardWorkerResponseSchema>;

/** Admin dashboard section 的稳定外壳。
 * `data` 只允许由 owner 适配器构造安全 JSON；当 owner 未装配或读取失败时只返回
 * `status` 与有界 `error_code`，不返回数据库行或错误正文。 */
export const AdminDashboardSectionResponseSchema = strictStruct({
  status: v.string(),
  observed_at: optionalNullable(v.string()),
  error_code: optionalNullable(v.string()),
  data: optionalNullable(jsonValue),
});

/** Admin dashboard section 的稳定外壳；失败时只返回 `status` 与有界 `error_code`。 */
export type AdminDashboardSectionResponse = v.InferOutput<
  typeof AdminDashboardSectionResponseSchema
>;

/** Admin dashboard 全部 section 的聚合响应。 */
export const AdminDashboardResponseSchema = strictStruct({
  health: AdminDashboardHealthResponseSchema,
  worker: AdminDashboardWorkerResponseSchema,
  process: AdminDashboardSectionResponseSchema,
  mail: AdminDashboardSectionResponseSchema,
  realtime: AdminDashboardSectionResponseSchema,
  credentials: AdminDashboardSectionResponseSchema,
  /** 外部账号运行时快照；Worker control 不可用时为 unknown section。 */
  external_accounts: AdminDashboardSectionResponseSchema,
  delivery: AdminDashboardSectionResponseSchema,
  fulfillment: AdminDashboardSectionResponseSchema,
  recovery: AdminDashboardSectionResponseSchema,
  alerting: AdminDashboardSectionResponseSchema,
  /** 资产对账与偿付能力报告投影；报告缺失时为 unknown section。 */
  reconciliation: AdminDashboardSectionResponseSchema,
});

/** Admin dashboard 聚合响应。 */
export type AdminDashboardResponse = v.InferOutput<typeof AdminDashboardResponseSchema>;
