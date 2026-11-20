// 结算退件页的闭集取值到译文键的映射与安全细节文本。
//
// 每个闭集取值都有固定的 Fluent 键；解码已拒绝闭集外的值，这里只兜底原样显示。键映射与
// 取译文分开，单元测试据此核对契约闭集逐项有译文、五种语言键集合一致。

import { compareText } from "@auction/web-shared/text";
import { t } from "@auction/web-shared/ui/i18n";
import { validUuid } from "@auction/web-shared/ui/validation";

/** 案件状态的译文键。 */
export function statusKey(value: string): string | null {
  switch (value) {
    case "quarantined":
      return "settlement-return-status-quarantined";
    case "recheck_pending":
      return "settlement-return-status-recheck-pending";
    case "awaiting_decision":
      return "settlement-return-status-awaiting-decision";
    case "take_pending":
      return "settlement-return-status-take-pending";
    case "take_unknown":
      return "settlement-return-status-take-unknown";
    case "resend_pending":
      return "settlement-return-status-resend-pending";
    case "held":
      return "settlement-return-status-held";
    case "resent":
      return "settlement-return-status-resent";
    case "inbox_closed":
      return "settlement-return-status-inbox-closed";
    case "reviewed":
      return "settlement-return-status-reviewed";
    default:
      return null;
  }
}

/** 证据分类的译文键。 */
export function matchKey(value: string): string | null {
  switch (value) {
    case "untrusted":
      return "settlement-return-match-untrusted";
    case "unbound":
      return "settlement-return-match-unbound";
    case "fulfillment_terminal":
      return "settlement-return-match-fulfillment-terminal";
    case "manual_reconcile":
      return "settlement-return-match-manual-reconcile";
    default:
      return null;
  }
}

/** 原因码的译文键。 */
export function reasonKey(value: string): string | null {
  switch (value) {
    case "source_unproven":
      return "settlement-return-reason-source-unproven";
    case "evidence_invalid":
      return "settlement-return-reason-evidence-invalid";
    case "obligation_unknown":
      return "settlement-return-reason-obligation-unknown";
    case "subject_kind_mismatch":
      return "settlement-return-reason-subject-kind-mismatch";
    case "delivery_unsent":
      return "settlement-return-reason-delivery-unsent";
    case "world_mismatch":
      return "settlement-return-reason-world-mismatch";
    case "dispatch_unknown":
      return "settlement-return-reason-dispatch-unknown";
    case "dispatch_ambiguous":
      return "settlement-return-reason-dispatch-ambiguous";
    case "dispatch_already_returned":
      return "settlement-return-reason-dispatch-already-returned";
    case "dispatch_superseded":
      return "settlement-return-reason-dispatch-superseded";
    case "body_unproven":
      return "settlement-return-reason-body-unproven";
    case "returned_assets_match":
      return "settlement-return-reason-returned-assets-match";
    case "returned_assets_mismatch":
      return "settlement-return-reason-returned-assets-mismatch";
    case "received_before_send":
      return "settlement-return-reason-received-before-send";
    default:
      return null;
  }
}

/** 处理动作的按钮文案键。 */
export function actionKey(value: string): string | null {
  switch (value) {
    case "reclaim_and_resend":
      return "settlement-return-action-reclaim-and-resend";
    case "reclaim_and_hold":
      return "settlement-return-action-reclaim-and-hold";
    case "resend_from_custody":
      return "settlement-return-action-resend-from-custody";
    case "close_inbox":
      return "settlement-return-action-close-inbox";
    case "acknowledge":
      return "settlement-return-action-acknowledge";
    case "recheck":
      return "settlement-return-action-recheck";
    case "retry_take":
      return "settlement-return-action-retry-take";
    default:
      return null;
  }
}

/** 处理动作在确认框中的后果说明键；说明只描述服务端冻结的后果，不提供可编辑的资产或收件人。 */
export function actionEffectKey(value: string): string | null {
  switch (value) {
    case "reclaim_and_resend":
      return "settlement-return-effect-reclaim-and-resend";
    case "reclaim_and_hold":
      return "settlement-return-effect-reclaim-and-hold";
    case "resend_from_custody":
      return "settlement-return-effect-resend-from-custody";
    case "close_inbox":
      return "settlement-return-effect-close-inbox";
    case "acknowledge":
      return "settlement-return-effect-acknowledge";
    case "recheck":
      return "settlement-return-effect-recheck";
    case "retry_take":
      return "settlement-return-effect-retry-take";
    default:
      return null;
  }
}

