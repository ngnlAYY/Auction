// 凭据变更与删除的 HTTP 正文和回执核对。

import { expect, spyOn, test } from "bun:test";
import { installFakeWindow } from "@auction/web-shared/router/test-window";
import {
  createAdminRequestHarness,
  parseJsonObject,
  TEST_SESSION_BODY,
} from "../api-request-fixture";
import type { AdminSessionHost } from "../session";
import {
  type AdminCredential,
  createAdminCredential,
  deleteAdminCredential,
  mutateAdminCredential,
  prepareAdminCredentialCreate,
  prepareAdminCredentialDelete,
  prepareAdminCredentialMutation,
} from ".";

const PATH = "/api/v1/admin/credentials";
const COMMAND_ID = "00000000-0000-4000-8000-000000000002";
const TARGET = "11111111-1111-4111-8111-111111111111";
const OTHER = "22222222-2222-4222-8222-222222222222";
const PREVIOUS: AdminCredential = {
  id: TARGET,
  kind: "mail_cookie",
  world: "shared",
  label: "Worker",
  player_name: null,
  status: "active",
  state_version: 7,
  created_at: "2026-11-18T00:00:00Z",
  rotated_at: null,
  updated_at: "2026-11-18T00:00:00Z",
};
const UNKNOWN = {
  ok: false,
  error: { kind: "outcome-unknown", code: "invalid_response", responseStatus: null },
} as const;

async function withReply<T>(
  method: "PATCH" | "DELETE",
  data: object,
  send: (host: AdminSessionHost) => Promise<T>,
  body: object,
): Promise<T> {
  const harness = createAdminRequestHarness();
  try {
    harness.installBrowserRequest();
    const { host } = harness.registerHost();
    const requests = harness.installFetch([
      { body: JSON.stringify(TEST_SESSION_BODY) },
      { body: JSON.stringify({ code: "ok", message: "", data }) },
    ]);
    const result = await send(host);
    expect(requests.map(({ method, url }) => [method, url])).toEqual([
      ["GET", "/api/v1/admin/session?include=server_time"],
      [method, PATH],
    ]);
    expect(parseJsonObject(requests[1]?.body ?? "", "凭据命令正文")).toEqual({
      command_id: COMMAND_ID,
      credential_id: TARGET,
      expected_state_version: 7,
      ...body,
    });
    return result;
  } finally {
    harness.cleanup();
  }
}

test("credential_mutation_requires_matching_version_target_and_action_projection", async () => {
  for (const [action, label, status, previous, updated, mismatch] of [
    ["rename", "New label", null, PREVIOUS, { label: "New label" }, { label: "Different" }],
    ["set_status", null, "disabled", PREVIOUS, { status: "disabled" }, { status: "active" }],
    [
      "set_status",
      null,
      "active",
      { ...PREVIOUS, status: "disabled" },
      { status: "active" },
      { status: "disabled" },
    ],
  ] as const) {
    const valid = { ...previous, ...updated, state_version: 8 };
    const prepared = prepareAdminCredentialMutation(COMMAND_ID, previous, action, label, status);
    for (const [patch, matches] of [
      [{}, true],
      [{ id: OTHER }, false],
      [{ state_version: 7 }, false],
      [{ world: "main" }, false],
      [mismatch, false],
    ] as const) {
      const data = { credential: { ...valid, ...patch }, restart_required: true };
      const result = await withReply(
        "PATCH",
        data,
        (host) => mutateAdminCredential(host, prepared, new AbortController().signal),
        { action, label, status },
      );
      expect(result).toEqual(matches ? { ok: true, value: data } : UNKNOWN);
    }
  }
});

test("credential_status_receipt_must_preserve_the_original_label", async () => {
  const prepared = prepareAdminCredentialMutation(
    COMMAND_ID,
    PREVIOUS,
    "set_status",
    null,
    "disabled",
  );
  const data = {
    credential: { ...PREVIOUS, status: "disabled", state_version: 8, label: "Different" },
    restart_required: true,
  };
  expect(
    await withReply(
      "PATCH",
      data,
      (host) => mutateAdminCredential(host, prepared, new AbortController().signal),
      { action: "set_status", label: null, status: "disabled" },
    ),
  ).toEqual(UNKNOWN);
});

test("credential_deletion_requires_matching_target_and_frozen_version", async () => {
  const prepared = prepareAdminCredentialDelete(COMMAND_ID, PREVIOUS);
  for (const [credential_id, state_version, matches] of [
    [TARGET, 8, true],
    [TARGET, 7, false],
    [OTHER, 8, false],
  ] as const) {
    const data = { credential_id, state_version, restart_required: true };
    const result = await withReply(
      "DELETE",
      data,
      (host) => deleteAdminCredential(host, prepared, new AbortController().signal),
      {},
    );
    expect(result).toEqual(matches ? { ok: true, value: data } : UNKNOWN);
  }
});

test("credential_import_reports_malformed_receipts_without_payload_or_persistence", async () => {
  const secret = "fake-test-cookie=telemetry-placeholder";
  const request = createAdminRequestHarness();
  const { fake, restore } = installFakeWindow("/cookies");
  const storageWrites: unknown[] = [];
  const diagnostics: unknown[] = [];
  const log = spyOn(console, "error").mockImplementation((...args) => diagnostics.push(...args));
  const storage = {
    getItem: () => null,
    setItem(key: string, value: string) {
      storageWrites.push(key, value);
    },
  };
  Object.defineProperties(fake, {
    localStorage: { value: storage },
    sessionStorage: { value: storage },
  });
  try {
    request.installBrowserRequest();
    const { host } = request.registerHost();
    const requests = request.installFetch([
      { body: JSON.stringify(TEST_SESSION_BODY) },
      // 成功状态下携带错误形状，且正文包含合成秘密，验证报告路径也不回显它。
      { status: 201, body: JSON.stringify({ code: "ok", message: secret, data: null }) },
      { body: "{}" },
    ]);
    const prepared = prepareAdminCredentialCreate({
      command_id: COMMAND_ID,
      kind: "mail_cookie",
      world: "main",
      label: "Worker",
      secret,
    });
    expect(await createAdminCredential(host, prepared, new AbortController().signal)).toEqual(
      UNKNOWN,
    );
    const beacon = requests[2];
    if (beacon === undefined) throw new Error("缺少凭据回执解码错误信标");
    expect(beacon.method).toBe("POST");
    expect(beacon.url.includes("class=dto_decode")).toBe(true);
    expect(beacon.url.includes(secret)).toBe(false);
    expect(beacon.body === "").toBe(true);
    expect(requests).toHaveLength(3);
    expect(storageWrites).toHaveLength(0);
    expect(diagnostics).toHaveLength(0);
  } finally {
    log.mockRestore();
    request.cleanup();
    restore();
  }
});
