// 订单与购买页的契约取值到译文键的映射；闭集新增取值时由 satisfies 逼出编译错误。

import type {
  DeliveryStageV2,
  LegStageV2,
  OrderKindV2,
  OrderRoleV2,
  OrderStateV2,
  PaymentStageV2,
} from "@auction/contracts/dto/public/consignment/order-v2";
import type { PurchaseRejectionCodeV2 } from "@auction/contracts/dto/public/consignment/purchase-v2";

const STATE_KEYS = {
  awaiting_payment: "consignment-order-state-awaiting-payment",
  awaiting_handoff: "consignment-order-state-awaiting-handoff",
  awaiting_payout: "consignment-order-state-awaiting-payout",
  refunding: "consignment-order-state-refunding",
  withdrawing: "consignment-order-state-withdrawing",
  completed: "consignment-order-state-completed",
  cancelled: "consignment-order-state-cancelled",
  reconcile: "consignment-order-state-reconcile",
} satisfies Record<OrderStateV2, string>;

// 交割、付款与两条资金腿的阶段词共用一张表；同一个词在各阶段里含义一致。
const STAGE_KEYS = {
  not_applicable: "consignment-stage-not-applicable",
  not_started: "consignment-stage-not-started",
  prepared: "consignment-stage-prepared",
  in_flight: "consignment-stage-in-flight",
  sent: "consignment-stage-sent",
  claimed: "consignment-stage-claimed",
  returned: "consignment-stage-returned",
  closed: "consignment-stage-closed",
  unknown: "consignment-stage-unknown",
  unpaid: "consignment-stage-unpaid",
  certified: "consignment-stage-certified",
  blocked: "consignment-stage-blocked",
  ready: "consignment-stage-ready",
  reserved: "consignment-stage-reserved",
} satisfies Record<DeliveryStageV2 | PaymentStageV2 | LegStageV2, string>;

const ROLE_KEYS = {
  buyer: "consignment-order-role-buyer",
  seller: "consignment-order-role-seller",
} satisfies Record<OrderRoleV2, string>;

const KIND_KEYS = {
  sale: "consignment-order-kind-sale",
  withdrawal: "consignment-order-kind-withdrawal",
} satisfies Record<OrderKindV2, string>;

// 只列出有专用译文的确定拒绝；其余走通用寄售错误文案。
const PURCHASE_ERROR_KEYS = {
  price_changed: "consignment-purchase-price-changed",
  equipment_committed: "consignment-purchase-equipment-committed",
  custody_unproven: "consignment-purchase-custody-unproven",
  operations_hold: "consignment-operations-hold",
  season_closed: "consignment-season-closed",
  self_purchase: "consignment-purchase-self",
  bot_party: "consignment-deposit-bot-party",
  participant_ineligible: "consignment-deposit-participant-ineligible",
  funding_unavailable: "consignment-purchase-funding-unavailable",
  consignment_disabled: "consignment-disabled",
  stale_version: "consignment-stale-version",
  listing_not_active: "consignment-listing-not-active",
  command_id_conflict: "consignment-command-conflict",
  outcome_unknown: null,
} satisfies Record<PurchaseRejectionCodeV2, string | null>;

/** 订单总体状态的译文键。 */
export function orderStateKey(state: OrderStateV2): string {
  return STATE_KEYS[state];
}

/** 各阶段的译文键。 */
export function stageKey(stage: DeliveryStageV2 | PaymentStageV2 | LegStageV2): string {
  return STAGE_KEYS[stage];
}

/** 读取者角色的译文键。 */
export function orderRoleKey(role: OrderRoleV2): string {
  return ROLE_KEYS[role];
}

/** 订单种类的译文键。 */
export function orderKindKey(kind: OrderKindV2): string {
  return KIND_KEYS[kind];
}

/** 购买确定拒绝的专用译文键；没有专用文案（含结果未知与未知码）返回 null。 */
export function purchaseErrorKey(code: string): string | null {
  const key = Object.entries(PURCHASE_ERROR_KEYS).find(([name]) => name === code)?.[1];
  return key ?? null;
}

/** 订单详情页的站内地址，只接收经契约解码的合同 ID。 */
export function orderHref(contractId: string): string {
  return `/consignment/orders/${contractId}`;
}

/** 购买确认页的站内地址。 */
export function purchaseHref(listingId: string): string {
  return `/consignment/listings/${listingId}/purchase`;
}
