// 核对 Rust 的告警、导航计数、webhook 及确认和静默线上契约。

import { expect, test } from "bun:test";
import {
  AdminAlertCommandResponseSchema,
  AdminAlertListResponseSchema,
  AdminAlertWebhookListResponseSchema,
  AdminPendingCountsResponseSchema,
} from "@auction/contracts/dto/admin/alerts";
import { apiResponseSchema } from "@auction/contracts/dto/response";
import { decode } from "@auction/contracts/wire";
import { parseJson } from "@auction/web-shared/net/wire";
import { err } from "@auction/web-shared/result";
import {
  type AdminAlertCommandAction,
  acknowledgeAdminAlert,
  fetchAdminAlerts,
  fetchAdminPendingCounts,
  fetchAdminWebhooks,
  prepareAdminAlertAcknowledge,
  prepareAdminAlertSilence,
  silenceAdminAlert,
} from "./alerts";
import {
  createAdminRequestHarness,
  jsonObjectAt,
  type Reply,
  removeAt,
  setAt,
  TEST_SESSION_BODY,
} from "./api-request-fixture";
import type { AdminSessionHost } from "./session";
import { adminFixtureBody, adminFixtureText } from "./t11-contract-fixture.test";

const PENDING_URL = "/api/v1/admin/pending-counts?include=settlement_returns";
const WEBHOOK_URL = "/api/v1/admin/alerts/webhooks?limit=1";
const ALERT_ID = "1796d9b615cd5b873fa150780021903075f6a01c1a16cdfff4ac5976c86e7b1c";
const SESSION_URL = "/api/v1/admin/session?include=server_time";

function envelope(data: unknown): string {
  return JSON.stringify({ code: "ok", message: "", data });
}

function decodedValue<T>(
  result: { readonly ok: true; readonly value: T } | { readonly ok: false },
): T {
  expect(result.ok).toBe(true);
  if (!result.ok) {
    throw new Error("正例未通过解码");
  }
  return result.value;
}

async function exchange<T>(
  replies: readonly Reply[],
  run: (host: AdminSessionHost, signal: AbortSignal) => Promise<T>,
) {
  const harness = createAdminRequestHarness();
  try {
    harness.installBrowserRequest();
    const requests = harness.installFetch(replies);
    const { host } = harness.registerHost();
    const result = await run(host, new AbortController().signal);
    return { result, requests };
  } finally {
    harness.cleanup();
  }
}

async function readAlerts(body: string, status: string, page = 1, size = 50) {
  const { result, requests } = await exchange([{ body }], (host, signal) =>
    fetchAdminAlerts(host, status, page, size, signal),
  );
  expect(requests).toEqual([
    {
      url: `/api/v1/admin/alerts?status=${status}&page=${page}&page_size=${size}&include=incident_generation`,
      method: "GET",
      body: "",
    },
  ]);
  return result;
}

async function readCounts(body: string) {
  const { result, requests } = await exchange([{ body }], fetchAdminPendingCounts);
  expect(requests).toEqual([{ url: PENDING_URL, method: "GET", body: "" }]);
  return result;
}

function alert(source: string) {
  return {
    id: "0".repeat(64),
    kind: "disk_capacity_low",
    scope: "backup",
    source,
    severity: "urgent",
    active: true,
    incident_generation: 1,
    count: 1,
    age_seconds: 0,
    first_seen_at: "2026-11-07T00:00:00Z",
    last_seen_at: "2026-11-07T00:00:00Z",
    resolved_at: null,
    last_notified_at: null,
    next_notify_at: null,
    acknowledged_at: null,
    acknowledged_by: null,
    silenced_until: null,
    runbook_anchor: "alert-runbook-backup",
  };
}

function resolvedAlert(source: string) {
  return { ...alert(source), active: false, resolved_at: "2026-11-08T00:00:00Z" };
}

