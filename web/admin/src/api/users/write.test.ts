// 用户写入的冻结命令、原始回执以及未决宿主的端点裁定。

import { expect, test } from "bun:test";
import type { AdminUserResponse } from "@auction/contracts/dto/admin/users";
import type { Result } from "@auction/web-shared/result";
import {
  createPendingHarness,
  waitForAttempt,
} from "../../surface/pending/pending-command-fixture";
import {
  createAdminRequestHarness,
  installWriteFetch,
  parseJsonObject,
  TEST_SESSION_BODY,
} from "../api-request-fixture";
import type { AdminWriteFailure } from "../failure";
import { adminWriteFailureDisplayCode, classifyAdminWriteFailure } from "../failure";
import type { AdminSessionHost } from "../session";
import {
  type AdminUser,
  type AdminUserMutationResult,
  blacklistAdminUser,
  createAdminUser,
  mutateAdminUser,
  prepareAdminUserBlacklistOnce,
  prepareAdminUserCreateOnce,
  prepareAdminUserMutationOnce,
  prepareAdminUserUnblacklistOnce,
  unblacklistAdminUser,
} from ".";

const USER_ID = "00000000-0000-4000-8000-000000000002";
const OTHER_ID = "00000000-0000-4000-8000-000000000004";
const COMMAND_ID = "00000000-0000-4000-8000-000000000003";
const USERS = "/api/v1/admin/users";
const BLACKLIST = `${USERS}/${USER_ID}/blacklist`;
const KEY = "A".repeat(43);
const USER: AdminUserResponse = {
  id: USER_ID,
  username: "before",
  forum_account_id: null,
  status: "active",
  key_status: "active",
  state_version: 3,
  created_at: "2026-11-14T00:00:00Z",
  updated_at: "2026-11-14T00:00:00Z",
};
const target: AdminUser = { ...USER, blacklist_count: 0, blacklist_history: [] };
type Attempt = (
  host: AdminSessionHost,
  id: string,
  signal: AbortSignal,
) => Promise<Result<AdminUser | AdminUserMutationResult, AdminWriteFailure>>;

function create(mode: "manual" | "generated"): Attempt {
  const prepare = prepareAdminUserCreateOnce("before", "42", mode, mode === "manual" ? KEY : null);
  return (host, id, signal) => createAdminUser(host, id, prepare, signal);
}

function patch(action: string, username: string | null, forumAccountId: string | null): Attempt {
  const prepare = prepareAdminUserMutationOnce(target, action, username, forumAccountId);
  return (host, id, signal) => mutateAdminUser(host, id, prepare, signal);
}

function blacklist(): Attempt {
  const prepare = prepareAdminUserBlacklistOnce(target, "reason");
  return (host, id, signal) => blacklistAdminUser(host, id, prepare, signal);
}

function unblacklist(): Attempt {
  const prepare = prepareAdminUserUnblacklistOnce({ ...target, status: "blacklisted" });
  return (host, id, signal) => unblacklistAdminUser(host, id, prepare, signal);
}
const cases = [
  {
    name: "创建生成 Key",
    method: "POST",
    path: USERS,
    attempt: () => create("generated"),
    request: { username: "before", forum_account_id: "42", key_mode: "generated", key: null },
    user: { ...USER, forum_account_id: "42", state_version: 1 },
    key: KEY,
    mismatches: [
      { username: "other" },
      { forum_account_id: "43" },
      { status: "blacklisted" },
      { key_status: "revoked" },
      { state_version: 2 },
    ],
  },
  {
    name: "创建手工 Key",
    method: "POST",
    path: USERS,
    attempt: () => create("manual"),
    request: { username: "before", forum_account_id: "42", key_mode: "manual", key: KEY },
    user: { ...USER, forum_account_id: "42", state_version: 1 },
    key: null,
    mismatches: [{ username: "other" }, { forum_account_id: null }],
  },
  {
    name: "改用户名",
    method: "PATCH",
    path: USERS,
    attempt: () => patch("rename", "after", null),
    request: {
      user_id: USER_ID,
      expected_state_version: 3,
      action: "rename",
      username: "after",
      forum_account_id: null,
    },
    user: { ...USER, username: "after", state_version: 4 },
    key: null,
    mismatches: [{ username: "other" }, { id: OTHER_ID }, { state_version: 5 }],
  },
  {
    name: "绑定论坛账号",
    method: "PATCH",
    path: USERS,
    attempt: () => patch("set_forum_account_id", null, "42"),
    request: {
      user_id: USER_ID,
      expected_state_version: 3,
      action: "set_forum_account_id",
      username: null,
      forum_account_id: "42",
    },
    user: { ...USER, forum_account_id: "42", state_version: 4 },
    key: null,
    mismatches: [{ forum_account_id: "43" }, { id: OTHER_ID }, { state_version: 3 }],
  },
  {
    name: "吊销 Key",
    method: "PATCH",
    path: USERS,
    attempt: () => patch("revoke_key", null, null),
    request: {
      user_id: USER_ID,
      expected_state_version: 3,
      action: "revoke_key",
      username: null,
      forum_account_id: null,
    },
    user: { ...USER, key_status: "revoked", state_version: 4 },
    key: null,
    mismatches: [{ key_status: "active" }, { id: OTHER_ID }, { state_version: 5 }],
  },
  {
    name: "重签 Key",
    method: "PATCH",
    path: USERS,
    attempt: () => patch("reissue_key", null, null),
    request: {
      user_id: USER_ID,
      expected_state_version: 3,
      action: "reissue_key",
      username: null,
      forum_account_id: null,
    },
    user: { ...USER, state_version: 4 },
    key: KEY,
    mismatches: [{ key_status: "revoked" }, { id: OTHER_ID }, { state_version: 3 }],
  },
  {
    name: "拉黑",
    method: "PUT",
    path: BLACKLIST,
    attempt: blacklist,
    request: { expected_state_version: 3, public_reason: "reason" },
    user: { ...USER, status: "blacklisted", state_version: 4 },
    key: null,
    mismatches: [{ status: "active" }, { id: OTHER_ID }, { state_version: 5 }],
  },
  {
    name: "解除拉黑",
    method: "DELETE",
    path: BLACKLIST,
    attempt: unblacklist,
    request: { expected_state_version: 3 },
    user: { ...USER, state_version: 4 },
    key: null,
    mismatches: [{ status: "blacklisted" }, { id: OTHER_ID }, { state_version: 3 }],
  },
];

