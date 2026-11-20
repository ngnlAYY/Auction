// 运行参数响应的字段闭集、枚举及安全整数校验。

import { jsonExactObject } from "@auction/web-shared/net/wire";

import {
  fieldValue,
  hasField,
  safeU64,
  validScalars,
  validScalarsWithSigned,
  validStringArray,
} from "./fields";

/** 校验 `operations` 区块的闭集结构与各子区块字段。 */
export function validOperations(value: unknown): boolean {
  const document = jsonExactObject(value, [
    "economics",
    "schedule",
    "forum",
    "alerts",
    "worker",
    "mail",
  ]);
  if (document === null) {
    return false;
  }
  return (
    requiredField(document, "economics", validEconomics) &&
    requiredField(document, "schedule", validSchedule) &&
    requiredField(document, "forum", validForum) &&
    requiredField(document, "alerts", validAlerts) &&
    requiredField(document, "worker", validWorker) &&
    requiredField(document, "mail", validMail)
  );
}

/** 服务端已补齐邮件区块的默认值。缺键即视为响应结构损坏。 */
function validMail(value: unknown): boolean {
  return (
    validScalars(
      value,
      ["managed", "rekey_enabled"],
      ["schedule"],
      [
        "maximum_response_bytes",
        "minimum_request_interval_millis",
        "request_timeout_seconds",
        "batch_size",
        "maximum_pages",
        "jitter_seconds",
        "lease_duration_seconds",
        "renew_interval_seconds",
        "fetch_attempts",
        "fetch_timeout_seconds",
        "process_attempts",
        "item_timeout_seconds",
        "batch_timeout_seconds",
        "retry_base_seconds",
        "retry_maximum_seconds",
        "account_reply_window_seconds",
        "explanation_mail_window_seconds",
        "explanation_mail_limit",
      ],
      ["send_intervals", "send_classes"],
    ) &&
    validSendIntervals(fieldValue(value, "send_intervals")) &&
    validSendClasses(fieldValue(value, "send_classes"))
  );
}

function validSendIntervals(value: unknown): boolean {
  return validScalars(
    value,
    [],
    [],
    [
      "outbid_seconds",
      "account_seconds",
      "intake_seconds",
      "settlement_seconds",
      "equipment_seconds",
      "operations_seconds",
      "manual_seconds",
    ],
    [],
  );
}

function validSendClasses(value: unknown): boolean {
  const object = jsonExactObject(value, [
    "account_reply",
    "intake_reply",
    "settlement_notice",
    "outbid_reminder",
    "player_summary",
    "operations_alert",
  ]);
  return object !== null && Object.values(object).every((item) => typeof item === "boolean");
}

function validEconomics(value: unknown): boolean {
  if (
    !validScalarsWithSigned(
      value,
      ["managed", "proxy_enabled"],
      ["proxy_default_mode"],
      ["proxy_max_outbid_uses"],
      [
        "start_price",
        "bid_increment_ppm",
        "soft_close_window_seconds",
        "soft_close_extension_seconds",
        "cancel_cutoff_seconds",
        "proxy_slow_interval_seconds",
        "final_phase_lead_seconds",
      ],
      [],
    )
  ) {
    return false;
  }
  const mode = fieldValue(value, "proxy_default_mode");
  return mode === "fast" || mode === "slow";
}

function validSchedule(value: unknown): boolean {
  // 早鸟区块可省略；直接校验借用字段，不复制整份排期后递归移除可选区块。
  let nested: readonly string[] = ["worlds"];
  // 用 `hasField` 区分缺键与显式 `null`；显式 `null` 仍交给早鸟校验并拒绝。
  if (hasField(value, "early_bird")) {
    const earlyBird = fieldValue(value, "early_bird");
    if (!validEarlyBird(earlyBird)) {
      return false;
    }
    nested = ["worlds", "early_bird"];
  }
  if (
    !validScalars(
      value,
      ["managed", "enabled"],
      [
        "timezone",
        "preview_day",
        "preview_time",
        "open_day",
        "open_time",
        "nominal_end_day",
        "nominal_end_time",
      ],
      [],
      nested,
    )
  ) {
    return false;
  }
  return (
    validWorldList(fieldValue(value, "worlds")) &&
    ["preview_day", "open_day", "nominal_end_day"].every((name) =>
      isWeekday(fieldValue(value, name)),
    )
  );
}

function validWorldList(value: unknown): boolean {
  return Array.isArray(value) && value.every((world) => world === "main" || world === "isekai");
}