function alertPage(
  alerts: readonly (ReturnType<typeof alert> | ReturnType<typeof resolvedAlert>)[],
  total: number,
  activeTotal: number,
  resolvedTotal: number,
) {
  return {
    alerts,
    page: 1,
    page_size: 50,
    total,
    active_total: activeTotal,
    resolved_total: resolvedTotal,
  };
}

test("pending_counts_require_the_requested_settlement_returns_field", async () => {
  const counts = { delivery_manual_reconcile: 1, recovery_confirmation: 0, active_alerts: 2 };
  const valid = { ...counts, settlement_returns: 3 };
  const validBody = envelope(valid);
  const schema = apiResponseSchema(AdminPendingCountsResponseSchema);
  expect(decode(schema, decodedValue(parseJson(validBody))).ok).toBe(true);
  expect((await readCounts(validBody)).ok).toBe(true);
  for (const invalid of [
    counts,
    { ...counts, settlement_returns: -1 },
    { ...counts, active_alerts: -1, settlement_returns: 0 },
  ]) {
    expect(await readCounts(envelope(invalid))).toEqual(err("invalid_response"));
  }

  // Rust 在计数谓词拒绝该值；TypeScript 在进入谓词之前由 DTO 拒绝。
  const unsafeReturns = envelope({ ...valid, settlement_returns: 9_007_199_254_740_992 });
  expect(decode(schema, decodedValue(parseJson(unsafeReturns))).ok).toBe(false);
  expect(await readCounts(unsafeReturns)).toEqual(err("invalid_response"));
});

test("filtered_alert_page_total_counts_only_the_selected_category", async () => {
  for (const [status, payload, accepted] of [
    ["active", alertPage([alert("backup")], 1, 1, 3), true],
    ["resolved", alertPage([resolvedAlert("backup")], 3, 1, 3), true],
    ["all", alertPage([alert("backup"), resolvedAlert("wal")], 4, 1, 3), true],
    ["active", alertPage([alert("backup")], 4, 1, 3), false],
    ["resolved", alertPage([resolvedAlert("backup")], 4, 1, 3), false],
    ["all", alertPage([alert("backup")], 3, 1, 3), false],
  ] as const) {
    const result = await readAlerts(envelope(payload), status);
    if (accepted) {
      expect(result.ok).toBe(true);
    } else {
      expect(result).toEqual(err("invalid_response"));
    }
  }
});

test("alert_page_entries_must_match_the_selected_filter", async () => {
  const mixed = [alert("backup"), resolvedAlert("wal")];
  expect(await readAlerts(envelope(alertPage(mixed, 2, 2, 1)), "active")).toEqual(
    err("invalid_response"),
  );
  expect(await readAlerts(envelope(alertPage(mixed, 2, 1, 2)), "resolved")).toEqual(
    err("invalid_response"),
  );
  expect((await readAlerts(envelope(alertPage(mixed, 3, 2, 1)), "all")).ok).toBe(true);
});

