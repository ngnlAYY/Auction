// 冻结 Rust 测试中的 Worker 热读字段清单及拒绝项。

import { describe, expect, test } from "bun:test";

import { HOT_FIELDS, isHot } from "./hot-fields";

// 此固定列表必须与 auction-application 的 ports/admin/runtime_config/hot.rs 同步。
const WORKER_HOT_FIELDS: readonly string[] = [
  "operations.mail.send_intervals.outbid_seconds",
  "operations.mail.send_intervals.account_seconds",
  "operations.mail.send_intervals.intake_seconds",
  "operations.mail.send_intervals.settlement_seconds",
  "operations.mail.send_intervals.equipment_seconds",
  "operations.mail.send_intervals.operations_seconds",
  "operations.mail.send_intervals.manual_seconds",
  "operations.mail.batch_size",
  "operations.mail.jitter_seconds",
  "operations.mail.fetch_attempts",
  "operations.mail.fetch_timeout_seconds",
  "operations.mail.process_attempts",
  "operations.mail.item_timeout_seconds",
  "operations.mail.batch_timeout_seconds",
  "operations.mail.retry_base_seconds",
  "operations.mail.retry_maximum_seconds",
  "operations.mail.account_reply_window_seconds",
  "operations.mail.explanation_mail_window_seconds",
  "operations.mail.explanation_mail_limit",
  "operations.mail.send_classes.account_reply",
  "operations.mail.send_classes.intake_reply",
  "operations.mail.send_classes.settlement_notice",
  "operations.mail.send_classes.outbid_reminder",
  "operations.mail.send_classes.player_summary",
  "operations.mail.send_classes.operations_alert",
  "operations.forum.batch_size",
  "operations.forum.poll_interval_seconds",
  "operations.forum.jitter_seconds",
  "operations.forum.item_timeout_seconds",
  "operations.worker.delivery.batch_limit",
  "operations.worker.delivery.poll_interval_seconds",
  "operations.worker.delivery.jitter_seconds",
  "operations.worker.delivery.request_timeout_seconds",
  "operations.worker.delivery.finalize_timeout_seconds",
];

describe("hot fields", () => {
  test("hot_field_classification_matches_the_worker_contract", () => {
    expect(HOT_FIELDS).toEqual(WORKER_HOT_FIELDS);
    for (const path of WORKER_HOT_FIELDS) {
      expect(isHot(path)).toBe(true);
    }
    for (const path of [
      "operations.mail.managed",
      "operations.mail.rekey_enabled",
      "metrics.enabled",
    ]) {
      expect(isHot(path)).toBe(false);
    }
  });
});
