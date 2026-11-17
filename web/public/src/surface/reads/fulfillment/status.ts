// 履约状态的译文键与语气样式映射。

/** 状态闭集到译文键；解码器已拒绝闭集外的取值，兜底只为不让页面因新状态崩溃。 */
export function statusKey(status: string): string {
  switch (status) {
    case "unsold_return_pending":
      return "fulfillment-status-unsold-return-pending";
    case "unsold_return_sent":
      return "fulfillment-status-unsold-return-sent";
    case "unsold_returned":
      return "fulfillment-status-unsold-returned";
    case "unsold_return_on_hold":
      return "fulfillment-status-unsold-return-on-hold";
    case "payment_mail_pending":
      return "fulfillment-status-payment-mail-pending";
    case "awaiting_payment":
      return "fulfillment-status-awaiting-payment";
    case "payment_overdue":
      return "fulfillment-status-payment-overdue";
    case "payment_on_hold":
      return "fulfillment-status-payment-on-hold";
    case "returned_by_buyer":
      return "fulfillment-status-returned-by-buyer";
    case "returned_on_hold":
      return "fulfillment-status-returned-on-hold";
    case "cod_resend_pending":
      return "fulfillment-status-cod-resend-pending";
    case "cod_resent_awaiting_payment":
      return "fulfillment-status-cod-resent-awaiting-payment";
    case "cod_return_limit_reached":
      return "fulfillment-status-cod-return-limit-reached";
    case "handoff_pending":
      return "fulfillment-status-handoff-pending";
    case "handoff_sent":
      return "fulfillment-status-handoff-sent";
    case "handoff_on_hold":
      return "fulfillment-status-handoff-on-hold";
    case "handoff_returned_by_buyer":
      return "fulfillment-status-handoff-returned-by-buyer";
    case "payout_pending":
      return "fulfillment-status-payout-pending";
    case "payout_sent":
      return "fulfillment-status-payout-sent";
    case "payout_on_hold":
      return "fulfillment-status-payout-on-hold";
    case "completed":
      return "fulfillment-status-completed";
    case "equipment_lost":
      return "fulfillment-status-equipment-lost";
    case "voided_return_pending":
      return "fulfillment-status-voided-return-pending";
    case "voided_return_sent":
      return "fulfillment-status-voided-return-sent";
    case "voided_returned":
      return "fulfillment-status-voided-returned";
    case "voided_on_hold":
      return "fulfillment-status-voided-on-hold";
    case "voided_refund_pending":
      return "fulfillment-status-voided-refund-pending";
    case "voided_relisted":
      return "fulfillment-status-voided-relisted";
    case "voided_relist_pending":
      return "fulfillment-status-voided-relist-pending";
    case "voided_relist_unavailable":
      return "fulfillment-status-voided-relist-unavailable";
    default:
      return "fulfillment-status-unknown";
  }
}

/** 颜色只是辅助提示，语义始终由状态文字表达；未知值保留中性样式。 */
export function statusTone(status: string): string {
  switch (status) {
    case "completed":
    case "unsold_returned":
      return "public-fulfillment__status public-fulfillment__status--done";
    case "unsold_return_pending":
    case "unsold_return_sent":
    case "payment_mail_pending":
    case "awaiting_payment":
    case "cod_resend_pending":
    case "cod_resent_awaiting_payment":
    case "handoff_pending":
    case "handoff_sent":
    case "payout_pending":
    case "payout_sent":
      return "public-fulfillment__status public-fulfillment__status--progress";
    case "unsold_return_on_hold":
    case "payment_overdue":
    case "payment_on_hold":
    case "returned_by_buyer":
    case "returned_on_hold":
    case "cod_return_limit_reached":
    case "handoff_on_hold":
    case "handoff_returned_by_buyer":
    case "payout_on_hold":
    case "equipment_lost":
      return "public-fulfillment__status public-fulfillment__status--attention";
    default:
      return status.startsWith("voided_")
        ? "public-fulfillment__status public-fulfillment__status--voided"
        : "public-fulfillment__status public-fulfillment__status--unknown";
  }
}