test("alert_page_counts_and_bounds_fail_closed", async () => {
  const page = envelope(alertPage([alert("backup")], 1, 1, 0));
  expect((await readAlerts(page, "active")).ok).toBe(true);
  expect(await readAlerts(page, "active", 2, 50)).toEqual(err("invalid_response"));
  expect(await readAlerts(page, "active", 1, 20)).toEqual(err("invalid_response"));
  const invalidFilter = await exchange([], (host, signal) =>
    fetchAdminAlerts(host, "silenced", 1, 50, signal),
  );
  expect(invalidFilter.result).toEqual(err("invalid_request"));
  expect(invalidFilter.requests).toEqual([]);
  const overfull = { ...alertPage([alert("backup"), alert("wal")], 2, 2, 0), page_size: 1 };
  expect(await readAlerts(envelope(overfull), "active", 1, 1)).toEqual(err("invalid_response"));
  const empty = alertPage([], 0, 0, 0);
  for (const invalid of [alertPage([alert("backup")], 0, 0, 0), { ...empty, resolved_total: -1 }]) {
    expect(await readAlerts(envelope(invalid), "active")).toEqual(err("invalid_response"));
  }

  const schema = apiResponseSchema(AdminAlertListResponseSchema);
  const emptyBody = envelope(empty);
  expect(decode(schema, decodedValue(parseJson(emptyBody))).ok).toBe(true);
  expect((await readAlerts(emptyBody, "active")).ok).toBe(true);
  // 分类计数超出安全整数范围时，TypeScript 的 DTO 先于分页谓词拒绝。
  const unsafeResolved = envelope({ ...empty, resolved_total: 9_007_199_254_740_992 });
  expect(decode(schema, decodedValue(parseJson(unsafeResolved))).ok).toBe(false);
  expect(await readAlerts(unsafeResolved, "active")).toEqual(err("invalid_response"));

  // 原始 i64 数字词法与 Rust 一致，避免 JS 序列化先行舍入。
  const overflow =
    '{"code":"ok","message":"","data":{"alerts":[],"page":1,"page_size":50,"total":-9223372036854775808,"active_total":9223372036854775807,"resolved_total":1}}';
  expect(decode(schema, decodedValue(parseJson(overflow))).ok).toBe(false);
  expect(await readAlerts(overflow, "all")).toEqual(err("invalid_response"));

  // Rust 的 i64 样本会先被安全整数 schema 拒绝；两项各自安全、但合计超界时才会到分页校验。
  const unsafeSum = alertPage([], 9_007_199_254_740_991, 9_007_199_254_740_991, 1);
  const safeSum = { ...unsafeSum, resolved_total: 0 };
  expect((await readAlerts(envelope(safeSum), "all")).ok).toBe(true);
  expect(decode(schema, decodedValue(parseJson(envelope(unsafeSum)))).ok).toBe(true);
  expect(await readAlerts(envelope(unsafeSum), "all")).toEqual(err("invalid_response"));
});

test("host_side_alert_sources_are_accepted_by_the_console", async () => {
  for (const source of ["backup", "wal", "reconciliation"]) {
    expect(
      (await readAlerts(envelope(alertPage([alert(source)], 1, 1, 0)), "active")).ok,
      source,
    ).toBe(true);
  }
  expect(
    await readAlerts(envelope(alertPage([alert("unknown_source")], 1, 1, 0)), "active"),
  ).toEqual(err("invalid_response"));
});

test("alert_list_always_requests_the_incident_generation", async () => {
  const body = envelope({ ...alertPage([], 0, 0, 0), page: 2 });
  const valid = await exchange([{ body }], (host, signal) =>
    fetchAdminAlerts(host, "all", 2, 50, signal),
  );
  expect(valid.result.ok).toBe(true);
  expect(valid.requests).toEqual([
    {
      method: "GET",
      url: "/api/v1/admin/alerts?status=all&page=2&page_size=50&include=incident_generation",
      body: "",
    },
  ]);
  const invalid = await exchange([], (host, signal) =>
    fetchAdminAlerts(host, "active&include=unknown", 1, 50, signal),
  );
  expect(invalid.result).toEqual(err("invalid_request"));
  expect(invalid.requests).toEqual([]);
});

test("alert_page_requires_a_nonnegative_integer_incident_generation", async () => {
  const name = "alerts-list";
  const page = decodedValue(await readAlerts(adminFixtureText(name), "active"));
  expect(page.alerts[0]?.incident_generation).toBe(2);
  expect(page.alerts[1]).toMatchObject({
    incident_generation: 1,
    acknowledged_at: "2026-11-18T12:00:00.000000Z",
    silenced_until: "2026-11-18T13:00:00.000000Z",
  });

  const path = "/data/alerts/0/incident_generation";
  const missing = adminFixtureBody(name);
  removeAt(missing, path);
  const schema = apiResponseSchema(AdminAlertListResponseSchema);
  expect(decode(schema, missing).ok).toBe(true);
  expect(await readAlerts(JSON.stringify(missing), "active")).toEqual(err("invalid_response"));
  for (const generation of [-1, 1.5, null]) {
    const invalid = adminFixtureBody(name);
    setAt(invalid, path, generation);
    expect(decode(schema, invalid).ok).toBe(generation !== 1.5);
    expect(await readAlerts(JSON.stringify(invalid), "active")).toEqual(err("invalid_response"));
  }
});