function validEarlyBird(value: unknown): boolean {
  const booleans = ["enabled", "pause_intake_before_season"];
  const numbers = ["window_days", "preview_lead_minutes", "live_minutes"];
  // 赛季前场次的两项默认值由后端各自省略。
  if (hasField(value, "pre_season_session")) {
    booleans.push("pre_season_session");
  }
  if (hasField(value, "pre_season_days_before")) {
    numbers.push("pre_season_days_before");
  }
  return (
    validScalars(value, booleans, ["timezone", "season_start_time", "open_time"], numbers, [
      "worlds",
      "season_starts",
      "session_days",
      "qualities",
    ]) &&
    validWorldList(fieldValue(value, "worlds")) &&
    validStringArray(fieldValue(value, "season_starts")) &&
    isSafeU64Array(fieldValue(value, "session_days")) &&
    validStringArray(fieldValue(value, "qualities"))
  );
}

function validForum(value: unknown): boolean {
  return validScalars(
    value,
    ["managed", "enabled"],
    [],
    [
      "batch_size",
      "poll_interval_seconds",
      "jitter_seconds",
      "maximum_response_bytes",
      "maximum_topic_pages",
      "minimum_request_interval_millis",
      "request_timeout_seconds",
      "item_timeout_seconds",
      "lease_duration_seconds",
    ],
    [],
  );
}

function validAlerts(value: unknown): boolean {
  if (
    !validScalarsWithSigned(
      value,
      ["managed", "enabled"],
      ["world_id", "recipient"],
      [
        "scan_interval_seconds",
        "jitter_seconds",
        "source_timeout_seconds",
        "batch_window_seconds",
        "cooldown_seconds",
        "claim_lease_seconds",
        "retry_delay_seconds",
        "max_signals_per_scan",
        "max_batch_items",
        "max_body_bytes",
        "delivery_pending_age_seconds",
        "fulfillment_backlog_age_seconds",
        "fulfillment_collection_overdue_age_seconds",
        "recovery_overdue_age_seconds",
      ],
      [
        "delivery_pending_count_threshold",
        "delivery_manual_reconcile_count_threshold",
        "fulfillment_backlog_count_threshold",
        "recovery_overdue_count_threshold",
      ],
      [],
    )
  ) {
    return false;
  }
  const worldId = fieldValue(value, "world_id");
  return worldId === "main" || worldId === "isekai";
}

function validWorker(value: unknown): boolean {
  return (
    validScalars(
      value,
      ["managed"],
      [],
      [
        "concurrency",
        "poll_interval_seconds",
        "batch_limit",
        "jitter_seconds",
        "item_timeout_seconds",
        "recovery_retry_delay_seconds",
      ],
      ["delivery", "fulfillment", "recovery"],
    ) &&
    requiredField(value, "delivery", validWorkerDelivery) &&
    requiredField(value, "fulfillment", validWorkerFulfillment) &&
    requiredField(value, "recovery", validWorkerRecovery)
  );
}

function validWorkerDelivery(value: unknown): boolean {
  return validScalars(
    value,
    [],
    [],
    [
      "batch_limit",
      "poll_interval_seconds",
      "jitter_seconds",
      "lease_duration_seconds",
      "renew_interval_seconds",
      "sending_grace_seconds",
      "request_timeout_seconds",
      "finalize_timeout_seconds",
    ],
    [],
  );
}

function validWorkerFulfillment(value: unknown): boolean {
  return validScalars(
    value,
    [],
    [],
    [
      "batch_limit",
      "concurrency",
      "poll_interval_seconds",
      "jitter_seconds",
      "lease_duration_seconds",
      "item_timeout_seconds",
      "retry_delay_seconds",
    ],
    [],
  );
}

function validWorkerRecovery(value: unknown): boolean {
  return validScalarsWithSigned(
    value,
    [],
    [],
    [
      "batch_limit",
      "concurrency",
      "poll_interval_seconds",
      "jitter_seconds",
      "lease_duration_seconds",
      "item_timeout_seconds",
      "health_refresh_interval_seconds",
      "evidence_stale_after_seconds",
    ],
    ["low_balance_threshold"],
    [],
  );
}

function isWeekday(value: unknown): boolean {
  return (
    value === "Monday" ||
    value === "Tuesday" ||
    value === "Wednesday" ||
    value === "Thursday" ||
    value === "Friday" ||
    value === "Saturday" ||
    value === "Sunday"
  );
}

function isSafeU64Array(value: unknown): boolean {
  return Array.isArray(value) && value.every((item) => safeU64(item) !== null);
}

/** Rust `Option::is_some_and`：键必须出现且通过谓词。 */
function requiredField(value: unknown, key: string, predicate: (value: unknown) => boolean) {
  return hasField(value, key) && predicate(fieldValue(value, key));
}
