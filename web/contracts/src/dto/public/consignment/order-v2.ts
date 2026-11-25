// 第 6 期订单读模型：阶段闭集、阶段组合与订单态的一致性，以及按角色省略的金额。

import * as v from "valibot";

import { integer, JSON_MAX_SAFE_INTEGER, requiredNullable, strictStruct } from "../../../wire";
import { listingPageSchema } from "./catalog";
import {
  ConsignmentTimestampSchema,
  ConsignmentUuidSchema,
  LISTING_PRICE_MAX,
  ListingPriceSchema,
} from "./scalar";
import { ListingWorldSchema } from "./states";

/** 订单种类：买家成交或原卖家直接撤回。 */
export const OrderKindV2Schema = v.picklist(["sale", "withdrawal"]);
/** 读取者在订单里的角色。 */
export const OrderRoleV2Schema = v.picklist(["buyer", "seller"]);
/** 订单对读取者的总体状态。 */
export const OrderStateV2Schema = v.picklist([
  "awaiting_payment",
  "awaiting_handoff",
  "awaiting_payout",
  "refunding",
  "withdrawing",
  "completed",
  "cancelled",
  "reconcile",
]);
/** 原件或载体的交割阶段。 */
export const DeliveryStageV2Schema = v.picklist([
  "not_started",
  "prepared",
  "in_flight",
  "sent",
  "claimed",
  "returned",
  "closed",
  "unknown",
]);
/** 买家付款阶段。 */
export const PaymentStageV2Schema = v.picklist([
  "not_applicable",
  "unpaid",
  "certified",
  "closed",
  "unknown",
]);
/** 卖款与退款两条资金腿的阶段。 */
export const LegStageV2Schema = v.picklist([
  "not_applicable",
  "blocked",
  "ready",
  "reserved",
  "sent",
  "claimed",
  "returned",
  "closed",
  "unknown",
]);
/** 订单上读取者可执行的动作。第 6 期恒为空，第 7 期放开时与校验同改。 */
export const OrderActionV2Schema = v.picklist(["locate", "challenge", "verify", "claim"]);

export type OrderKindV2 = v.InferOutput<typeof OrderKindV2Schema>;
export type OrderRoleV2 = v.InferOutput<typeof OrderRoleV2Schema>;
export type OrderStateV2 = v.InferOutput<typeof OrderStateV2Schema>;
export type DeliveryStageV2 = v.InferOutput<typeof DeliveryStageV2Schema>;
export type PaymentStageV2 = v.InferOutput<typeof PaymentStageV2Schema>;
export type LegStageV2 = v.InferOutput<typeof LegStageV2Schema>;
export type OrderActionV2 = v.InferOutput<typeof OrderActionV2Schema>;

/** 订单版本从 1 开始，必须能由浏览器精确表示。 */
export const OrderVersionV2Schema = v.pipe(
  integer,
  v.minValue(1),
  v.maxValue(JSON_MAX_SAFE_INTEGER),
);

/** 订单阶段组合，供两个订单 DTO 共用。 */
export interface OrderStagesV2 {
  readonly order_state: OrderStateV2;
  readonly delivery_stage: DeliveryStageV2;
  readonly payment_stage: PaymentStageV2;
  readonly payout_stage: LegStageV2;
  readonly refund_stage: LegStageV2;
}

const BEFORE_CLAIM: ReadonlySet<DeliveryStageV2> = new Set([
  "not_started",
  "prepared",
  "in_flight",
  "sent",
]);
const ACTIVE_LEG: ReadonlySet<LegStageV2> = new Set([
  "blocked",
  "ready",
  "reserved",
  "sent",
  "returned",
]);

/**
 * 阶段组合与订单态是否相容。`reconcile` 吸收矛盾组合，但不放宽适用性：
 * 成交订单一定有付款腿，撤回订单的三条资金腿一定不适用。
 * 领域投影与本函数各写一次，由共享夹具互核。
 */
