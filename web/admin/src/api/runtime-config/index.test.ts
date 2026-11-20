// Admin 运行配置读取入口、投影回退与 Worker 重启状态的回归。

import { afterEach, expect, spyOn, test } from "bun:test";
import { readFileSync } from "node:fs";
import { AdminRuntimeConfigResponseWithWorkerRestartStateSchema } from "@auction/contracts/dto/admin/rules";
import { apiResponseSchema } from "@auction/contracts/dto/response";
import { decode } from "@auction/contracts/wire";
import { err } from "@auction/web-shared/result";

import {
  asAdminJsonValue,
  createAdminRequestHarness,
  type RecordedRequest,
  type Reply,
  TEST_SESSION_BODY,
} from "../api-request-fixture";
import {
  fetchAdminRuntimeConfig,
  prepareAdminRuntimeConfigUpdateOnce,
  updateAdminRuntimeConfig,
  validateAdminRuntimeConfig,
} from "./index";
import { createRuntimeConfigFixtures, removeAt } from "./runtime-config-fixture";

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
  withWorkerState,
  workerRestartState,
} = createRuntimeConfigFixtures(fixtureText);

const BASE_URL =
  "/api/v1/admin/config?include_equipment_taxonomy=true&include=fulfillment_collection_overdue_age_seconds";
const EXTENDED_URL = `${BASE_URL}&include_worker_restart_state=true`;
const VALIDATE_URL = `/api/v1/admin/config:validate?include_equipment_taxonomy=true&include=fulfillment_collection_overdue_age_seconds`;
const SESSION_URL = "/api/v1/admin/session?include=server_time";
const INVALID_QUERY =
  '{"code":"invalid_query","message":"不支持查询参数","data":null,"request_id":null}';
