// 逐项核对 Rust 看板聚合投影的分区解码与失败关闭约束。

import { expect, test } from "bun:test";
import { AdminDashboardResponseSchema } from "@auction/contracts/dto/admin/audit";
import { apiResponseSchema } from "@auction/contracts/dto/response";
import { decode } from "@auction/contracts/wire";
import { err } from "@auction/web-shared/result";
import { jsonObjectAt } from "../api-request-fixture";
import { adminFixtureBody } from "../t11-contract-fixture.test";
import { completePayload, dashboardSnapshot, unknownSection } from "./dashboard-fixture";
import { decodeDashboardResponse } from "./decode";

function withField(payload: unknown, pointer: string, key: string, value?: unknown): unknown {
  const copy = structuredClone(payload);
  const object = jsonObjectAt(copy, pointer);
  if (value === undefined) {
    expect(Object.hasOwn(object, key)).toBe(true);
    delete object[key];
  } else {
    object[key] = value;
  }
  return copy;
}

function payloadWithoutLiveHeartbeats() {
  const summary = {
    status: "degraded",
    readiness: false,
    audit_failure_count: 0,
    missing_services: ["public", "admin", "worker"],
  };
  return {
    ...completePayload(),
    health: summary,
    process: {
      status: "degraded",
      data: { processes: [], summary, observed_at: "0001-01-01T00:00:00Z" },
    },
  };
}

function reconciliationSection(worldCount: number) {
  const world = {
    world: "main",
    custody_missing_count: 0,
    custody_in_transit_count: 5,
    solvency_shortfall_credits: 0,
    solvency_in_transit_count: 31,
    solvency_in_transit_credits: 38_668_000,
  };
  return {
    status: "ok",
    observed_at: "2026-01-01T00:00:00Z",
    data: {
      observed_at: "2026-01-01T00:00:00Z",
      custody_report_age_seconds: 1,
      solvency_report_age_seconds: 1,
      worlds: Array.from({ length: worldCount }, () => ({ ...world })),
    },
  };
}

function realtimeSection(totalLag: number) {
  return {
    status: "ok",
    observed_at: "2026-01-01T00:00:00Z",
    data: {
      observed_at: "2026-01-01T00:00:00Z",
      total_lag: totalLag,
      lags: [],
      failure: { count: 0, oldest_at: null, latest_code: "" },
    },
  };
}

function rejected(payload: unknown, label?: string): void {
  expect(decodeDashboardResponse(payload), label).toEqual(err("invalid_response"));
}

test("degraded_empty_process_section_preserves_the_rest_of_the_snapshot", () => {
  const snapshot = dashboardSnapshot(payloadWithoutLiveHeartbeats());
  expect(snapshot.process.status).toBe("degraded");
  expect(snapshot.process.observed_at).toBeNull();
  expect(snapshot.process.data?.processes).toEqual([]);
  expect(snapshot.process.data?.summary.missing_services).toEqual(["public", "admin", "worker"]);
  expect(snapshot.fulfillment.data?.pending).toBe(2);
});

test("other_observed_sections_still_require_an_observation_time", () => {
  const payload = payloadWithoutLiveHeartbeats();
  expect(decodeDashboardResponse(payload).ok).toBe(true);
  rejected(withField(payload, "/process", "status", "ok"));
  const observedAt = "2026-01-01T00:00:00.000000Z";
  const withHeartbeat = {
    ...payload,
    process: {
      ...payload.process,
      data: {
        ...payload.process.data,
        processes: [
          {
            service_name: "admin",
            process_role: "admin",
            instance_id: "admin-1",
            version: "0.1.0",
            started_at: observedAt,
            status: "ready",
            readiness: true,
            audit_failure_count: 0,
            tracing: {
              status: "disabled",
              dropped_span_count: 0,
              export_failure_count: 0,
              last_success_at: null,
              last_failure_at: null,
              last_error_code: "",
            },
            observed_at: observedAt,
            expires_at: "2026-01-01T00:01:00.000000Z",
          },
        ],
        observed_at: observedAt,
      },
    },
  };
  rejected(withHeartbeat);
  expect(
    decodeDashboardResponse(withField(withHeartbeat, "/process", "observed_at", observedAt)).ok,
  ).toBe(true);
  const complete = completePayload();
  expect(decodeDashboardResponse(complete).ok).toBe(true);
  rejected(withField(complete, "/fulfillment", "observed_at"));
});