export function stagesMatch(kind: OrderKindV2, stages: OrderStagesV2): boolean {
  const { order_state: state, delivery_stage: delivery, payment_stage: payment } = stages;
  const { payout_stage: payout, refund_stage: refund } = stages;
  const unknown =
    delivery === "unknown" || payment === "unknown" || payout === "unknown" || refund === "unknown";
  if (unknown && state !== "reconcile") return false;
  const beforeClaim = BEFORE_CLAIM.has(delivery);
  // 退款权转移的前提是原件已证退回或封闭（或从未起写）；prepared、in_flight、sent 仍可能被确领。
  const settled = delivery === "not_started" || delivery === "returned" || delivery === "closed";
  const prePayout = payout === "not_applicable" || payout === "blocked";
  const closedLeg = payout === "not_applicable" || payout === "closed";
  if (kind === "withdrawal") {
    if (
      payment !== "not_applicable" ||
      payout !== "not_applicable" ||
      refund !== "not_applicable"
    ) {
      return false;
    }
    if (state === "reconcile") return true;
    if (state === "withdrawing") return beforeClaim || delivery === "returned";
    if (state === "completed") return delivery === "claimed";
    if (state === "cancelled") {
      return delivery === "not_started" || delivery === "returned" || delivery === "closed";
    }
    return false;
  }
  switch (state) {
    case "reconcile":
      return payment !== "not_applicable";
    case "awaiting_payment":
      return payment === "unpaid" && beforeClaim && prePayout && refund === "not_applicable";
    case "awaiting_handoff":
      return payment === "certified" && beforeClaim && prePayout && refund === "not_applicable";
    case "awaiting_payout":
      return (
        payment === "certified" &&
        delivery === "claimed" &&
        ACTIVE_LEG.has(payout) &&
        refund === "not_applicable"
      );
    case "refunding":
      return payment === "certified" && settled && closedLeg && ACTIVE_LEG.has(refund);
    case "completed":
      return (
        delivery === "claimed" &&
        payment === "certified" &&
        payout === "claimed" &&
        refund === "not_applicable"
      );
    case "cancelled":
      return (
        settled &&
        closedLeg &&
        ((payment === "closed" && (refund === "not_applicable" || refund === "closed")) ||
          (payment === "certified" && refund === "claimed"))
      );
    case "withdrawing":
      return false;
  }
}

/** 键可以缺省，出现时必须是 min 到价格上限的整数；null 不是省略。 */
function exactOptionalAmount(min: number) {
  return v.exactOptional(v.pipe(integer, v.minValue(min), v.maxValue(LISTING_PRICE_MAX)));
}

const ORDER_V2_FIELDS = strictStruct({
  contract_id: ConsignmentUuidSchema,
  kind: OrderKindV2Schema,
  role: OrderRoleV2Schema,
  listing_id: ConsignmentUuidSchema,
  equipment_id: ConsignmentUuidSchema,
  world: ListingWorldSchema,
  order_state: OrderStateV2Schema,
  delivery_stage: DeliveryStageV2Schema,
  payment_stage: PaymentStageV2Schema,
  payout_stage: LegStageV2Schema,
  refund_stage: LegStageV2Schema,
  allowed_actions: v.array(OrderActionV2Schema),
  version: OrderVersionV2Schema,
  price: requiredNullable(ListingPriceSchema),
  deadline_at: requiredNullable(ConsignmentTimestampSchema),
  // 只有卖方的成交订单带这两项；其余视图省略，出现 null 一律拒绝。
  fee: exactOptionalAmount(0),
  seller_amount: exactOptionalAmount(1),
});

/** 不变量 6、10：卖方视图 P = F + S；买方视图与撤回订单不带私有金额。 */
function amountsMatch(order: v.InferOutput<typeof ORDER_V2_FIELDS>): boolean {
  if (order.kind === "withdrawal") {
    return (
      order.role === "seller" &&
      order.price === null &&
      order.deadline_at === null &&
      order.fee === undefined &&
      order.seller_amount === undefined
    );
  }
  if (order.price === null || order.deadline_at === null) return false;
  if (order.role === "buyer") return order.fee === undefined && order.seller_amount === undefined;
  const { fee, seller_amount: sellerAmount } = order;
  return (
    fee !== undefined &&
    sellerAmount !== undefined &&
    fee < order.price &&
    fee + sellerAmount === order.price
  );
}

/** 本人订单。不得含预约码、装备 key、对方账号或原信正文。 */
export const OrderV2Schema = v.pipe(
  ORDER_V2_FIELDS,
  // 第 6 期买卖双方没有订单命令；放开动作须与第 7 期的授权矩阵一起改。
  v.check((order) => order.allowed_actions.length === 0),
  v.check(amountsMatch),
  v.check((order) =>
    stagesMatch(order.kind, {
      order_state: order.order_state,
      delivery_stage: order.delivery_stage,
      payment_stage: order.payment_stage,
      payout_stage: order.payout_stage,
      refund_stage: order.refund_stage,
    }),
  ),
);
/** 本人订单。 */
export type OrderV2 = v.InferOutput<typeof OrderV2Schema>;

/** 订单分页：筛选 `role、world、state、page、page_size`，页壳与挂单分页一致。 */
export const OrderPageV2Schema = listingPageSchema(OrderV2Schema);
/** 订单页。 */
export type OrderPageV2 = v.InferOutput<typeof OrderPageV2Schema>;