test("frozen_alert_commands_serialize_to_fixed_wire_bytes", () => {
  const id = "01234567-89ab-4cde-8f01-23456789abcd";
  const acknowledge = prepareAdminAlertAcknowledge(1, "disk replaced");
  const silence = prepareAdminAlertSilence(2, 86_400, "maintenance");
  for (const [prepare, expected] of [
    [
      acknowledge,
      '{"command_id":"01234567-89ab-4cde-8f01-23456789abcd","incident_generation":1,"reason":"disk replaced"}',
    ],
    [
      silence,
      '{"command_id":"01234567-89ab-4cde-8f01-23456789abcd","incident_generation":2,"reason":"maintenance","silence_seconds":86400}',
    ],
  ] as const) {
    const prepared = decodedValue(prepare(id));
    expect(prepared.body).toBe(expected);
    expect(decodedValue(prepare(id))).toBe(prepared);
  }
});

test("silence_duration_outside_server_bounds_is_rejected_before_sending", async () => {
  const id = "00000000-0000-0000-0000-000000000001";
  for (const duration of [59, 7 * 24 * 3_600 + 1]) {
    const prepare = prepareAdminAlertSilence(1, duration, "");
    expect(prepare(id)).toMatchObject({ ok: false, error: { code: "invalid_request" } });
    const { result, requests } = await exchange([], (host, signal) =>
      silenceAdminAlert(host, ALERT_ID, id, prepare, signal),
    );
    expect(result).toMatchObject({ ok: false, error: { code: "invalid_request" } });
    expect(requests).toEqual([]);
  }
  for (const duration of [60, 7 * 24 * 3_600]) {
    expect(prepareAdminAlertSilence(1, duration, "")(id).ok).toBe(true);
  }
});

test("alert_and_pending_counts_fixtures_preserve_opt_in_projections", async () => {
  const name = "alerts-list-default";
  const wire = decodedValue(
    decode(apiResponseSchema(AdminAlertListResponseSchema), adminFixtureBody(name)),
  );
  expect(wire.data.alerts.length).toBeGreaterThan(0);
  for (const entry of wire.data.alerts) {
    expect(entry.incident_generation).toBeNull();
  }
  expect(await readAlerts(adminFixtureText(name), "active")).toEqual(err("invalid_response"));
  for (const [sample, count] of [
    ["pending-counts", null],
    ["pending-counts-settlement-returns", 3],
  ] as const) {
    const wire = decodedValue(
      decode(apiResponseSchema(AdminPendingCountsResponseSchema), adminFixtureBody(sample)),
    );
    expect(wire.data.settlement_returns).toBe(count);
    const result = await readCounts(adminFixtureText(sample));
    if (count === null) {
      expect(result).toEqual(err("invalid_response"));
    } else {
      expect(result.ok).toBe(true);
    }
  }
});

async function readWebhooks(body: string, limit: number) {
  return exchange([{ body }], (host, signal) => fetchAdminWebhooks(host, limit, signal));
}

async function sendAlertReceipt(
  body: string,
  action: AdminAlertCommandAction,
  commandId: string,
  alertId: string,
) {
  const send = action === "acknowledge" ? acknowledgeAdminAlert : silenceAdminAlert;
  const prepare =
    action === "acknowledge"
      ? prepareAdminAlertAcknowledge(1, "")
      : prepareAdminAlertSilence(1, 60, "");
  const { result, requests } = await exchange(
    [{ body: JSON.stringify(TEST_SESSION_BODY) }, { body }],
    (host, signal) => send(host, alertId, commandId, prepare, signal),
  );
  expect(requests.map(({ method, url }) => ({ method, url }))).toEqual([
    { method: "GET", url: SESSION_URL },
    { method: "POST", url: `/api/v1/admin/alerts/${alertId}/${action}` },
  ]);
  return result;
}

