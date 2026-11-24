// 真实 Public 会话外壳中的寄售写入守卫、卸载与换号恢复。

import { expect, test } from "bun:test";
import { parseJson } from "@auction/web-shared/net/wire";

import { loginPublic, PUBLIC_SESSION_CHANGE_STORAGE_KEY } from "../../api/session";
import { fixture, loginReceipt } from "../shell/session-fixture";
import {
  ALICE,
  attemptWrite,
  BOB,
  COMMAND_PATH,
  DETAIL,
  expectedUser,
  listing,
  memory,
  mountPrivate,
  NEW,
  OPERATIONS,
  PENDING_KEY,
  REPRICE,
  RETRY,
  requestBody,
  WITHDRAW,
  writePath,
} from "./private-fixture.test";
import { failure, response, STATUS_PATH, settle, status } from "./read-fixture.test";

const SESSION_PATH = "/api/v1/session";
const GUARDS = [
  [401, "unauthenticated"],
  [409, "session_changed"],
] as const;

async function settleShell() {
  await settle();
  await Bun.sleep(0);
  await settle();
}

for (const operation of OPERATIONS) {
  test.each(GUARDS)(`${operation} %i %s 先了结，再卸载；重登和换号隔离`, async (http, code) => {
    const shell = fixture(null);
    const storage = memory();
    const firstReply = Promise.withResolvers<Response>();
    const writes: { id: unknown; user: string | null }[] = [];
    const pendingAtNotice: (string | undefined)[] = [];
    let observeNotice = false;
    const setLocal = storage.local.set.bind(storage.local);
    storage.local.set = (key, value) => {
      if (observeNotice && key === PUBLIC_SESSION_CHANGE_STORAGE_KEY) {
        pendingAtNotice.push(storage.session.get(PENDING_KEY));
      }
      return setLocal(key, value);
    };
    let signingInAs = ALICE;
    shell.server.mutate = (path, method, init) => {
      if (path === SESSION_PATH && method === "POST") {
        shell.server.identity = signingInAs;
        return loginReceipt(signingInAs);
      }
      if (path === STATUS_PATH) return response(status(true));
      if (path.startsWith(COMMAND_PATH)) return failure(404, "command_not_found");
      const request = { url: new URL(path, "https://auction.test"), init };
      if (writePath(request)) {
        writes.push({ id: requestBody(request).command_id, user: expectedUser(request) });
        return writes.length === 1 ? firstReply.promise : failure(503, "outcome_unknown");
      }
      return response(
        listing("displayed", { allowed_actions: [operation].filter((op) => op !== "register") }),
      );
    };
    const label =
      operation === "register"
        ? "Submit registration"
        : operation === "reprice"
          ? REPRICE
          : WITHDRAW;
    const page = await mountPrivate(operation === "register" ? NEW : DETAIL, null, storage, shell);

    async function login(user: typeof ALICE) {
      signingInAs = user;
      const result = await loginPublic("fake-login-key", shell.signal);
      expect(result).toMatchObject({ ok: true, value: { user_id: user.user_id } });
      await settleShell();
      expect(shell.commandOwner()).toBe(user.user_id);
      expect(shell.session()).toMatchObject({ ok: true, value: { user_id: user.user_id } });
    }

    try {
      await settleShell();
      await login(ALICE);
      const oldButton = page.button(label);
      expect(oldButton).not.toBeNull();
      attemptWrite(page, operation);
      await settle();
      expect(writes).toEqual([{ id: expect.any(String), user: ALICE.user_id }]);
      expect(parseJson(storage.session.get(PENDING_KEY) ?? "")).toMatchObject({
        ok: true,
        value: { command_id: writes[0]?.id, expected_user_id: ALICE.user_id, operation },
      });

      shell.server.identity = null;
      observeNotice = true;
      firstReply.resolve(failure(http, code));
      await settleShell();
      observeNotice = false;
      expect(shell.session()).toEqual({ ok: true, value: null });
      expect(page.button(label)).toBeNull();
      expect(pendingAtNotice).toEqual(["null"]);
      expect(storage.session.get(PENDING_KEY)).toBe("null");

      await login(ALICE);
      expect(page.button(label)).not.toBe(oldButton);
      expect(shell.requests.filter(({ path }) => path.startsWith(COMMAND_PATH))).toEqual([]);
      attemptWrite(page, operation);
      await settle();
      expect(writes).toHaveLength(2);
      expect(writes[1]).toEqual({ id: expect.any(String), user: ALICE.user_id });
      expect(writes[1]?.id).not.toBe(writes[0]?.id);

      // 让 Alice 留下一条真实的未知记录，避免换号断言自然通过。
      const alicePending = storage.session.get(PENDING_KEY);
      expect(parseJson(alicePending ?? "")).toMatchObject({
        ok: true,
        value: { command_id: writes[1]?.id, expected_user_id: ALICE.user_id },
      });
      await login(BOB);
      expect(page.button(label)).not.toBeNull();
      expect(page.button("Retry")).toBeNull();
      expect(page.button(RETRY)).toBeNull();
      expect(page.text()).not.toContain("Checking the result");
      expect(shell.requests.filter(({ path }) => path.startsWith(COMMAND_PATH))).toEqual([]);
      expect(storage.session.get(PENDING_KEY)).toBe(alicePending);
      expect(storage.session.get(`hv.consignment.pending.${BOB.user_id}`)).toBeUndefined();
      expect(page.errors).toEqual([]);
    } finally {
      page.dispose();
      shell.dispose();
    }
  });
}