/** 阻断原因的译文键。 */
export function blockerKey(value: string): string | null {
  switch (value) {
    case "untrusted":
      return "settlement-return-blocker-untrusted";
    case "unbound":
      return "settlement-return-blocker-unbound";
    case "assets_mismatch":
      return "settlement-return-blocker-assets-mismatch";
    case "state_conflict":
      return "settlement-return-blocker-state-conflict";
    case "resend_limit":
      return "settlement-return-blocker-resend-limit";
    case "obligation_ineligible":
      return "settlement-return-blocker-obligation-ineligible";
    case "proof_required":
      return "settlement-return-blocker-proof-required";
    case "send_unresolved":
      return "settlement-return-blocker-send-unresolved";
    case "observation_required":
      return "settlement-return-blocker-observation-required";
    default:
      return null;
  }
}

/** 资产位置的译文键；文案明确区分“已寄出”“仍在 Bot 收件箱”“已领回”与“代管”。 */
export function locationKey(value: string): string | null {
  switch (value) {
    case "unverified":
      return "settlement-return-location-unverified";
    case "returned_in_inbox":
      return "settlement-return-location-returned-in-inbox";
    case "take_in_progress":
      return "settlement-return-location-take-in-progress";
    case "location_unknown":
      return "settlement-return-location-location-unknown";
    case "taken_awaiting_resend":
      return "settlement-return-location-taken-awaiting-resend";
    case "taken_in_custody":
      return "settlement-return-location-taken-in-custody";
    case "resent_in_transit":
      return "settlement-return-location-resent-in-transit";
    default:
      return null;
  }
}

/** 补寄案的结案证据只证明寄出；案件不再跟踪后续持有人或领取状态。 */
export function showsAssetLocation(status: string): boolean {
  return status !== "resent";
}

/** 处理计划的译文键。 */
export function planKey(value: string): string | null {
  switch (value) {
    case "resend":
      return "settlement-return-plan-resend";
    case "hold":
      return "settlement-return-plan-hold";
    default:
      return null;
  }
}

/** 处理事件的译文键。 */
export function eventKey(value: string): string | null {
  switch (value) {
    case "opened":
      return "settlement-return-event-opened";
    case "acknowledged":
      return "settlement-return-event-acknowledged";
    case "recheck_requested":
      return "settlement-return-event-recheck-requested";
    case "recheck_rebound":
      return "settlement-return-event-recheck-rebound";
    case "recheck_unchanged":
      return "settlement-return-event-recheck-unchanged";
    case "take_requested":
      return "settlement-return-event-take-requested";
    case "take_activated":
      return "settlement-return-event-take-activated";
    case "take_proven":
      return "settlement-return-event-take-proven";
    case "take_unknown":
      return "settlement-return-event-take-unknown";
    case "take_observed":
      return "settlement-return-event-take-observed";
    case "take_retry_requested":
      return "settlement-return-event-take-retry-requested";
    case "held":
      return "settlement-return-event-held";
    case "resend_authorized":
      return "settlement-return-event-resend-authorized";
    case "resent":
      return "settlement-return-event-resent";
    case "inbox_closed":
      return "settlement-return-event-inbox-closed";
    default:
      return null;
  }
}

/** 义务种类的译文键。 */
export function kindKey(value: string): string | null {
  switch (value) {
    case "winner_equipment_cod":
      return "settlement-return-kind-winner-equipment-cod";
    case "winner_payment_carrier_cod":
      return "settlement-return-kind-winner-payment-carrier-cod";
    case "winner_equipment_handoff":
      return "settlement-return-kind-winner-equipment-handoff";
    case "seller_payout":
      return "settlement-return-kind-seller-payout";
    case "buyer_refund":
      return "settlement-return-kind-buyer-refund";
    case "unsold_return":
      return "settlement-return-kind-unsold-return";
    case "withdrawn_return":
      return "settlement-return-kind-withdrawn-return";
    default:
      return null;
  }
}