test("complete_dashboard_payload_with_waiting_and_webhook_fields_decodes", () => {
  const { fulfillment, alerting, external_accounts: external } = dashboardSnapshot();
  expect(fulfillment.data).toMatchObject({
    waiting_payment: 4,
    waiting_prerequisite: 3,
    oldest_prerequisite_wait_age_seconds: 60,
    awaiting_collection: 5,
    awaiting_collection_unreadable: 2,
    oldest_awaiting_collection_age_seconds: 691_200,
    collection_overdue_count: 3,
  });
  expect(fulfillment.data?.collection_overdue_items).toHaveLength(1);
  expect(fulfillment.data?.collection_overdue_items[0]).toMatchObject({
    lot_number: 12,
    last_read_failed: true,
  });
  expect(alerting.data).toMatchObject({
    webhook_pending_count: 2,
    webhook_manual_reconcile_count: 1,
  });
  expect(external.data?.mail.busy).toBe(true);
  expect(external.data?.mail.busy_age_seconds).toBe(12);
  expect(external.data?.mail.pollers.items[0]?.paused).toBe(true);
  expect(external.data?.maintenance.state).toBe("running");
  expect(external.data?.forum.write_unknown_count).toBe(1);
  expect(external.data?.credentials.items[0]?.available).toBe(1);
});

test("unknown_external_accounts_section_decodes_without_data", () => {
  const snapshot = dashboardSnapshot({
    ...completePayload(),
    worker: { condition: "unavailable", state: "", ready: false, version: "", mail_pollers: 0 },
    external_accounts: unknownSection(),
    fulfillment: unknownSection(),
    alerting: unknownSection(),
  });
  expect(snapshot.external_accounts).toMatchObject({
    data: null,
    status: "unknown",
    error_code: "source_unavailable",
  });
});

test("missing_nullable_or_counter_fields_fail_closed", () => {
  const complete = completePayload();
  expect(decodeDashboardResponse(complete).ok).toBe(true);
  for (const [path, key] of [
    ["/external_accounts/data/mail", "last_success_at"],
    ["/external_accounts/data/mail", "busy_age_seconds"],
    ["/external_accounts/data/mail/pollers/items/0", "paused_since"],
    ["/external_accounts/data/forum", "last_request_at"],
    ["/external_accounts/data/maintenance", "running_age_seconds"],
    ["/alerting/data", "last_failure_at"],
    ["/alerting/data", "webhook_pending_count"],
    ["/alerting/data", "last_error_code"],
  ] as const) {
    rejected(withField(complete, path, key), `${path}/${key}`);
  }
});

test("awaiting_collection_counts_must_be_consistent", () => {
  const path = "/fulfillment/data";
  const complete = completePayload();
  expect(decodeDashboardResponse(complete).ok).toBe(true);
  for (const [key, value] of [
    ["awaiting_collection_unreadable", 6],
    ["awaiting_collection_unreadable", -1],
    ["awaiting_collection", -1],
    ["oldest_awaiting_collection_age_seconds", -1],
    ["collection_overdue_count", 6],
    ["collection_overdue_count", -1],
    ["collection_overdue_count", 0],
  ] as const) {
    rejected(withField(complete, path, key, value), `${key}=${value}`);
  }
  const empty = completePayload();
  Object.assign(empty.fulfillment.data, {
    awaiting_collection: 0,
    awaiting_collection_unreadable: 0,
    collection_overdue_count: 0,
    collection_overdue_items: [],
  });
  rejected(empty);
  const quiet = withField(empty, path, "oldest_awaiting_collection_age_seconds", 0);
  expect(decodeDashboardResponse(quiet).ok).toBe(true);
  rejected(withField(quiet, path, "collection_overdue_count", 1));
});