function receipt(method: string, user: typeof USER, key: string | null) {
  return JSON.stringify({
    code: method === "POST" ? "created" : "ok",
    message: "",
    data: key === null ? { user } : { user, generated_key: key },
  });
}

for (const row of cases) {
  test(`${row.name}核对回执并使用原命令重试`, async () => {
    const harness = createAdminRequestHarness();
    try {
      harness.installBrowserRequest();
      const session = { body: JSON.stringify(TEST_SESSION_BODY) };
      const bad = [
        ...row.mismatches.map((fields) => receipt(row.method, { ...row.user, ...fields }, row.key)),
        receipt(row.method, row.user, row.key === null ? KEY : null),
      ];
      if (row.key !== null) {
        const good = receipt(row.method, row.user, row.key);
        bad.push(
          good.replace(`"id":"${USER_ID}"`, `"id":"${OTHER_ID}","id":"${USER_ID}"`),
          good.replace(
            `"state_version":${row.user.state_version}`,
            `"state_version":7,"state_version":${row.user.state_version}`,
          ),
          good.replace(
            `"generated_key":"${KEY}"`,
            `"generated_key":"${"B".repeat(43)}","generated_key":"${KEY}"`,
          ),
        );
      }
      const bodies = [...bad, receipt(row.method, row.user, row.key)];
      const requests = harness.installFetch(
        bodies.flatMap((body) => [session, { status: row.method === "POST" ? 201 : 200, body }]),
      );
      const { host } = harness.registerHost();
      const attempt = row.attempt();
      const signal = new AbortController().signal;
      for (const body of bad) {
        const result = await attempt(host, COMMAND_ID, signal);
        if (result.ok) throw new Error(`${row.name}交付了错配回执`);
        expect(classifyAdminWriteFailure(result.error), body).toBe("unknown");
        expect(adminWriteFailureDisplayCode(result.error)).toBe("invalid_response");
      }
      const result = await attempt(host, COMMAND_ID, signal);
      expect(result).toEqual({
        ok: true,
        value:
          row.path === USERS
            ? {
                user: { ...row.user, blacklist_count: 0, blacklist_history: [] },
                generated_key: row.key,
              }
            : { ...row.user, blacklist_count: 0, blacklist_history: [] },
      });
      const writes = requests.filter(({ method }) => method !== "GET");
      expect(requests).toHaveLength(bodies.length * 2);
      const first = writes[0];
      if (first === undefined) throw new Error("缺少用户写入");
      expect(first.method).toBe(row.method);
      expect(new URL(first.url, "https://auction.test").pathname).toBe(row.path);
      expect(parseJsonObject(first.body, "用户写命令")).toEqual({
        command_id: COMMAND_ID,
        ...row.request,
      });
      expect(writes).toEqual(bodies.map(() => first));
    } finally {
      harness.cleanup();
    }
  });
}

