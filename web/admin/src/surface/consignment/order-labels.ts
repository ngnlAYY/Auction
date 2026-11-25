// 运营订单与证据的契约取值到译文键的映射；闭集新增取值时由 satisfies 逼出编译错误。

import type { OrderEvidenceItemV2 } from "@auction/contracts/dto/admin/consignment/order-v2";
import type {
  DeliveryStageV2,
  LegStageV2,
  OrderKindV2,
  OrderStateV2,
  PaymentStageV2,
} from "@auction/contracts/dto/public/consignment/order-v2";

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

const KIND_KEYS = {
  sale: "consignment-order-kind-sale",
  withdrawal: "consignment-order-kind-withdrawal",
} satisfies Record<OrderKindV2, string>;

type Evidence<K extends OrderEvidenceItemV2["kind"]> = Extract<OrderEvidenceItemV2, { kind: K }>;

const EVIDENCE_KIND_KEYS = {
  round: "consignment-evidence-kind-round",
  attempt: "consignment-evidence-kind-attempt",
  return_take: "consignment-evidence-kind-return-take",
  observation: "consignment-evidence-kind-observation",
  cash_use: "consignment-evidence-kind-cash-use",
  blocker: "consignment-evidence-kind-blocker",
  closure: "consignment-evidence-kind-closure",
  resolution: "consignment-evidence-kind-resolution",
  recheck: "consignment-evidence-kind-recheck",
} satisfies Record<OrderEvidenceItemV2["kind"], string>;

const LEG_KEYS = {
  original: "consignment-evidence-leg-original",
  carrier: "consignment-evidence-leg-carrier",
  payout: "consignment-evidence-leg-payout",
  refund: "consignment-evidence-leg-refund",
} satisfies Record<Evidence<"round">["leg"], string>;

const TAKE_KEYS = {
  pending: "consignment-evidence-take-pending",
  write_started: "consignment-evidence-take-write-started",
  taken: "consignment-evidence-take-taken",
  consumed: "consignment-evidence-take-consumed",
  reconcile: "consignment-evidence-take-reconcile",
} satisfies Record<Evidence<"return_take">["state"], string>;

const CATEGORY_KEYS = {
  sent: "consignment-evidence-category-sent",
  take: "consignment-evidence-category-take",
  rts: "consignment-evidence-category-rts",
  payment: "consignment-evidence-category-payment",
  claim: "consignment-evidence-category-claim",
} satisfies Record<Evidence<"observation">["category"], string>;

const CONCLUSION_KEYS = {
  pending: "consignment-evidence-conclusion-pending",
  accepted: "consignment-evidence-conclusion-accepted",
  rejected: "consignment-evidence-conclusion-rejected",
  conflict: "consignment-evidence-conclusion-conflict",
} satisfies Record<Evidence<"observation">["conclusion"], string>;

const PURPOSE_KEYS = {
  seller: "consignment-evidence-purpose-seller",
  fee: "consignment-evidence-purpose-fee",
  carrier: "consignment-evidence-purpose-carrier",
  refund: "consignment-evidence-purpose-refund",
  retained: "consignment-evidence-purpose-retained",
} satisfies Record<Evidence<"cash_use">["purpose"], string>;

const BLOCKER_KEYS = {
  opening_uid_block: "consignment-evidence-blocker-opening-uid",
  opposite_payment_open: "consignment-evidence-blocker-opposite-payment",
  funding_boundary: "consignment-evidence-blocker-funding-boundary",
  unknown_external_write: "consignment-evidence-blocker-unknown-write",
} satisfies Record<Evidence<"blocker">["blocker"], string>;

const CLOSURE_KEYS = {
  cancelled_unpaid: "consignment-evidence-closure-cancelled-unpaid",
  refund_closed: "consignment-evidence-closure-refund-closed",
} satisfies Record<Evidence<"closure">["closure_kind"], string>;

/** resolve 动作的译文键；证据里的历史处置与表单选项共用。 */
export const RESOLVE_ACTION_KEYS = {
  cancel_unpaid: "consignment-resolve-action-cancel-unpaid",
  refund_returned: "consignment-resolve-action-refund-returned",
  refund_lost: "consignment-resolve-action-refund-lost",
  refund_season: "consignment-resolve-action-refund-season",
  retain_entitlement: "consignment-resolve-action-retain-entitlement",
  redispatch_original: "consignment-resolve-action-redispatch-original",
} satisfies Record<Evidence<"resolution">["action"], string>;

/** 订单总体状态的译文键。 */
export function orderStateKey(state: OrderStateV2): string {
  return STATE_KEYS[state];
}

/** 各阶段的译文键。 */
export function stageKey(stage: DeliveryStageV2 | PaymentStageV2 | LegStageV2): string {
  return STAGE_KEYS[stage];
}

/** 订单种类的译文键。 */
export function orderKindKey(kind: OrderKindV2): string {
  return KIND_KEYS[kind];
}

/** 证据条目种类的译文键。 */
export function evidenceKindKey(kind: OrderEvidenceItemV2["kind"]): string {
  return EVIDENCE_KIND_KEYS[kind];
}

/** 证据条目里枚举字段的译文键。 */
export const evidenceKeys = {
  leg: (leg: Evidence<"round">["leg"]) => LEG_KEYS[leg],
  take: (state: Evidence<"return_take">["state"]) => TAKE_KEYS[state],
  category: (category: Evidence<"observation">["category"]) => CATEGORY_KEYS[category],
  conclusion: (conclusion: Evidence<"observation">["conclusion"]) => CONCLUSION_KEYS[conclusion],
  purpose: (purpose: Evidence<"cash_use">["purpose"]) => PURPOSE_KEYS[purpose],
  blocker: (blocker: Evidence<"blocker">["blocker"]) => BLOCKER_KEYS[blocker],
  closure: (closure: Evidence<"closure">["closure_kind"]) => CLOSURE_KEYS[closure],
  action: (action: Evidence<"resolution">["action"]) => RESOLVE_ACTION_KEYS[action],
};
