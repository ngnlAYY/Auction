// Rust 退件校验测试使用的摘要与详情数据。

/** Rust 退件夹具的案件 ID。 */
export const CASE = "0f8fad5b-d9cb-469f-a165-70867728950e";

/** Rust 退件夹具的投递 ID。 */
export const DELIVERY = "1f8fad5b-d9cb-469f-a165-70867728950e";

/** 创建可独立变更的退件摘要。 */
export function returnSummary() {
  return {
    return_id: CASE,
    case_version: 2,
    status: "awaiting_decision",
    match_state: "manual_reconcile",
    reason_code: "returned_assets_match",
    world_id: "main",
    external_mail_id: "993021",
    received_at: "2026-11-16T01:02:03.000000Z",
    obligation_id: "2f8fad5b-d9cb-469f-a165-70867728950e",
    obligation_kind: "seller_payout",
    auction_id: null,
    lot_id: null,
    delivery_id: DELIVERY,
    dispatch_no: 1,
    recipient: "Seller",
    credits: 12000,
    cod: 0,
    attachment_count: 0,
    plan: null,
    resend_limit: 3,
    resends_used: 0,
    asset_location: "returned_in_inbox",
    created_at: "2026-11-16T01:02:03.000000Z",
    updated_at: "2026-11-16T01:02:03.000000Z",
    closed_at: null,
  };
}

/** 创建 Rust 的分页载荷，供业务校验和内存列表组件共用。 */
export function returnPage<T>(items: readonly T[], total: number) {
  return {
    returns: [...items],
    pagination: { page: 1, page_size: 20, total },
  };
}

/** 创建 Rust 的详情载荷；调用方从中单独变更待验证的字段。 */
export function returnDetail() {
  const firstEvent = {
    case_version: 1,
    event_kind: "opened",
    occurred_at: "2026-11-16T01:02:03Z",
    actor_id: null,
    command_id: null,
    details: { dispatch_no: 1 },
  };
  const secondEvent = {
    case_version: 2,
    event_kind: "recheck_rebound",
    occurred_at: "2026-11-16T01:03:03Z",
    actor_id: null,
    command_id: null,
    details: {},
  };
  return {
    firstEvent,
    secondEvent,
    data: {
      summary: returnSummary(),
      events: [firstEvent, secondEvent],
      dispatches: [
        {
          dispatch_no: 1,
          delivery_id: DELIVERY,
          delivery_status: "succeeded",
          sending_at: "2026-11-16T00:00:00Z",
          completed_at: "2026-11-16T00:01:00Z",
          authorized_by: null,
        },
      ],
      take: null,
      resend_dispatch_no: null,
      custody: null,
      allowed_actions: ["reclaim_and_resend", "reclaim_and_hold", "close_inbox"],
      blockers: [
        { action: "resend_from_custody", blocker: "state_conflict" },
        { action: "acknowledge", blocker: "state_conflict" },
        { action: "recheck", blocker: "state_conflict" },
        { action: "retry_take", blocker: "state_conflict" },
      ],
      recheck_unavailable: false,
      take_unavailable: false,
      take_unknown_reason: null,
    },
  };
}
