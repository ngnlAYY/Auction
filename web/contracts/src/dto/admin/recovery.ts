// Admin recovery DTO。

import * as v from "valibot";
import { int32, integer, optionalNullable, requiredNullable, strictStruct } from "../../wire";

/** Admin recovery 确认请求；`expected_state_version` 是报告快照中的版本 fence。
 *
 * 指定字段沿用旧版 Go 非指针标量的 `null` 零值语义：缺失由 `default` 补缺省值，显式
 * `null` 读作零值。 */
export interface AdminRecoveryConfirmationRequest {
  command_id: string;
  expected_state_version: number;
  action: string;
  reason: string;
}

/** Admin recovery 确认结果的稳定投影。 */
export const AdminRecoveryConfirmationResultResponseSchema = strictStruct({
  /** Rust 内部沿用领域名；旧 v1 线上契约固定使用 `id`。 */
  id: v.string(),
  action: v.string(),
  stage: v.string(),
  resolution: v.string(),
  state_version: integer,
  relisted_auction_id: requiredNullable(v.string()),
  relisted_lot_id: requiredNullable(v.string()),
});

export type AdminRecoveryConfirmationResultResponse = v.InferOutput<
  typeof AdminRecoveryConfirmationResultResponseSchema
>;

export const AdminRecoveryConfirmationResponseSchema = strictStruct({
  recovery: AdminRecoveryConfirmationResultResponseSchema,
});

export type AdminRecoveryConfirmationResponse = v.InferOutput<
  typeof AdminRecoveryConfirmationResponseSchema
>;

/** Admin recovery action 的安全投影。 */
export const AdminRecoveryActionResponseSchema = strictStruct({
  id: v.string(),
  kind: v.string(),
  state: v.string(),
  delivery_id: requiredNullable(v.string()),
  failure_code: requiredNullable(v.string()),
  proof_kind: requiredNullable(v.string()),
  proof_observed_at: requiredNullable(v.string()),
  updated_at: v.string(),
});

export type AdminRecoveryActionResponse = v.InferOutput<typeof AdminRecoveryActionResponseSchema>;

/** Admin recovery case 的安全投影，不包含内部 claim/lease 或外部正文。 */
export const AdminRecoveryResponseSchema = strictStruct({
  id: v.string(),
  auction_id: v.string(),
  lot_id: v.string(),
  lot_number: int32,
  world_id: v.string(),
  equipment_name: v.string(),
  equipment_category: v.string(),
  stage: v.string(),
  resolution: requiredNullable(v.string()),
  state_version: integer,
  payment_deadline_at: v.string(),
  reminder_count: int32,
  reminder_limit: int32,
  custody_proof_kind: requiredNullable(v.string()),
  custody_proof_observed_at: requiredNullable(v.string()),
  blocking_code: requiredNullable(v.string()),
  /** 退件不能把原义务每一次可能外达的发送逐一归属的具体原因（`return_not_current`、
   * `send_in_flight`、`sends_exceed_returns`、`send_without_sent_mid`、`send_records_incomplete`、
   * `send_after_return`）。只在请求带 `include=send_attribution` 且原因存在时输出；缺省响应
   * 保持已发布的字段集合。 */
  send_attribution_blocker: optionalNullable(v.string()),
  manual_reconcile_delivery_id: requiredNullable(v.string()),
  relisted_auction_id: requiredNullable(v.string()),
  relisted_lot_id: requiredNullable(v.string()),
  actions: v.array(AdminRecoveryActionResponseSchema),
  allowed_commands: v.array(v.string()),
  updated_at: v.string(),
});

export type AdminRecoveryResponse = v.InferOutput<typeof AdminRecoveryResponseSchema>;

export const AdminRecoveryReportResponseSchema = strictStruct({
  auction_id: v.string(),
  recoveries: v.array(AdminRecoveryResponseSchema),
});

export type AdminRecoveryReportResponse = v.InferOutput<typeof AdminRecoveryReportResponseSchema>;
