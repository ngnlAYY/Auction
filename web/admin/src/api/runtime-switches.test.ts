// 运行时开关的原因边界与读取、暂停命令契约样本回归。

import { afterEach, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import {
  AdminExternalAccessPauseResponseSchema,
  RuntimeSwitchListResponseSchema,
} from "@auction/contracts/dto/admin/runtime-switches";
import { apiResponseSchema } from "@auction/contracts/dto/response";
import { decode } from "@auction/contracts/wire";
import { utf8ByteLength } from "@auction/web-shared/text";

import {
  createAdminRequestHarness,
  jsonObject,
  parseJsonObject,
  type RecordedRequest,
  removeAt,
  setAt,
  TEST_SESSION_BODY,
  valueAt,
} from "./api-request-fixture";
import {
  type ExternalAccessPauseAttempt,
  fetchRuntimeSwitches,
  type PreparedExternalAccessPause,
  prepareExternalAccessPauseOnce,
  submitExternalAccessPause,
  validExternalAccessPauseReason,
} from "./runtime-switches";

const READ_URL = "/api/v1/admin/runtime-switches";
const COMMAND_URL = "/api/v1/admin/runtime-switches/external-access-paused";
const SESSION_URL = "/api/v1/admin/session?include=server_time";
const COMMAND_ID = "00000000-0000-4000-8000-000000000002";
const INVALID_RESPONSE = { ok: false, error: "invalid_response" } as const;

const FIXTURES = new URL(
  "../../../../crates/auction-contracts/tests/fixtures/admin/",
  import.meta.url,
);

function fixtureText(fixtureName: string): string {
  return readFileSync(new URL(`${fixtureName}.json`, FIXTURES), "utf8");
}

const requests = createAdminRequestHarness();
afterEach(() => {
  requests.cleanup();
});

function requestLines(recorded: readonly RecordedRequest[]): (readonly [string, string])[] {
  return recorded.map((request) => [request.method, request.url] as const);
}

async function readSwitches(body: string): Promise<{
  readonly result: Awaited<ReturnType<typeof fetchRuntimeSwitches>>;
  readonly requests: readonly RecordedRequest[];
}> {
  const registration = requests.registerHost();
  const recorded = requests.installFetch([{ body }]);
  const result = await fetchRuntimeSwitches(registration.host, new AbortController().signal);
  return { result, requests: [...recorded] };
}

async function submitWithResponse(
  prepared: PreparedExternalAccessPause,
  responseBody: string,
): Promise<{
  readonly result: Awaited<ReturnType<typeof submitExternalAccessPause>>;
  readonly requests: readonly RecordedRequest[];
}> {
  requests.installBrowserRequest();
  const registration = requests.registerHost();
  const recorded = requests.installFetch([
    { body: JSON.stringify(TEST_SESSION_BODY) },
    { body: responseBody },
  ]);
  const result = await submitExternalAccessPause(
    registration.host,
    prepared,
    new AbortController().signal,
  );
  return { result, requests: [...recorded] };
}

function prepare(attempt: ExternalAccessPauseAttempt): PreparedExternalAccessPause {
  const prepared = prepareExternalAccessPauseOnce(attempt)(COMMAND_ID);
  if (!prepared.ok) {
    throw new Error("暂停命令准备失败");
  }
  return prepared.value;
}

function databaseIntents(body: Record<string, unknown>): unknown[] {
  const entries = valueAt(body, "/data/database_intent");
  if (!Array.isArray(entries)) {
    throw new Error("样本缺少 database_intent 数组");
  }
  for (const entry of entries) {
    jsonObject(entry, "database_intent 条目");
  }
  return entries;
}

test("暂停原因使用服务端 UTF-8 字节上限", () => {
  const atLimit = `${"停".repeat(341)}x`;
  expect(utf8ByteLength(atLimit)).toBe(1024);
  expect(validExternalAccessPauseReason(atLimit)).toBe(true);
  expect(validExternalAccessPauseReason("x".repeat(1024))).toBe(true);
  expect(validExternalAccessPauseReason("停".repeat(342))).toBe(false);
  expect(validExternalAccessPauseReason("x".repeat(1025))).toBe(false);
});

test("暂停原因必须是非空、已归一化的单行文本", () => {
  expect(validExternalAccessPauseReason("暂停检查")).toBe(true);
  for (const value of ["", " ", " 前后有空白 ", "两\n行", "有\t控制字符"]) {
    expect(validExternalAccessPauseReason(value), JSON.stringify(value)).toBe(false);
  }
});

test("运行时开关读取与命令契约样本", async () => {
  const body = fixtureText("runtime-switches");
  const read = await readSwitches(body);
  expect(read.result.ok).toBe(true);
  if (!read.result.ok) {
    throw new Error("运行时开关样本应通过读取校验");
  }
  expect(requestLines(read.requests)).toEqual([["GET", READ_URL]]);
  const pause = read.result.value.database_intent.find(
    (item) => item.name === "external_access_paused",
  );
  expect(pause?.operation).toBe("set");
  expect(read.result.value.admin_last_success_age_seconds).toBeNull();

  const listSchema = apiResponseSchema(RuntimeSwitchListResponseSchema);
  const missingIntent = parseJsonObject(body, "runtime-switches.json");
  expect(decode(listSchema, missingIntent).ok).toBe(true);
  // 删键应当先被 DTO 拒绝，公开读取入口仍须实际发出 GET。
  expect(removeAt(missingIntent, "/data/database_intent")).toBeDefined();
  expect(decode(listSchema, missingIntent).ok).toBe(false);
  const missingIntentRead = await readSwitches(JSON.stringify(missingIntent));
  expect(missingIntentRead.result).toEqual(INVALID_RESPONSE);
  expect(requestLines(missingIntentRead.requests)).toEqual([["GET", READ_URL]]);

  const duplicated = parseJsonObject(fixtureText("runtime-switches"), "runtime-switches.json");
  const entries = databaseIntents(duplicated);
  expect(entries.length).toBeGreaterThan(0);
  const originalLength = entries.length;
  const first = entries[0];
  if (first === undefined) {
    throw new Error("样本缺少首条数据库意图");
  }
  entries.push(structuredClone(first));
  const duplicatedRead = await readSwitches(JSON.stringify(duplicated));
  expect(duplicatedRead.result).toEqual(INVALID_RESPONSE);
  expect(requestLines(duplicatedRead.requests)).toEqual([["GET", READ_URL]]);

  const missingPause = parseJsonObject(fixtureText("runtime-switches"), "runtime-switches.json");
  const kept = databaseIntents(missingPause).filter(
    (entry) => jsonObject(entry, "database_intent 条目").name !== "external_access_paused",
  );
  expect(kept.length).toBe(originalLength - 1);
  setAt(missingPause, "/data/database_intent", kept);
  const missingPauseRead = await readSwitches(JSON.stringify(missingPause));
  expect(missingPauseRead.result).toEqual(INVALID_RESPONSE);
  expect(requestLines(missingPauseRead.requests)).toEqual([["GET", READ_URL]]);

  const prepared = prepare({
    commandId: COMMAND_ID,
    expectedRevision: 1,
    paused: false,
    expiresAt: null,
    reason: "resume regular polling",
  });
  const commandBody = fixtureText("runtime-switch-command");
  const accepted = await submitWithResponse(prepared, commandBody);
  expect(accepted.result.ok).toBe(true);
  if (!accepted.result.ok) {
    throw new Error("暂停命令样本应被接受");
  }
  expect(accepted.result.value.name).toBe("external_access_paused");
  expect(accepted.result.value.operation).toBe("clear");
  expect(accepted.result.value.revision).toBe(2);
  expect(requestLines(accepted.requests)).toEqual([
    ["GET", SESSION_URL],
    ["POST", COMMAND_URL],
  ]);

  const commandSchema = apiResponseSchema(AdminExternalAccessPauseResponseSchema);
  const missingRevision = parseJsonObject(commandBody, "runtime-switch-command.json");
  expect(decode(commandSchema, missingRevision).ok).toBe(true);
  expect(removeAt(missingRevision, "/data/accepted/revision")).toBe(2);
  expect(decode(commandSchema, missingRevision).ok).toBe(false);
  const missingReceipt = await submitWithResponse(prepared, JSON.stringify(missingRevision));
  expect(missingReceipt.result).toEqual({
    ok: false,
    error: { status: null, code: "invalid_response" },
  });
  expect(requestLines(missingReceipt.requests)).toEqual([
    ["GET", SESSION_URL],
    ["POST", COMMAND_URL],
  ]);

  const paused = prepare({
    commandId: COMMAND_ID,
    expectedRevision: 1,
    paused: true,
    expiresAt: "2026-11-20T00:00:00Z",
    reason: "manual account use",
  });
  const wrongDirection = await submitWithResponse(paused, commandBody);
  expect(wrongDirection.result).toEqual({
    ok: false,
    error: { status: null, code: "invalid_response" },
  });
  expect(requestLines(wrongDirection.requests)).toEqual([
    ["GET", SESSION_URL],
    ["POST", COMMAND_URL],
  ]);

  const invalid = parseJsonObject(
    fixtureText("runtime-switch-command"),
    "runtime-switch-command.json",
  );
  setAt(invalid, "/data/accepted/name", "metrics_sli_snapshot_enabled");
  const wrongName = await submitWithResponse(prepared, JSON.stringify(invalid));
  expect(wrongName.result).toEqual({
    ok: false,
    error: { status: null, code: "invalid_response" },
  });
  expect(requestLines(wrongName.requests)).toEqual([
    ["GET", SESSION_URL],
    ["POST", COMMAND_URL],
  ]);
});
