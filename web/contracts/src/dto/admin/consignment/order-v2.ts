// Admin 订单读模型与证据分页；Admin 同时看到买卖双方，金额不按角色省略。

import * as v from "valibot";

import { integer, requiredNullable, strictStruct } from "../../../wire";
import { listingPageSchema } from "../../public/consignment/catalog";
import {
  DeliveryStageV2Schema,
  LegStageV2Schema,
  OrderKindV2Schema,
  OrderStateV2Schema,
  OrderVersionV2Schema,
  PaymentStageV2Schema,
  stagesMatch,
} from "../../public/consignment/order-v2";
import {
  ConsignmentTimestampSchema,
  ConsignmentUuidSchema,
  consignmentTextSchema,
  ListingPriceSchema,
} from "../../public/consignment/scalar";
import { ListingWorldSchema } from "../../public/consignment/states";
import { OrderResolveActionSchema } from "./order-resolve";

/** 现金腿金额上限：标价上限十亿再加载体本金 1。 */
const CASH_AMOUNT_MAX = 1_000_000_001;

/** Admin 订单。买家字段与金额只在成交订单出现；不含预约码、装备 key 与原信正文。 */
export const AdminOrderV2Schema = v.pipe(
  strictStruct({
    contract_id: ConsignmentUuidSchema,
    kind: OrderKindV2Schema,
    listing_id: ConsignmentUuidSchema,
    equipment_id: ConsignmentUuidSchema,
    world: ListingWorldSchema,
    order_state: OrderStateV2Schema,
    delivery_stage: DeliveryStageV2Schema,
    payment_stage: PaymentStageV2Schema,
    payout_stage: LegStageV2Schema,
    refund_stage: LegStageV2Schema,
    version: OrderVersionV2Schema,
    price: requiredNullable(ListingPriceSchema),
    deadline_at: requiredNullable(ConsignmentTimestampSchema),
    fee: requiredNullable(integer),
    seller_amount: requiredNullable(integer),
    buyer_user_id: requiredNullable(ConsignmentUuidSchema),
    buyer_name: requiredNullable(consignmentTextSchema(128)),
    seller_user_id: ConsignmentUuidSchema,
    seller_name: consignmentTextSchema(128),
    updated_at: ConsignmentTimestampSchema,
  }),
  v.check((order) => {
    if (order.kind === "withdrawal") {
      return (
        order.price === null &&
        order.deadline_at === null &&
        order.fee === null &&
        order.seller_amount === null &&
        order.buyer_user_id === null &&
        order.buyer_name === null
      );
    }
    // 不变量 6、10：P = F + S，手续费不吞没全部价款，买卖双方不同账号。
    const { price, fee, seller_amount: sellerAmount, buyer_user_id: buyer } = order;
    return (
      price !== null &&
      order.deadline_at !== null &&
      fee !== null &&
      sellerAmount !== null &&
      buyer !== null &&
      order.buyer_name !== null &&
      fee >= 0 &&
      fee < price &&
      sellerAmount >= 1 &&
      fee + sellerAmount === price &&
      buyer !== order.seller_user_id
    );
  }),
  v.check((order) => stagesMatch(order.kind, order)),
);
/** Admin 订单。 */
export type AdminOrderV2 = v.InferOutput<typeof AdminOrderV2Schema>;

/** Admin 订单分页。 */
export const AdminOrderPageV2Schema = listingPageSchema(AdminOrderV2Schema);

const FENCE = v.pipe(integer, v.minValue(1));
const DIGEST = v.pipe(v.string(), v.regex(/^[0-9a-f]{64}$/u));

/** 订单证据条目。只含 ID、阶段、金额、摘要与时刻，不含预约码、装备 key 与原信正文。 */
export const OrderEvidenceItemV2Schema = v.union([
  strictStruct({
    kind: v.literal("round"),
    round_id: ConsignmentUuidSchema,
    round_number: FENCE,
    leg: v.picklist(["original", "carrier", "payout", "refund"]),
    delivery_stage: DeliveryStageV2Schema,
    created_at: ConsignmentTimestampSchema,
  }),
  v.pipe(
    strictStruct({
      kind: v.literal("attempt"),
      attempt_id: ConsignmentUuidSchema,
      round_id: ConsignmentUuidSchema,
      fence: FENCE,
      write_started: v.boolean(),
      delivery_stage: DeliveryStageV2Schema,
    }),
    // 起写事实先于 POST：未起写只能是 prepared，起写后不得回到未开始。
    v.check(
      ({ write_started, delivery_stage }) =>
        write_started !== (delivery_stage === "not_started" || delivery_stage === "prepared"),
    ),
  ),
  v.pipe(
    strictStruct({
      kind: v.literal("return_take"),
      intake_id: ConsignmentUuidSchema,
      work_id: ConsignmentUuidSchema,
      original_round_id: ConsignmentUuidSchema,
      fence: requiredNullable(FENCE),
      state: v.picklist(["pending", "write_started", "taken", "consumed", "reconcile"]),
    }),
    v.check(({ fence, state }) => (fence === null) === (state === "pending")),
  ),
  strictStruct({
    kind: v.literal("observation"),
    observation_id: ConsignmentUuidSchema,
    category: v.picklist(["sent", "take", "rts", "payment", "claim"]),
    conclusion: v.picklist(["pending", "accepted", "rejected", "conflict"]),
    digest: DIGEST,
    observed_at: ConsignmentTimestampSchema,
  }),
  v.pipe(
    strictStruct({
      kind: v.literal("cash_use"),
      purpose: v.picklist(["seller", "fee", "carrier", "refund", "retained"]),
      state: LegStageV2Schema,
      amount: v.pipe(integer, v.minValue(0), v.maxValue(CASH_AMOUNT_MAX)),
    }),
    v.check(({ state }) => state !== "not_applicable"),
  ),
  strictStruct({
    kind: v.literal("blocker"),
    blocker: v.picklist([
      "opening_uid_block",
      "opposite_payment_open",
      "funding_boundary",
      "unknown_external_write",
    ]),
    since: ConsignmentTimestampSchema,
    resolved: v.boolean(),
  }),
  // 恢复段（第 7 期）：Sale 封闭事实、Admin 处置记录与复核工作。
  strictStruct({
    kind: v.literal("closure"),
    closure_kind: v.picklist(["cancelled_unpaid", "refund_closed"]),
    closed_at: ConsignmentTimestampSchema,
  }),
  strictStruct({
    kind: v.literal("resolution"),
    resolution_id: ConsignmentUuidSchema,
    action: OrderResolveActionSchema,
    evidence_id: ConsignmentUuidSchema,
    resolved_at: ConsignmentTimestampSchema,
  }),
  strictStruct({
    kind: v.literal("recheck"),
    work_id: ConsignmentUuidSchema,
    requested_at: ConsignmentTimestampSchema,
  }),
]);
/** 订单证据条目。 */
export type OrderEvidenceItemV2 = v.InferOutput<typeof OrderEvidenceItemV2Schema>;

/** 订单证据分页。 */
export const OrderEvidencePageV2Schema = listingPageSchema(OrderEvidenceItemV2Schema);