test("collection_overdue_items_fail_closed", () => {
  const path = "/fulfillment/data/collection_overdue_items/0";
  const complete = completePayload();
  expect(decodeDashboardResponse(complete).ok).toBe(true);
  for (const [key, value] of [
    ["obligation_kind", "winner_equipment_cod"],
    ["obligation_kind", "seller"],
    ["world", "moon"],
    ["auction_sequence", 0],
    ["lot_number", 0],
    ["age_seconds", -1],
    ["recipient", ""],
    ["equipment_name", " "],
    ["sent_at", ""],
    ["obligation_id", ""],
    ["unexpected", 1],
  ] as const) {
    rejected(withField(complete, path, key, value), key);
  }
  for (const kind of [
    "buyer_refund",
    "unsold_return",
    "withdrawn_return",
    "winner_equipment_handoff",
  ]) {
    expect(
      decodeDashboardResponse(withField(complete, path, "obligation_kind", kind)).ok,
      kind,
    ).toBe(true);
  }
});

test("unknown_fields_fail_closed", () => {
  const complete = completePayload();
  expect(decodeDashboardResponse(complete).ok).toBe(true);
  for (const path of [
    "",
    "/health",
    "/worker",
    "/alerting",
    "/alerting/data",
    "/fulfillment/data",
    "/external_accounts/data/forum",
    "/external_accounts/data/credentials/items/0",
  ]) {
    rejected(withField(complete, path, "unexpected", 1), path);
  }
});

test("disabled_alerting_must_be_ok_without_observed_at", () => {
  const disabled = withField(completePayload(), "/alerting/data", "enabled", false);
  rejected(disabled);
  const quiet = withField(disabled, "/alerting", "observed_at");
  expect(decodeDashboardResponse(quiet).ok).toBe(true);
  rejected(withField(quiet, "/alerting", "status", "degraded"));
});

test("enabled_alerting_observed_at_must_match_last_scan", () => {
  rejected(withField(completePayload(), "/alerting", "observed_at", "2026-01-01T00:00:01Z"));
  const missing = withField(completePayload(), "/alerting", "observed_at");
  rejected(missing);
  expect(
    decodeDashboardResponse(withField(missing, "/alerting/data", "last_observed_at", null)).ok,
  ).toBe(true);
});

test("busy_age_must_follow_mail_gate_state", () => {
  const path = "/external_accounts/data/mail";
  const idleWithAge = withField(completePayload(), path, "busy", false);
  rejected(idleWithAge);
  expect(decodeDashboardResponse(withField(idleWithAge, path, "busy_age_seconds", null)).ok).toBe(
    true,
  );
  rejected(withField(completePayload(), path, "busy_age_seconds", null));
  rejected(withField(completePayload(), path, "busy_age_seconds", -1));
});

test("dashboard_mail_pollers_reject_diagnostics", () => {
  const poller = {
    mailbox_key: "main",
    last_poll_at: "2026-01-01T00:00:00Z",
    next_poll_at: "2026-01-01T00:01:00Z",
    consecutive_failures: 0,
    paused: false,
    state_version: 1,
    updated_at: "2026-01-01T00:00:00Z",
  };
  const plain = {
    ...completePayload(),
    mail: {
      status: "ok",
      observed_at: "2026-01-01T00:00:00Z",
      data: { observed_at: "2026-01-01T00:00:00Z", pollers: [poller] },
    },
  };
  expect(decodeDashboardResponse(plain).ok).toBe(true);
  rejected(
    withField(plain, "/mail/data/pollers/0", "diagnostics", {
      observed_at: "2026-01-01T00:00:00Z",
      cursor_status: "empty",
      completed_external_mail_id: null,
      worker_condition: "available",
      worker_observed_at: null,
      progress: null,
    }),
  );
});