test("webhook_and_alert_command_contract_fixtures", async () => {
  const webhookName = "alert-webhooks";
  const webhook = await readWebhooks(adminFixtureText(webhookName), 1);
  expect(webhook.requests).toEqual([{ method: "GET", url: WEBHOOK_URL, body: "" }]);
  const entries = decodedValue(webhook.result);
  expect(entries).toHaveLength(1);
  expect(entries[0]).toMatchObject({
    status: "pending",
    attempt_count: 0,
    scope: "auction",
    completed_at: null,
  });
  const badWebhook = adminFixtureBody(webhookName);
  removeAt(badWebhook, "/data/entries/0/attempt_count");
  expect(decode(apiResponseSchema(AdminAlertWebhookListResponseSchema), badWebhook).ok).toBe(false);
  const missingAttempt = await readWebhooks(JSON.stringify(badWebhook), 1);
  expect(missingAttempt.requests).toEqual([{ method: "GET", url: WEBHOOK_URL, body: "" }]);
  expect(missingAttempt.result).toEqual(err("invalid_response"));
  const zeroLimit = await readWebhooks(adminFixtureText(webhookName), 0);
  expect(zeroLimit.result).toEqual(err("invalid_request"));
  expect(zeroLimit.requests).toEqual([]);

  // 合法请求上限为 1；通过扩展夹具数组构造两条独立记录，路径助手只改已有数组槽位。
  const oversized = adminFixtureBody(webhookName);
  const entry = jsonObjectAt(oversized, "/data/entries/0");
  const oversizedEntries = jsonObjectAt(oversized, "/data").entries;
  if (!Array.isArray(oversizedEntries)) {
    throw new Error("webhook 夹具 entries 必须是数组");
  }
  oversizedEntries.push({ ...entry, id: "00000000-0000-4000-8000-000000000021" });
  const oversizedBody = JSON.stringify(oversized);
  expect(decode(apiResponseSchema(AdminAlertWebhookListResponseSchema), oversized).ok).toBe(true);
  const withinLimit = await readWebhooks(oversizedBody, 2);
  expect(withinLimit.requests).toEqual([
    { method: "GET", url: "/api/v1/admin/alerts/webhooks?limit=2", body: "" },
  ]);
  expect(decodedValue(withinLimit.result)).toHaveLength(2);
  const exceedsLimit = await readWebhooks(oversizedBody, 1);
  expect(exceedsLimit.requests).toEqual([{ method: "GET", url: WEBHOOK_URL, body: "" }]);
  expect(exceedsLimit.result).toEqual(err("invalid_response"));

  for (const [sample, action, commandId, silencedUntil] of [
    ["alert-command", "acknowledge", "01234567-89ab-4cde-8f01-23456789abcd", null],
    [
      "alert-silence-command",
      "silence",
      "01234567-89ab-4cde-8f01-23456789abce",
      "2025-11-15T10:46:40.110000Z",
    ],
  ] as const) {
    const text = adminFixtureText(sample);
    const result = decodedValue(await sendAlertReceipt(text, action, commandId, ALERT_ID));
    expect(result).toMatchObject({ id: ALERT_ID, action, silenced_until: silencedUntil });
    const schema = apiResponseSchema(AdminAlertCommandResponseSchema);
    expect(decode(schema, adminFixtureBody(sample)).ok).toBe(true);
    const missingAction = adminFixtureBody(sample);
    removeAt(missingAction, "/data/alert/action");
    expect(decode(schema, missingAction).ok).toBe(false);
    expect(
      await sendAlertReceipt(JSON.stringify(missingAction), action, commandId, ALERT_ID),
    ).toMatchObject({ ok: false, error: { code: "invalid_response" } });
    expect(await sendAlertReceipt(text, action, commandId, "ab".repeat(32))).toMatchObject({
      ok: false,
      error: { code: "invalid_response" },
    });
  }
});
