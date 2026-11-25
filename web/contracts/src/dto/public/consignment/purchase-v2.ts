// 买家购买的请求、回执与确定拒绝码。

import * as v from "valibot";

import { strictStruct } from "../../../wire";
import { CommandUuidV4Schema } from "./command";
import { OrderVersionV2Schema } from "./order-v2";
import { ConsignmentUuidSchema, ListingPriceSchema, ListingVersionSchema } from "./scalar";
import { ListingWorldSchema } from "./states";

/**
 * 购买可返回的确定拒绝码与结果未知码。`outcome_unknown` 只表示提交结果不可判；
 * 504、断线与解码失败没有可用码，客户端同样必须保留原输入与命令 ID 原样重放。
 */
export const PurchaseRejectionCodeV2Schema = v.picklist([
  "command_id_conflict",
  "stale_version",
  "price_changed",
  "listing_not_active",
  "equipment_committed",
  "custody_unproven",
  "operations_hold",
  "season_closed",
  "self_purchase",
  "bot_party",
  "participant_ineligible",
  "funding_unavailable",
  "consignment_disabled",
  "outcome_unknown",
]);
/** 购买拒绝码。 */
export type PurchaseRejectionCodeV2 = v.InferOutput<typeof PurchaseRejectionCodeV2Schema>;

const REJECTION_STATUS: Record<PurchaseRejectionCodeV2, number> = {
  command_id_conflict: 409,
  stale_version: 409,
  price_changed: 409,
  listing_not_active: 409,
  equipment_committed: 409,
  custody_unproven: 409,
  operations_hold: 409,
  season_closed: 409,
  self_purchase: 403,
  bot_party: 403,
  participant_ineligible: 403,
  funding_unavailable: 503,
  consignment_disabled: 404,
  outcome_unknown: 503,
};

/** 与拒绝码配对的 HTTP 状态。 */
export function purchaseRejectionStatus(code: PurchaseRejectionCodeV2): number {
  return REJECTION_STATUS[code];
}

/** 只有确定写前的 `funding_unavailable` 可在客户端白名单中视为可信 503。 */
export function isTrustedPurchaseRejection(code: PurchaseRejectionCodeV2): boolean {
  return code === "funding_unavailable";
}

/** 购买请求。不变量 1、6：稳定 UUIDv4、预期版本与预期价同时绑定。 */
export interface PurchaseRequestV2 {
  command_id: string;
  expected_version: number;
  expected_price: number;
}

/** 购买请求的闭集线协议。 */
export const PurchaseRequestV2Schema = strictStruct({
  command_id: CommandUuidV4Schema,
  expected_version: ListingVersionSchema,
  expected_price: ListingPriceSchema,
});

/** `201 purchased` 回执。只含本次成交的公开事实，不含卖家、预约或装备 key。 */
export const PurchaseReceiptV2Schema = v.pipe(
  strictStruct({
    schema_version: v.literal(2),
    operation: v.literal("purchase"),
    disposition: v.literal("purchased"),
    command_id: ConsignmentUuidSchema,
    listing_id: ConsignmentUuidSchema,
    listing_version: ListingVersionSchema,
    // 购买提交即进入 sale_pending，订单从 awaiting_payment 开始。
    state: v.literal("sale_pending"),
    contract_id: ConsignmentUuidSchema,
    contract_version: OrderVersionV2Schema,
    equipment_id: ConsignmentUuidSchema,
    world: ListingWorldSchema,
    price: ListingPriceSchema,
    order_state: v.literal("awaiting_payment"),
  }),
);
/** 购买回执。 */
export type PurchaseReceiptV2 = v.InferOutput<typeof PurchaseReceiptV2Schema>;

/**
 * 不变量 1、6：回执必须回答该请求——命令 ID 与价格逐字相同，挂单版本向前推进。
 * 重放返回原回执，所以不要求版本恰为预期版本加一。
 * 返回 false 时调用方必须把结果视为 outcome_unknown，只能原样重放同一请求。
 */
export function purchaseReceiptAnswers(
  request: PurchaseRequestV2,
  receipt: PurchaseReceiptV2,
): boolean {
  return (
    receipt.command_id === request.command_id &&
    receipt.price === request.expected_price &&
    receipt.listing_version > request.expected_version
  );
}