test.each([
  [`"id":"${USER_ID}"`, `"id":"${OTHER_ID}",`],
  ['"state_version":4', '"state_version":7,'],
] as const)("无 Key 回执重复 %s 时结果未知并保留未决项", async (field, extra) => {
  const good = receipt("PATCH", { ...USER, username: "after", state_version: 4 }, null);
  const duplicate = good.replace(field, `${extra}${field}`);
  expect(duplicate).not.toBe(good);
  const requestHarness = createAdminRequestHarness();
  const pending = createPendingHarness();
  try {
    requestHarness.installBrowserRequest();
    const network = installWriteFetch(
      (_, _init, index) => new Response(index === 0 ? duplicate : good),
    );
    try {
      const failures: AdminWriteFailure[] = [];
      const delivered: string[] = [];
      const attempt = patch("rename", "after", null);
      const {
        host,
        origin: { commands },
      } = pending;
      const entity = `user:${USER_ID}`;
      expect(
        commands.submit(
          entity,
          "改用户名",
          async (id, signal) => {
            const result = await attempt(host.sessionHost, id, signal);
            if (!result.ok) failures.push(result.error);
            return result;
          },
          (result) => {
            if ("user" in result) delivered.push(result.user.username);
          },
          () => {},
        ),
      ).toBe(true);
      await waitForAttempt(() => commands.anyBusy());
      expect(network.writes).toHaveLength(1);
      expect(failures).toEqual([
        { kind: "outcome-unknown", code: "invalid_response", responseStatus: null },
      ]);
      expect(delivered).toEqual([]);
      const entries = host.entries.peek();
      expect(entries).toHaveLength(1);
      const [entry] = entries;
      if (entry === undefined) throw new Error("缺少未决用户命令");
      expect(entry.unknown).toBe(true);
      expect(entry.error).toBe("invalid_response");

      host.retry(entry.identity.commandId);
      await waitForAttempt(() => commands.anyBusy());
      expect(network.writes).toHaveLength(2);
      expect(delivered).toEqual(["after"]);
      expect(commands.contains(entity)).toBe(false);
      const [first, retry] = network.writes;
      expect(retry).toEqual(first);
    } finally {
      network.restore();
    }
  } finally {
    pending.dispose();
    requestHarness.cleanup();
  }
});

test("用户 PATCH 和黑名单 PUT DELETE 只有真实 409 才能以 state_conflict 结清旧记录", async () => {
  for (const [method, path, makeAttempt] of [
    ["PATCH", USERS, () => patch("rename", "after", null)],
    ["PUT", BLACKLIST, blacklist],
    ["DELETE", BLACKLIST, unblacklist],
    ["POST", USERS, () => create("generated")],
  ] as const) {
    for (const status of [409, 422]) {
      const requestHarness = createAdminRequestHarness();
      const pending = createPendingHarness();
      try {
        requestHarness.installBrowserRequest();
        const network = installWriteFetch((_, _init, index) =>
          index === 0
            ? Response.json(
                { code: "storage_unavailable", message: "暂不可用", data: null, request_id: null },
                { status: 503 },
              )
            : Response.json(
                {
                  code: "state_conflict",
                  message: "用户版本已变化",
                  data: { current_state_version: 4 },
                  request_id: null,
                },
                { status },
              ),
        );
        try {
          const rejected: string[] = [];
          const attempt = makeAttempt();
          const { commands } = pending.origin;
          expect(
            commands.submit(
              "user:fixture",
              "用户写入",
              (id, signal) => attempt(pending.host.sessionHost, id, signal),
              () => {
                throw new Error("拒绝回执不能交付成功");
              },
              (code) => {
                rejected.push(code);
              },
            ),
          ).toBe(true);
          await waitForAttempt(() => commands.anyBusy());
          expect(pending.host.entries.peek()[0]?.unknown).toBe(true);
          const id = pending.host.entries.peek()[0]?.identity.commandId;
          if (id === undefined) throw new Error("缺少未决用户命令");
          pending.host.retry(id);
          await waitForAttempt(() => commands.anyBusy());
          expect(network.writes.map(({ method, path }) => ({ method, path }))).toEqual([
            { method, path },
            { method, path },
          ]);
          expect(network.writes[1]?.body).toBe(network.writes[0]?.body);
          const clears = method !== "POST" && status === 409;
          expect(commands.contains("user:fixture")).toBe(!clears);
          if (clears) {
            expect(rejected).toEqual(["storage_unavailable", "state_conflict"]);
          } else {
            const display = method === "POST" || status === 409 ? "state_conflict" : "http_422";
            expect(rejected).toEqual(["storage_unavailable", display]);
            expect(pending.host.entries.peek()[0]?.error).toBe(display);
          }
        } finally {
          network.restore();
        }
      } finally {
        pending.dispose();
        requestHarness.cleanup();
      }
    }
  }
});