const COMMAND_ID = "00000000-0000-4000-8000-000000000002";
const WORKER_RESPONSE_SCHEMA = apiResponseSchema(
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

function requestLines(recorded: readonly RecordedRequest[]): (readonly [string, string])[] {
  return recorded.map((request) => [request.method, request.url] as const);
}

async function readWithReplies(replies: readonly Reply[]): Promise<{
  readonly result: Awaited<ReturnType<typeof fetchAdminRuntimeConfig>>;
  readonly requests: readonly RecordedRequest[];
}> {
  const registration = requests.registerHost();
  const recorded = requests.installFetch(replies);
  const result = await fetchAdminRuntimeConfig(registration.host, new AbortController().signal);
  return { result, requests: [...recorded] };
}

async function validateWithResponse(responseBody: string): Promise<{
  readonly result: Awaited<ReturnType<typeof validateAdminRuntimeConfig>>;
  readonly requests: readonly RecordedRequest[];
}> {
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
  return { result, requests: [...recorded] };
}

async function updateWithResponse(responseBody: string): Promise<{
  readonly result: Awaited<ReturnType<typeof updateAdminRuntimeConfig>>;
  readonly requests: readonly RecordedRequest[];
}> {
  requests.installBrowserRequest();
  const registration = requests.registerHost();
  const recorded = requests.installFetch([
    { body: JSON.stringify(TEST_SESSION_BODY) },
    { body: responseBody },
  ]);
  const prepare = prepareAdminRuntimeConfigUpdateOnce(
    defaultProjection().data.fingerprint,
    asAdminJsonValue(defaultDocument()),
  );
  const result = await updateAdminRuntimeConfig(
    registration.host,
    COMMAND_ID,
    prepare,
    new AbortController().signal,
  );
  return { result, requests: [...recorded] };
}

test("运行配置请求同时请求两个扩展投影", async () => {
  const read = await readWithReplies([
    { body: JSON.stringify(withWorkerState(workerRestartState())) },
  ]);
  expect(read.result.ok).toBe(true);
  expect(requestLines(read.requests)).toEqual([["GET", EXTENDED_URL]]);

  const validated = await validateWithResponse(JSON.stringify(defaultProjection()));
  expect(validated.result.ok).toBe(true);
  expect(requestLines(validated.requests)).toEqual([
    ["GET", SESSION_URL],
    ["POST", VALIDATE_URL],
  ]);

  // Rust 的保存回执要求 restart_required 为 true；其余字段沿用默认样本。
  const updateResponse = cloneProjection(defaultProjection());
  updateResponse.data.restart_required = true;
  const updated = await updateWithResponse(JSON.stringify(updateResponse));
  expect(updated.result.ok).toBe(true);
  expect(requestLines(updated.requests)).toEqual([
    ["GET", SESSION_URL],
    ["PUT", BASE_URL],
  ]);
});

test("只解码后端投影里的已知 Worker 重启状态", async () => {
  // 原始 Worker 样本只用于投影解码；编辑器读取还要求默认样本里的两个扩展投影。
  const raw = runtimeProjection("runtime-config-worker-restart-state");
  const decoded = decode(WORKER_RESPONSE_SCHEMA, raw);
  expect(decoded.ok).toBe(true);
  if (!decoded.ok) {
    throw new Error("原始 Worker 投影应通过扩展响应 DTO 解码");
  }
  expect(decoded.value.data.worker_restart_state).toBe("unknown");
  const initial = await readWithReplies([
    { body: JSON.stringify(withWorkerState(workerRestartState())) },
  ]);
  expect(initial.result.ok).toBe(true);
  if (!initial.result.ok) {
    throw new Error("默认投影应通过读取解码");
  }
  expect(initial.result.value.worker_restart_state).toBe("unknown");

  for (const wire of ["restart_required", "consistent"] as const) {
    const projection = cloneProjection(raw);
    projection.data.worker_restart_state = wire;
    const state = decode(WORKER_RESPONSE_SCHEMA, projection);
    expect(state.ok && state.value.data.worker_restart_state, wire).toBe(wire);

    const read = await readWithReplies([{ body: JSON.stringify(withWorkerState(wire)) }]);
    expect(read.result.ok && read.result.value.worker_restart_state, wire).toBe(wire);
  }

  const missingProjection = cloneProjection(raw);
  removeAt(missingProjection, "/data/worker_restart_state");
  expect(decode(WORKER_RESPONSE_SCHEMA, missingProjection).ok, "缺少 Worker 状态").toBe(false);

  const missingRead = await readWithReplies([{ body: JSON.stringify(defaultProjection()) }]);
  expect(missingRead.result).toEqual(INVALID_RESPONSE);
  expect(requestLines(missingRead.requests)).toEqual([["GET", EXTENDED_URL]]);

  for (const invalid of ["outdated", false, { unknown: null }, null]) {
    const label = JSON.stringify(invalid);
    const projection = cloneProjection(raw);
    projection.data.worker_restart_state = invalid;
    expect(decode(WORKER_RESPONSE_SCHEMA, projection).ok, label).toBe(false);

    const read = await readWithReplies([{ body: JSON.stringify(withWorkerState(invalid)) }]);
    expect(read.result, label).toEqual(INVALID_RESPONSE);
    expect(requestLines(read.requests), label).toEqual([["GET", EXTENDED_URL]]);
  }

  // include_worker=false 的对照：完整基础投影省略 Worker 字段时成功。
  const control = await readWithReplies([
    { status: 400, body: INVALID_QUERY },
    { body: JSON.stringify(defaultProjection()) },
  ]);
  expect(control.result.ok).toBe(true);
  expect(requestLines(control.requests)).toEqual([
    ["GET", EXTENDED_URL],
    ["GET", BASE_URL],
  ]);

  // 回退请求不允许出现 Worker 字段：null 与合法字符串都必须被拒绝。
  for (const state of [null, workerRestartState()]) {
    const label = JSON.stringify(state);
    const read = await readWithReplies([
      { status: 400, body: INVALID_QUERY },
      { body: JSON.stringify(withWorkerState(state)) },
    ]);
    expect(read.result, label).toEqual(INVALID_RESPONSE);
    expect(requestLines(read.requests), label).toEqual([
      ["GET", EXTENDED_URL],
      ["GET", BASE_URL],
    ]);
  }
});

test("编辑器读取 Worker 状态并保留既有投影", async () => {
  const value = await readWithReplies([
    { body: JSON.stringify(withWorkerState(workerRestartState())) },
  ]);
  expect(value.result.ok).toBe(true);
  if (!value.result.ok) {
    throw new Error("完整编辑器投影应通过读取解码");
  }
  expect(value.result.value.equipment_taxonomy).not.toBeNull();
  expect(value.result.value.worker_restart_state).toBe("unknown");
  expect(requestLines(value.requests)).toEqual([["GET", EXTENDED_URL]]);

  const missingState = await readWithReplies([{ body: JSON.stringify(defaultProjection()) }]);
  expect(missingState.result).toEqual(INVALID_RESPONSE);
  expect(requestLines(missingState.requests)).toEqual([["GET", EXTENDED_URL]]);

  const missingProjections = await readWithReplies([
    { body: JSON.stringify(runtimeProjection("runtime-config-worker-restart-state")) },
  ]);
  expect(missingProjections.result).toEqual(INVALID_RESPONSE);
  expect(requestLines(missingProjections.requests)).toEqual([["GET", EXTENDED_URL]]);
});

test("编辑器只在 HTTP 400 invalid_query 时回退一次", async () => {
  const fallback = await readWithReplies([
    { status: 400, body: INVALID_QUERY },
    { body: JSON.stringify(defaultProjection()) },
  ]);
  expect(fallback.result.ok).toBe(true);
  if (!fallback.result.ok) {
    throw new Error("回退后的基础投影应通过读取解码");
  }
  expect(fallback.result.value.equipment_taxonomy).not.toBeNull();
  expect(fallback.result.value.worker_restart_state).toBe("unknown");
  expect(requestLines(fallback.requests)).toEqual([
    ["GET", EXTENDED_URL],
    ["GET", BASE_URL],
  ]);

  const rejected = await readWithReplies([
    { status: 400, body: INVALID_QUERY },
    { status: 400, body: INVALID_QUERY },
  ]);
  expect(rejected.result).toEqual({
    ok: false,
    error: { status: 400, code: "invalid_query" },
  });
  expect(requestLines(rejected.requests)).toEqual([
    ["GET", EXTENDED_URL],
    ["GET", BASE_URL],
  ]);

  for (const [reply, expected] of [
    [
      { status: 400, body: "" },
      { status: 400, code: "http_400" },
    ],
    [
      { status: 503, body: INVALID_QUERY },
      { status: 503, code: "http_503" },
    ],
  ] as const) {
    const actual = await readWithReplies([reply]);
    expect(actual.result).toEqual({ ok: false, error: expected });
    expect(requestLines(actual.requests)).toEqual([["GET", EXTENDED_URL]]);
  }

  // 传输层返回的无状态 `invalid_query` 也要原样保留，且只请求一次扩展投影。
  const registration = requests.registerHost();
  const requestedUrls: string[] = [];
  const transport = await import("../transport");
  const spy = spyOn(transport, "sendGet").mockImplementation(async (_host, url) => {
    requestedUrls.push(url);
    return err("invalid_query");
  });
  try {
    const actual = await fetchAdminRuntimeConfig(registration.host, new AbortController().signal);
    expect(actual).toEqual({
      ok: false,
      error: { status: null, code: "invalid_query" },
    });
    expect(requestedUrls).toEqual([EXTENDED_URL]);
  } finally {
    spy.mockRestore();
  }
});
