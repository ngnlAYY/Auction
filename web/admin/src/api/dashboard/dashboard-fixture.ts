// Rust 看板解码与渲染用例共用的完整线上载荷。

import { decodeDashboardResponse } from "./decode";

/** 未取得观测结果的分区。 */
export function unknownSection() {
  return { status: "unknown", error_code: "source_unavailable" };
}

/** 创建一份独立的 Rust complete_payload 载荷。 */
export function completePayload() {
  return {
    health: {
      status: "ok",
      readiness: true,
      audit_failure_count: 0,
      missing_services: [],
    },
    worker: {
      condition: "available",
      state: "serving",
      ready: true,
      version: "0.1.0",
      mail_pollers: 1,
    },
    process: unknownSection(),
    mail: unknownSection(),
    realtime: unknownSection(),
    credentials: unknownSection(),
    external_accounts: {
      status: "ok",
      observed_at: "2026-01-01T00:00:00Z",
      data: {
        observed_at: "2026-01-01T00:00:00Z",
        mail: {
          busy: true,
          busy_age_seconds: 12,
          gate_wait_count: 2,
          gate_wait_seconds_total: 3,
          request_count: 5,
          last_request_at: "2026-01-01T00:00:00Z",
          last_success_at: null,
          last_failure_at: null,
          pollers: {
            status: "available",
            items: [
              {
                mailbox_key: "main",
                paused: true,
                paused_since: "2026-01-01T00:00:00Z",
                consecutive_failures: 0,
                last_poll_at: "2026-01-01T00:00:00Z",
                next_poll_at: "2026-01-01T00:01:00Z",
              },
            ],
          },
        },
        forum: {
          last_request_at: null,
          last_success_at: null,
          last_failure_at: null,
          write_unknown_count: 1,
        },
        maintenance: {
          state: "running",
          last_started_at: "2026-01-01T00:00:00Z",
          last_success_at: null,
          last_failure_at: null,
          last_canceled_at: null,
          running_age_seconds: 4,
        },
        credentials: {
          status: "available",
          items: [
            {
              world: "main",
              kind: "mail_cookie",
              available: 1,
              invalid: 0,
            },
          ],
        },
      },
    },
    delivery: unknownSection(),
    recovery: unknownSection(),
    fulfillment: {
      status: "ok",
      observed_at: "2026-01-01T00:00:00Z",
      data: {
        pending: 2,
        blocked: 1,
        failed: 0,
        manual_reconcile: 0,
        waiting_payment: 4,
        waiting_prerequisite: 3,
        awaiting_collection: 5,
        awaiting_collection_unreadable: 2,
        observed_at: "2026-01-01T00:00:00Z",
        oldest_backlog_age_seconds: 120,
        oldest_prerequisite_wait_age_seconds: 60,
        oldest_awaiting_collection_age_seconds: 691200,
        collection_overdue_count: 3,
        collection_overdue_items: [
          {
            obligation_id: "00000000-0000-0000-0000-000000000010",
            obligation_kind: "seller_payout",
            world: "main",
            auction_sequence: 4,
            lot_number: 12,
            equipment_category: "Shield",
            equipment_name: "Radiant Buckler",
            recipient: "collection-seller",
            sent_at: "2026-01-01T00:00:00.000000Z",
            age_seconds: 691200,
            last_read_failed: true,
          },
        ],
      },
    },
    alerting: {
      status: "ok",
      observed_at: "2026-01-01T00:00:00Z",
      data: {
        enabled: true,
        active_signal_count: 1,
        due_signal_count: 0,
        claimed_batch_count: 0,
        succeeded_batch_count: 1,
        failed_batch_count: 0,
        manual_reconcile_count: 0,
        webhook_pending_count: 2,
        webhook_sending_count: 0,
        webhook_failed_count: 0,
        webhook_manual_reconcile_count: 1,
        last_observed_at: "2026-01-01T00:00:00Z",
        last_success_at: null,
        last_failure_at: null,
        last_error_code: "",
      },
    },
    reconciliation: unknownSection(),
  };
}

/** 从 Rust 的完整载荷构造面板快照，先核对真实解码入口接受载荷。 */
export function dashboardSnapshot(payload: unknown = completePayload()) {
  const decoded = decodeDashboardResponse(payload);
  if (!decoded.ok) {
    throw new Error(`看板渲染夹具无法解码：${decoded.error}`);
  }
  return decoded.value;
}