/** Take 结果未知原因的译文键（原因名称）。 */
export function takeUnknownReasonKey(value: string): string | null {
  switch (value) {
    case "assets_claimable":
      return "settlement-return-take-unknown-reason-assets-claimable";
    case "assets_unclaimable":
      return "settlement-return-take-unknown-reason-assets-unclaimable";
    case "assets_changed":
      return "settlement-return-take-unknown-reason-assets-changed";
    case "mail_missing":
      return "settlement-return-take-unknown-reason-mail-missing";
    case "receipt_unproven":
      return "settlement-return-take-unknown-reason-receipt-unproven";
    case "session_changed":
      return "settlement-return-take-unknown-reason-session-changed";
    case "take_unverifiable":
      return "settlement-return-take-unknown-reason-take-unverifiable";
    case "disposition_conflict":
      return "settlement-return-take-unknown-reason-disposition-conflict";
    case "awaiting_observation":
      return "settlement-return-take-unknown-reason-awaiting-observation";
    default:
      return null;
  }
}

/** Take 结果未知原因的运维指引译文键；与告警通知的指引同一口径。 */
export function takeUnknownGuidanceKey(value: string): string | null {
  switch (value) {
    case "assets_claimable":
      return "settlement-return-take-unknown-guidance-assets-claimable";
    case "assets_unclaimable":
      return "settlement-return-take-unknown-guidance-assets-unclaimable";
    case "assets_changed":
      return "settlement-return-take-unknown-guidance-assets-changed";
    case "mail_missing":
      return "settlement-return-take-unknown-guidance-mail-missing";
    case "receipt_unproven":
      return "settlement-return-take-unknown-guidance-receipt-unproven";
    case "session_changed":
      return "settlement-return-take-unknown-guidance-session-changed";
    case "take_unverifiable":
      return "settlement-return-take-unknown-guidance-take-unverifiable";
    case "disposition_conflict":
      return "settlement-return-take-unknown-guidance-disposition-conflict";
    case "awaiting_observation":
      return "settlement-return-take-unknown-guidance-awaiting-observation";
    default:
      return null;
  }
}

/** 持久 Take 执行状态的译文键。 */
export function takeStateKey(value: string): string | null {
  switch (value) {
    case "pending":
      return "settlement-return-take-state-pending";
    case "enqueued":
      return "settlement-return-take-state-enqueued";
    case "satisfied":
      return "settlement-return-take-state-satisfied";
    case "manual_reconcile":
      return "settlement-return-take-state-manual-reconcile";
    default:
      return null;
  }
}

/** 取闭集译文；键缺失（解码已拒绝，理论上不会发生）时原样显示。 */
export function label(key: (value: string) => string | null, value: string): string {
  const messageId = key(value);
  return messageId === null ? value : t(messageId);
}

/** 事件安全细节的单行文本：按键名排序，只展示标量；嵌套值以省略号代替，不展开任意结构。 */
export function eventDetailsText(details: unknown): string {
  if (typeof details !== "object" || details === null || Array.isArray(details)) {
    return "";
  }
  const entries = Object.entries(details);
  entries.sort((left, right) => compareText(left[0], right[0]));
  return entries
    .map(([key, value]) => {
      const text = scalarText(value);
      return `${key}=${text}`;
    })
    .join(" · ");
}

function scalarText(value: unknown): string {
  if (value === null) {
    return "-";
  }
  if (typeof value === "boolean") {
    return value ? "true" : "false";
  }
  if (typeof value === "number") {
    return String(value);
  }
  if (typeof value === "string") {
    return value;
  }
  return "…";
}

/** 投递来源为结算退件 Take 时，取出其案件 ID（`settlement_return:<案件 ID>`）；只接受规范 UUID。 */
export function settlementReturnCase(originKind: string, originId: string): string | null {
  if (originKind !== "settlement_return_action") {
    return null;
  }
  const prefix = "settlement_return:";
  if (!originId.startsWith(prefix)) {
    return null;
  }
  const caseId = originId.slice(prefix.length);
  return validUuid(caseId) ? caseId : null;
}
