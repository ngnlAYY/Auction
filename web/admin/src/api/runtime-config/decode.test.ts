// 运行配置文档的公开解码入口：投影、长值、负数边界与错误形状。

import { afterEach, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { utf8ByteLength } from "@auction/web-shared/text";

import {
  asAdminJsonValue,
  createAdminRequestHarness,
  TEST_SESSION_BODY,
} from "../api-request-fixture";
import { fetchAdminRuntimeConfig, validateAdminRuntimeConfig } from "./index";
import {
  createRuntimeConfigFixtures,
  jsonObjectAt,
  setAt,
  valueAt,
} from "./runtime-config-fixture";

const FIXTURES = new URL(
  "../../../../../crates/auction-contracts/tests/fixtures/admin/",
  import.meta.url,
);

function fixtureText(name: string): string {
  return readFileSync(new URL(`${name}.json`, FIXTURES), "utf8");
}

const {
  cloneProjection,
  defaultDocument,
  defaultProjection,
  runtimeProjection,
  workerRestartState,
} = createRuntimeConfigFixtures(fixtureText);

const EXTENDED_URL =
  "/api/v1/admin/config?include_equipment_taxonomy=true&include=fulfillment_collection_overdue_age_seconds&include_worker_restart_state=true";
const VALIDATE_URL =
  "/api/v1/admin/config:validate?include_equipment_taxonomy=true&include=fulfillment_collection_overdue_age_seconds";
const SESSION_URL = "/api/v1/admin/session?include=server_time";
const INVALID_RESPONSE = {
  ok: false,
  error: { status: null, code: "invalid_response" },
} as const;

const requests = createAdminRequestHarness();
afterEach(() => {
  requests.cleanup();
});

async function validateWithBody(
  responseBody: string,
): Promise<Awaited<ReturnType<typeof validateAdminRuntimeConfig>>> {
  requests.installBrowserRequest();
  const registration = requests.registerHost();
  const recorded = requests.installFetch([
    { body: JSON.stringify(TEST_SESSION_BODY) },
    { body: responseBody },
  ]);
  const result = await validateAdminRuntimeConfig(
    registration.host,
    asAdminJsonValue(defaultDocument()),
    new AbortController().signal,
  );
  expect(recorded.map(({ method, url }) => [method, url])).toEqual([
    ["GET", SESSION_URL],
    ["POST", VALIDATE_URL],
  ]);
  return result;
}

test("解码默认后端投影", async () => {
  const response = await validateWithBody(JSON.stringify(defaultProjection()));
  expect(response.ok).toBe(true);
  if (!response.ok) {
    throw new Error("默认投影应通过解码");
  }
  expect(response.value.equipment_taxonomy).not.toBeNull();
  expect(
    valueAt(
      response.value.document,
      "/operations/alerts/fulfillment_collection_overdue_age_seconds",
    ),
  ).toBe(259_200);
});

test("解码带长路径与追踪端点的后端投影", async () => {
  const response = await validateWithBody(
    JSON.stringify(runtimeProjection("runtime-config-long-values")),
  );
  expect(response.ok).toBe(true);
  if (!response.ok) {
    throw new Error("长值投影应通过解码");
  }
  const prefix = valueAt(response.value.document, "/equipment_source/allowed_path_prefixes/0");
  const endpoint = valueAt(response.value.document, "/tracing/endpoint");
  expect(typeof prefix === "string" ? utf8ByteLength(prefix) : -1).toBe(602);
  expect(typeof endpoint === "string" ? utf8ByteLength(endpoint) : -1).toBe(2134);
});

test("原样解码带禁用来源条目的后端投影", async () => {
  const response = await validateWithBody(
    JSON.stringify(runtimeProjection("runtime-config-disabled-origins")),
  );
  expect(response.ok).toBe(true);
  if (!response.ok) {
    throw new Error("禁用来源投影应通过解码");
  }
  const security = jsonObjectAt(response.value.document, "/security");
  expect(security.allowed_origins_enabled).toBe(false);
  expect(security.allowed_origins).toEqual(["", " https://example.invalid ", ""]);
});

test("值的格式错误不阻止读取运行配置", async () => {
  const projection = defaultProjection();
  setAt(projection.data.document, "/security/trusted_proxy_cidrs", ["bad-cidr"]);
  setAt(projection.data.document, "/security/allowed_origins", [
    "https://example.invalid",
    "https://example.invalid/",
  ]);
  setAt(projection.data.document, "/equipment_source/allowed_path_prefixes", [
    "/equip/../",
    "/equip%2F/",
  ]);
  setAt(projection.data.document, "/tracing/enabled", true);
  setAt(projection.data.document, "/tracing/endpoint", "https://example.invalid/?query");
  expect((await validateWithBody(JSON.stringify(projection))).ok).toBe(true);

  // 读取走 Worker 扩展投影，文档格式容错应与预校验共用的解码规则一致。
  projection.data.worker_restart_state = workerRestartState();
  const registration = requests.registerHost();
  const recorded = requests.installFetch([{ body: JSON.stringify(projection) }]);
  const response = await fetchAdminRuntimeConfig(registration.host, new AbortController().signal);
  expect(recorded.map(({ method, url }) => [method, url])).toEqual([["GET", EXTENDED_URL]]);
  expect(response.ok).toBe(true);
  if (!response.ok) {
    throw new Error("格式异常的文档应可读取");
  }
  expect(response.value.document).toEqual(projection.data.document);
});

test("同时保留新旧颜色选择", async () => {
  const projection = defaultProjection();
  const rule = {
    category: "Cloth",
    categories: ["Light"],
    equipment_type: "Phase",
    equipment_types: ["Cotton"],
    quality: "Legendary",
    qualities: ["Peerless"],
    level: { min: 0, max: 0 },
    prefixes: ["Radiant"],
    suffixes: [],
    prefix_color: "#ff0000",
    suffix_color: "#00aa00",
    combined_color: "#aa00aa",
  };
  setAt(projection.data.document, "/equipment_colors/rules", [rule]);
  const response = await validateWithBody(JSON.stringify(projection));
  expect(response.ok).toBe(true);
  if (!response.ok) {
    throw new Error("新旧颜色选择应同时保留");
  }
  expect(response.value.document).toEqual(cloneProjection(projection).data.document);

  // 负例只改被测的 `qualities`，其余字段保持正例原样。
  const invalid = cloneProjection(projection);
  expect(valueAt(invalid.data.document, "/equipment_colors/rules/0/prefixes")).toEqual(["Radiant"]);
  setAt(invalid.data.document, "/equipment_colors/rules/0/qualities", [false]);
  expect(await validateWithBody(JSON.stringify(invalid))).toEqual(INVALID_RESPONSE);
});

test("保留额外的看板邮箱", async () => {
  const projection = defaultProjection();
  const mailboxes = [
    { world_id: "main", mailbox_key: "mail-1" },
    { world_id: "isekai", mailbox_key: "mail-2" },
    { world_id: "main", mailbox_key: "mail-3" },
  ];
  setAt(projection.data.document, "/dashboard_mailboxes", mailboxes);
  const response = await validateWithBody(JSON.stringify(projection));
  expect(response.ok).toBe(true);
  if (!response.ok) {
    throw new Error("额外看板邮箱应被保留");
  }
  expect(response.value.document).toEqual(cloneProjection(projection).data.document);

  const invalid = cloneProjection(projection);
  setAt(invalid.data.document, "/dashboard_mailboxes/0/mailbox_key", []);
  expect(await validateWithBody(JSON.stringify(invalid))).toEqual(INVALID_RESPONSE);
});

test("有符号文档字段接受安全负数", async () => {
  const signedFields = [
    "/operations/economics/start_price",
    "/operations/economics/bid_increment_ppm",
    "/operations/economics/soft_close_window_seconds",
    "/operations/economics/soft_close_extension_seconds",
    "/operations/economics/cancel_cutoff_seconds",
    "/operations/economics/proxy_slow_interval_seconds",
    "/operations/economics/final_phase_lead_seconds",
    "/operations/alerts/delivery_pending_count_threshold",
    "/operations/alerts/delivery_manual_reconcile_count_threshold",
    "/operations/alerts/fulfillment_backlog_count_threshold",
    "/operations/alerts/recovery_overdue_count_threshold",
    "/operations/worker/recovery/low_balance_threshold",
    "/equipment_filter/level/min",
    "/equipment_filter/level/max",
  ];
  for (const path of signedFields) {
    const negative = defaultProjection();
    setAt(negative.data.document, path, -1);
    const response = await validateWithBody(JSON.stringify(negative));
    expect(response.ok, path).toBe(true);
    if (!response.ok) {
      throw new Error(`${path} 应接受 -1`);
    }
    expect(valueAt(response.value.document, path), path).toBe(-1);

    const unsafe = defaultProjection();
    setAt(unsafe.data.document, path, -9_007_199_254_740_992);
    expect(await validateWithBody(JSON.stringify(unsafe)), path).toEqual(INVALID_RESPONSE);
  }
  for (const path of [
    "/operations/economics/proxy_max_outbid_uses",
    "/operations/alerts/delivery_pending_age_seconds",
    "/operations/worker/recovery/batch_limit",
  ]) {
    const projection = defaultProjection();
    setAt(projection.data.document, path, -1);
    expect(await validateWithBody(JSON.stringify(projection)), path).toEqual(INVALID_RESPONSE);
  }

  const colors = defaultProjection();
  setAt(colors.data.document, "/equipment_colors/rules", [
    {
      category: "",
      equipment_type: "",
      quality: "",
      level: { min: -1, max: -1 },
      prefixes: [],
      suffixes: [],
      prefix_color: "#ff0000",
      suffix_color: "#00aa00",
      combined_color: "#aa00aa",
    },
  ]);
  const response = await validateWithBody(JSON.stringify(colors));
  expect(response.ok).toBe(true);
  if (!response.ok) {
    throw new Error("装备颜色等级应接受安全负数");
  }
  expect(valueAt(response.value.document, "/equipment_colors/rules/0/level")).toEqual({
    min: -1,
    max: -1,
  });
});

test("拒绝错误形状与不安全整数", async () => {
  for (const [path, invalid] of [
    ["/security/trusted_proxy_cidrs", [{}]],
    ["/equipment_source/allowed_path_prefixes", [null]],
    ["/tracing/endpoint", { url: "https://example.invalid/" }],
    ["/operations/alerts/fulfillment_collection_overdue_age_seconds", 9_007_199_254_740_992],
    ["/listener/mode", "unknown"],
  ] as const) {
    const projection = defaultProjection();
    setAt(projection.data.document, path, invalid);
    expect(await validateWithBody(JSON.stringify(projection)), path).toEqual(INVALID_RESPONSE);
  }
});
