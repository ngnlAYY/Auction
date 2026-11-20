// 运行配置读取投影与预校验契约样本；负例从同一份样本删除必填字段派生。

import { afterEach, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import {
  type AdminRuntimeConfigResponse,
  AdminRuntimeConfigResponseSchema,
  AdminRuntimeConfigResponseWithWorkerRestartStateSchema,
} from "@auction/contracts/dto/admin/rules";
import { apiResponseSchema } from "@auction/contracts/dto/response";
import { decode } from "@auction/contracts/wire";

import {
  asAdminJsonValue,
  createAdminRequestHarness,
  parseJsonObject,
  type RecordedRequest,
  type Reply,
  TEST_SESSION_BODY,
} from "../api-request-fixture";
import { adminApiFailureFromHttpBody } from "../failure";
import { fetchAdminRuntimeConfig, validateAdminRuntimeConfig } from "./index";
import {
  createRuntimeConfigFixtures,
  jsonObjectAt,
  removeAt,
  valueAt,
} from "./runtime-config-fixture";

const FIXTURES = new URL(
  "../../../../../crates/auction-contracts/tests/fixtures/admin/",
  import.meta.url,
);

function fixtureText(name: string): string {
  return readFileSync(new URL(`${name}.json`, FIXTURES), "utf8");
}

const { defaultDocument, workerRestartState } = createRuntimeConfigFixtures(fixtureText);

const EXTENDED_URL =
  "/api/v1/admin/config?include_equipment_taxonomy=true&include=fulfillment_collection_overdue_age_seconds&include_worker_restart_state=true";
const VALIDATE_URL =
  "/api/v1/admin/config:validate?include_equipment_taxonomy=true&include=fulfillment_collection_overdue_age_seconds";
const SESSION_URL = "/api/v1/admin/session?include=server_time";
const CONFIG_RESPONSE_SCHEMA = apiResponseSchema(AdminRuntimeConfigResponseSchema);
const EXTENDED_CONFIG_RESPONSE_SCHEMA = apiResponseSchema(
  AdminRuntimeConfigResponseWithWorkerRestartStateSchema,
);
const INVALID_RESPONSE = {
  ok: false,
  error: { status: null, code: "invalid_response" },
} as const;

const requests = createAdminRequestHarness();
afterEach(() => {
  requests.cleanup();
});

async function readWithReplies(replies: readonly Reply[]): Promise<{
  readonly result: Awaited<ReturnType<typeof fetchAdminRuntimeConfig>>;
  readonly requests: readonly RecordedRequest[];
}> {
  const registration = requests.registerHost();
  const recorded = requests.installFetch(replies);
  const result = await fetchAdminRuntimeConfig(registration.host, new AbortController().signal);
  return { result, requests: [...recorded] };
}

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

function expectDecodedProjection(
  projection: unknown,
  name: string,
  taxonomy: boolean,
  threshold: boolean,
): AdminRuntimeConfigResponse {
  const decoded = decode(CONFIG_RESPONSE_SCHEMA, projection);
  expect(decoded.ok, name).toBe(true);
  if (!decoded.ok) {
    throw new Error(`${name} 应通过响应 DTO 解码`);
  }
  const data = decoded.value.data;
  expect(data.equipment_taxonomy !== undefined && data.equipment_taxonomy !== null, name).toBe(
    taxonomy,
  );
  const alerts = jsonObjectAt(data.document, "/operations/alerts");
  expect(Object.hasOwn(alerts, "fulfillment_collection_overdue_age_seconds"), name).toBe(threshold);
  return data;
}

test("运行配置读取投影要求编辑器同时请求两个扩展字段", async () => {
  for (const [name, taxonomy, threshold] of [
    ["runtime-config-unextended", false, false],
    ["runtime-config-taxonomy", true, false],
    ["runtime-config-collection-threshold", false, true],
  ] as const) {
    const projection = parseJsonObject(fixtureText(name), name);
    expectDecodedProjection(projection, name, taxonomy, threshold);
    // 同一份响应先通过扩展 DTO 解码，再由编辑器校验拒绝。
    jsonObjectAt(projection, "/data").worker_restart_state = workerRestartState();
    const body = JSON.stringify(projection);
    const extended = decode(EXTENDED_CONFIG_RESPONSE_SCHEMA, parseJsonObject(body, name));
    expect(extended.ok, name).toBe(true);
    const read = await readWithReplies([{ body }]);
    expect(read.result, name).toEqual(INVALID_RESPONSE);
    expect(
      read.requests.map((request) => [request.method, request.url]),
      name,
    ).toEqual([["GET", EXTENDED_URL]]);
  }
});

test("运行配置预校验契约样本", async () => {
  for (const [name, taxonomy, threshold] of [
    ["config-validate-default", false, false],
    ["config-validate-taxonomy", true, false],
    ["config-validate-collection-threshold", false, true],
    ["config-validate", true, true],
  ] as const) {
    const body = fixtureText(name);
    const projection = parseJsonObject(body, name);
    const decoded = expectDecodedProjection(projection, name, taxonomy, threshold);
    const result = await validateWithBody(body);
    if (taxonomy && threshold) {
      expect(result.ok, name).toBe(true);
      if (!result.ok) {
        throw new Error(`${name} 应通过完整预校验投影`);
      }
      expect(valueAt(result.value.document, "/allow_manual_early_open")).toBe(true);
    } else {
      expect(result, name).toEqual(INVALID_RESPONSE);
    }

    // 不完整的投影本身就会被编辑器拒绝，删键后的失败要单独在 DTO 层确认。
    const missingFingerprint = structuredClone(projection);
    expect(removeAt(missingFingerprint, "/data/fingerprint")).toBe(decoded.fingerprint);
    expect(decode(CONFIG_RESPONSE_SCHEMA, missingFingerprint).ok, name).toBe(false);
    expect(
      await validateWithBody(JSON.stringify(missingFingerprint)),
      `${name}: fingerprint`,
    ).toEqual(INVALID_RESPONSE);
  }

  const body = fixtureText("config-validate-invalid");
  expect(adminApiFailureFromHttpBody(400, body).code).toBe("invalid_runtime_config");
  const invalid = parseJsonObject(
    fixtureText("config-validate-invalid"),
    "config-validate-invalid.json",
  );
  expect(removeAt(invalid, "/code")).toBe("invalid_runtime_config");
  expect(adminApiFailureFromHttpBody(400, JSON.stringify(invalid)).code).toBe("http_400");
});