const FULFILLMENT_KEYS = [
  "awaiting_collection",
  "awaiting_collection_unreadable",
  "oldest_awaiting_collection_age_seconds",
  "collection_overdue_count",
  "collection_overdue_items",
];
const RECONCILIATION_KEYS = [
  "custody_in_transit_count",
  "solvency_in_transit_count",
  "solvency_in_transit_credits",
];

test("baseline_shaped_sections_are_rejected", () => {
  const complete = completePayload();
  expect(decodeDashboardResponse(complete).ok).toBe(true);
  let baseline: unknown = complete;
  for (const key of FULFILLMENT_KEYS) {
    rejected(withField(complete, "/fulfillment/data", key), key);
    baseline = withField(baseline, "/fulfillment/data", key);
  }
  rejected(baseline);
  const extended = { ...complete, reconciliation: reconciliationSection(1) };
  expect(decodeDashboardResponse(extended).ok).toBe(true);
  for (const key of RECONCILIATION_KEYS) {
    rejected(withField(extended, "/reconciliation/data/worlds/0", key), key);
  }
});

test("reconciliation_in_transit_must_be_non_negative", () => {
  const payload = { ...completePayload(), reconciliation: reconciliationSection(1) };
  const snapshot = dashboardSnapshot(payload);
  expect(snapshot.reconciliation.data?.worlds[0]).toMatchObject({
    custody_in_transit_count: 5,
    solvency_in_transit_credits: 38_668_000,
  });
  for (const key of RECONCILIATION_KEYS) {
    rejected(withField(payload, "/reconciliation/data/worlds/0", key, -1), key);
  }
});

test("semantic_closed_sets_and_bounds_fail_closed", () => {
  for (const [path, key, valid, invalid] of [
    ["/external_accounts/data/maintenance", "state", "idle", "paused"],
    ["/external_accounts/data/mail/pollers", "status", "unavailable", "partial"],
    ["/external_accounts/data/credentials", "status", "unavailable", "stale"],
    ["/external_accounts/data/forum", "write_unknown_count", 0, -1],
    ["", "reconciliation", reconciliationSection(2), reconciliationSection(3)],
    ["", "realtime", realtimeSection(0), realtimeSection(-1)],
  ] as const) {
    expect(decodeDashboardResponse(withField(completePayload(), path, key, valid)).ok, key).toBe(
      true,
    );
    rejected(withField(completePayload(), path, key, invalid), key);
  }
});

test("dashboard_decodes_backend_fixture_with_no_live_heartbeats", () => {
  const baseline = adminFixtureBody("dashboard");
  const wire = decode(apiResponseSchema(AdminDashboardResponseSchema), baseline);
  expect(wire.ok).toBe(true);
  if (!wire.ok) {
    throw new Error("缺省响应外壳无法解码");
  }
  const fulfillment = jsonObjectAt(wire.value.data.fulfillment.data, "");
  expect(fulfillment.pending).toBe(5);
  expect(Object.hasOwn(fulfillment, "awaiting_collection")).toBe(false);
  rejected(wire.value.data);

  const extended = decode(
    apiResponseSchema(AdminDashboardResponseSchema),
    adminFixtureBody("dashboard-collection-wait"),
  );
  expect(extended.ok).toBe(true);
  if (!extended.ok) {
    throw new Error("等待领取响应外壳无法解码");
  }
  const snapshot = dashboardSnapshot(extended.value.data);
  expect(snapshot.process.observed_at).toBeNull();
  expect(snapshot.process.data?.processes).toEqual([]);
  expect(snapshot.fulfillment.data).toMatchObject({
    pending: 2,
    awaiting_collection: 3,
    collection_overdue_count: 1,
  });
  expect(snapshot.fulfillment.data?.collection_overdue_items).toHaveLength(1);
  expect(snapshot.fulfillment.data?.collection_overdue_items[0]).toMatchObject({
    lot_number: 12,
    age_seconds: 180,
    last_read_failed: true,
  });
  expect(snapshot.reconciliation.data?.worlds[0]?.solvency_in_transit_count).toBe(1);
});
