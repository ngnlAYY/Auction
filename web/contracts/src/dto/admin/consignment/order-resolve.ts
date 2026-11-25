// Admin 订单 recheck 与 resolve 的请求与回执线协议。
// 请求只有稳定 UUIDv4 命令 ID、预期合同版本，resolve 另加动作与证据 ID；没有金额、收件人或装备字段，
// 退款金额只由 6B 单写者按冻结成交额决定（不变量 4、6）。回执只含 ID、版本与动作（不变量 7）。

import * as v from "valibot";

import { strictStruct } from "../../../wire";
import { CommandUuidV4Schema } from "../../public/consignment/command";
import { OrderVersionV2Schema } from "../../public/consignment/order-v2";
import { ConsignmentUuidSchema } from "../../public/consignment/scalar";

/** resolve 的动作闭集（D9 §6.2）；退款类三个动作共用同一个 6B 写端。 */
export const OrderResolveActionSchema = v.picklist([
  "cancel_unpaid",
  "refund_returned",
  "refund_lost",
  "refund_season",
  "retain_entitlement",
  "redispatch_original",
]);
/** resolve 动作。 */
export type OrderResolveAction = v.InferOutput<typeof OrderResolveActionSchema>;

/** Admin 排入订单复核。 */
export const RecheckOrderRequestSchema = strictStruct({
  command_id: CommandUuidV4Schema,
  expected_version: OrderVersionV2Schema,
});
/** 复核请求。 */
export type RecheckOrderRequest = v.InferOutput<typeof RecheckOrderRequestSchema>;

/** Admin 订单处置；多余字段整体拒绝。 */
export const ResolveOrderRequestSchema = strictStruct({
  command_id: CommandUuidV4Schema,
  expected_version: OrderVersionV2Schema,
  action: OrderResolveActionSchema,
  evidence_id: ConsignmentUuidSchema,
});
/** 处置请求。 */
export type ResolveOrderRequest = v.InferOutput<typeof ResolveOrderRequestSchema>;

/** recheck 的 202 回执：只有合同、版本与复核工作 ID。 */
export const RecheckOrderReceiptSchema = strictStruct({
  schema_version: v.literal(2),
  operation: v.literal("recheck"),
  disposition: v.literal("recheck_queued"),
  command_id: ConsignmentUuidSchema,
  contract_id: ConsignmentUuidSchema,
  contract_version: OrderVersionV2Schema,
  work_id: ConsignmentUuidSchema,
});
/** 复核回执。 */
export type RecheckOrderReceipt = v.InferOutput<typeof RecheckOrderReceiptSchema>;

/** 处置已记录（200）：动作不是补寄。 */
export const ResolutionRecordedReceiptSchema = v.pipe(
  strictStruct({
    schema_version: v.literal(2),
    operation: v.literal("resolve"),
    disposition: v.literal("resolution_recorded"),
    command_id: ConsignmentUuidSchema,
    contract_id: ConsignmentUuidSchema,
    contract_version: OrderVersionV2Schema,
    action: OrderResolveActionSchema,
    resolution_id: ConsignmentUuidSchema,
  }),
  v.check(({ action }) => action !== "redispatch_original"),
);
/** 处置记录回执。 */
export type ResolutionRecordedReceipt = v.InferOutput<typeof ResolutionRecordedReceiptSchema>;

/** 补寄已排队（202）：只含将要开的新轮 ID，不带动作。 */
export const RedispatchQueuedReceiptSchema = strictStruct({
  schema_version: v.literal(2),
  operation: v.literal("resolve"),
  disposition: v.literal("redispatch_queued"),
  command_id: ConsignmentUuidSchema,
  contract_id: ConsignmentUuidSchema,
  contract_version: OrderVersionV2Schema,
  dispatch_id: ConsignmentUuidSchema,
});
/** 补寄排队回执。 */
export type RedispatchQueuedReceipt = v.InferOutput<typeof RedispatchQueuedReceiptSchema>;
