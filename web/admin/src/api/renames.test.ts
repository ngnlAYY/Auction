// 改名列表的私有 schema 及回滚准备，通过公开读取入口覆盖。

import { expect, spyOn, test } from "bun:test";
import { err } from "@auction/web-shared/result";
import { createAdminRequestHarness } from "./api-request-fixture";
import { adminApiFailureDefiniteRejection, adminApiFailureDisplayCode } from "./failure";
import { fetchAdminRenames, prepareAdminRenameRevert } from "./renames";
import { adminFixtureText, mustSucceed, withoutFixtureField } from "./t10-contract-fixture.test";

const RENAME = {
  id: "00000000-0000-4000-8000-000000000001",
  user_id: "00000000-0000-4000-8000-000000000002",
  previous_username: "before",
  next_username: "after",
  actor: "operator",
  created_at: "2026-11-14T00:00:00Z",
  reverted_at: null,
  user_state_version: 3,
};
const COMMAND_ID = "00000000-0000-4000-8000-000000000003";
const PREPARE_PATH = `/api/v1/admin/renames?page=1&page_size=10&user_id=${RENAME.user_id}`;

function list(renames: readonly unknown[], total = 1): string {
  return JSON.stringify({
    code: "ok",
    message: "",
    data: { pagination: { page: 1, page_size: 10, total }, renames },
  });
}

// 观察实际读取用到的 schema，区分反序列化失败与后续跨字段校验失败。
async function readList(body: string, page: number, size: number, userId = "all") {
  const failure = await import("./failure");
  const harness = createAdminRequestHarness();
  let restoreDecode: (() => void) | undefined;
  try {
    const decodeReceipt = spyOn(failure, "decodeAdminReceipt");
    restoreDecode = () => decodeReceipt.mockRestore();
    harness.installBrowserRequest();
    const requests = harness.installFetch([{ body }]);
    const { host } = harness.registerHost();
    const result = await fetchAdminRenames(
      host,
      userId,
      "",
      page,
      size,
      new AbortController().signal,
    );
    expect(requests).toEqual([
      {
        method: "GET",
        url:
          userId === "all" ? `/api/v1/admin/renames?page=${page}&page_size=${size}` : PREPARE_PATH,
        body: "",
      },
    ]);
    expect(decodeReceipt).toHaveBeenCalledTimes(1);
    const receipt = decodeReceipt.mock.results[0];
    if (receipt?.type !== "return") throw new Error("改名列表没有执行回执解码");
    return { result, decoded: receipt.value };
  } finally {
    try {
      restoreDecode?.();
    } finally {
      harness.cleanup();
    }
  }
}

test("explicit_null_reverted_at_decodes_as_none", async () => {
  const { result, decoded } = await readList(list([RENAME]), 1, 10);
  expect(decoded.ok).toBe(true);
  expect(mustSucceed(result).renames[0]?.reverted_at).toBeNull();
});

test("missing_reverted_at_fails_closed", async () => {
  const { reverted_at: _revertedAt, ...missing } = RENAME;
  const { result, decoded } = await readList(list([missing]), 1, 10);
  expect(decoded).toEqual(err("invalid_response"));
  expect(result).toEqual(err("invalid_response"));
});

async function prepareWithBody(body: string) {
  const harness = createAdminRequestHarness();
  try {
    harness.installBrowserRequest();
    const requests = harness.installFetch([{ body }]);
    const { host } = harness.registerHost();
    const result = await prepareAdminRenameRevert(
      host,
      COMMAND_ID,
      RENAME,
      new AbortController().signal,
    );
    expect(requests).toEqual([{ method: "GET", url: PREPARE_PATH, body: "" }]);
    return result;
  } finally {
    harness.cleanup();
  }
}

function expectUnsent(result: Awaited<ReturnType<typeof prepareAdminRenameRevert>>, code: string) {
  if (result.ok) throw new Error("回滚准备应拒绝");
  expect(result.error).toBe(`not_sent:${code}`);
  expect(adminApiFailureDefiniteRejection(result.error)).toBe(true);
  expect(adminApiFailureDisplayCode(result.error)).toBe(code);
}

test("rename_preparation_requires_a_consistent_version_and_marks_failures_unsent", async () => {
  const prepared = mustSucceed(await prepareWithBody(list([RENAME])));
  expect(prepared.request.expected_state_version).toBe(3);
  expect(prepared.request.command_id).toBe(COMMAND_ID);

  const transport = await import("./transport");
  const harness = createAdminRequestHarness();
  let restoreRead: (() => void) | undefined;
  try {
    const read = spyOn(transport, "sendGet");
    restoreRead = () => read.mockRestore();
    const { host } = harness.registerHost();
    for (const code of ["network_error", "request_timeout", "invalid_response", "http_503"]) {
      read.mockClear();
      read.mockResolvedValueOnce(err(code));
      const signal = new AbortController().signal;
      expectUnsent(await prepareAdminRenameRevert(host, COMMAND_ID, RENAME, signal), code);
      expect(read.mock.calls).toEqual([[host, PREPARE_PATH, signal]]);
      expect(adminApiFailureDefiniteRejection(code)).toBe(false);
    }
  } finally {
    try {
      restoreRead?.();
    } finally {
      harness.cleanup();
    }
  }

  const { user_state_version: _version, ...missingVersion } = RENAME;
  // 线上显式零会在读取层被拒；省略版本键才能取得 Rust 准备助手收到的零版本投影。
  for (const [body, versions] of [
    [list([missingVersion]), [0]],
    [list([RENAME, { ...RENAME, user_state_version: 2 }], 2), [3, 2]],
    [list([], 0), []],
  ] as const) {
    const { result, decoded } = await readList(body, 1, 10, RENAME.user_id);
    expect(decoded.ok).toBe(true);
    expect(mustSucceed(result).renames.map((row) => row.user_state_version)).toEqual([...versions]);
    expectUnsent(await prepareWithBody(body), "invalid_response");
  }
});

test("rename_list_contract_fixtures", async () => {
  for (const [name, page, size, count] of [
    ["renames", 1, 50, 1],
    ["renames-empty-page", 2, 1, 0],
  ] as const) {
    const text = adminFixtureText(name);
    const { result, decoded } = await readList(text, page, size);
    expect(decoded.ok).toBe(true);
    const rejected = await readList(
      withoutFixtureField(text, "/data/pagination/total"),
      page,
      size,
    );
    expect(rejected.decoded).toEqual(err("invalid_response"));
    expect(rejected.result).toEqual(err("invalid_response"));
    const { renames } = mustSucceed(result);
    expect(renames).toHaveLength(count);
    if (count === 1) {
      expect(renames[0]?.user_state_version).toBe(3);
      expect(renames[0]?.reverted_at).toBeNull();
    }
  }
});
