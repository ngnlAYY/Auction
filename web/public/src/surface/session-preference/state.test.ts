// 登录时长提交在真实 Cookie 锁后再次核对会话，并保留已未知的冻结请求。

import { expect, test } from "bun:test";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createTaskScope } from "@auction/web-shared/reactive/tasks";
import { createRoot, flush } from "solid-js";
import { PUBLIC_SESSION_CHANGE_STORAGE_KEY } from "../../api/session";
import { ALICE, BOB, fixture, reject, success } from "../shell/session-fixture";
import { preferenceOwnerSubmit, preferenceStateDefault } from "./state";

async function settle(): Promise<void> {
  await Bun.sleep(0);
  flush();
}

test.each([
  ["复核开始", false],
  ["复核开始", true],
  ["跨页修订先于广播", false],
  ["跨页修订先于广播", true],
  ["跨页修订先于提交", false],
  ["跨页修订先于提交", true],
] as const)("等锁时%s，登录时长未发送（重试 %s）", async (boundary, retry) => {
  const mounted = fixture(ALICE);
  const lock = Promise.withResolvers<void>();
  const payload = { command_id: ALICE.user_id, duration_seconds: null, expected_version: 1 };
  const state = createCell({
    ...preferenceStateDefault(),
    authoritative: { user_id: ALICE.user_id, duration_seconds: null, version: 1, expires_at: null },
    pending: retry ? payload : null,
    pendingUncertain: retry,
    error: retry ? { code: "network_error", status: 0 } : null,
  });
  const participant = createRoot((dispose) => ({ tasks: createTaskScope(), dispose }));
  const owner = {
    userId: ALICE.user_id,
    identity: mounted.commandOwner,
    session: mounted.session,
    state,
    tasks: participant.tasks,
    readStarted: () => {},
  };
  const bodies: unknown[] = [];
  try {
    await settle();
    const hold = mounted.browser.hold(lock.promise);
    mounted.server.mutate = (_path, _method, init) => {
      bodies.push(init?.body);
      return reject("network_error", 503);
    };
    if (boundary === "跨页修订先于提交") {
      mounted.browser.storage.set(PUBLIC_SESSION_CHANGE_STORAGE_KEY, BOB.user_id);
      expect(mounted.session.state()).toBe("ready");
    }
    preferenceOwnerSubmit(owner);
    expect(state.peek().busy).toBe(true);
    await settle();
    expect(bodies).toEqual([]);
    mounted.server.identity = boundary === "跨页修订先于提交" ? ALICE : BOB;
    mounted.server.read = () => reject("storage_unavailable", 503);
    if (boundary === "复核开始") {
      mounted.checkSession("cookie_changed");
    } else if (boundary === "跨页修订先于广播") {
      mounted.browser.storage.set(PUBLIC_SESSION_CHANGE_STORAGE_KEY, BOB.user_id);
      expect(mounted.session.state()).toBe("ready");
    }
    lock.resolve();
    await hold;
    await settle();
    expect(bodies).toEqual([]);
    expect(state.peek().busy).toBe(false);
    expect(state.peek().pending).toEqual(retry ? payload : null);
    expect(state.peek().pendingUncertain).toBe(retry);
    expect(state.peek().error?.code).toBe(retry ? "network_error" : "command_not_sent");
    expect(mounted.requests.at(-1)?.path).toBe("/api/v1/session");

    mounted.server.identity = ALICE;
    mounted.server.read = () => success(ALICE);
    mounted.checkSession("cookie_changed");
    await settle();
    expect(bodies).toEqual([]);
    preferenceOwnerSubmit(owner);
    await settle();
    expect(bodies).toHaveLength(1);
    if (retry) expect(JSON.parse(String(bodies[0]))).toEqual(payload);
  } finally {
    lock.resolve();
    participant.dispose();
    mounted.dispose();
  }
});
