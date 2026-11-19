// 对账与会话截止、主动退出和私有宿主的交接。

import { expect, jest, test } from "bun:test";
import { err, ok } from "@auction/web-shared/result";
import { flush } from "solid-js";

import {
  logoutPublic,
  PUBLIC_SESSION_CHANGE_STORAGE_KEY,
  registerPublicSessionCheckTrigger,
} from "../../api/session";
import { setSessionPreference } from "../../api/session-preference";
import { currentSessionConfirmation } from "./session-authority";
import { ALICE, BOB, fixture, reject, success } from "./session-fixture";

const DEADLINE = "2090-01-01T12:01:00Z";
const FINITE = { ...ALICE, expires_at: DEADLINE, absolute_expires_at: DEADLINE };
const EXTENDED = {
  ...ALICE,
  expires_at: "2090-01-01T12:02:00Z",
  absolute_expires_at: "2090-01-01T12:02:00Z",
};
const START = Date.parse(DEADLINE) - 1_000;

async function settle(): Promise<void> {
  await new Promise<void>((resolve) => setImmediate(resolve));
  flush();
}

test("权威 GET 在读完前遇到新主修订时，必须重新读取才确认新修订", async () => {
  const mounted = fixture(ALICE);
  const stale = Promise.withResolvers<Response>();
  const current = Promise.withResolvers<Response>();
  try {
    await settle();
    expect(currentSessionConfirmation(mounted.session)).toEqual({ revision: null });
    mounted.server.read = () => stale.promise;
    mounted.checkSession("cookie_changed");
    await settle();
    const firstRead = mounted.requests.length;
    mounted.browser.storage.set(PUBLIC_SESSION_CHANGE_STORAGE_KEY, BOB.user_id);
    mounted.server.read = () => current.promise;
    stale.resolve(success(ALICE));
    await settle();
    expect(mounted.requests).toHaveLength(firstRead + 1);
    expect(currentSessionConfirmation(mounted.session)).toBeNull();
    current.resolve(success(ALICE));
    await settle();
    expect(currentSessionConfirmation(mounted.session)).toEqual({ revision: BOB.user_id });
  } finally {
    stale.resolve(success(ALICE));
    current.resolve(success(ALICE));
    mounted.dispose();
  }
});

test("跨标签页重登待权威读回同一账号才发布偏好修订", async () => {
  const mounted = fixture(ALICE);
  try {
    await settle();
    const login = (revision: string) => {
      mounted.browser.storage.set("hv.public.session-login-revision", revision);
      mounted.browser.storage.set(PUBLIC_SESSION_CHANGE_STORAGE_KEY, revision);
      mounted.browser.notifySessionRevision();
    };
    mounted.server.read = () => reject("temporarily_unavailable", 503);
    login(ALICE.user_id);
    await settle();
    expect(mounted.preferenceReloadRevision()).toBe(0);

    mounted.server.read = () => success(mounted.server.identity);
    mounted.browser.focus();
    await settle();
    expect(mounted.preferenceReloadRevision()).toBe(1);
    mounted.browser.notifySessionRevision();
    await settle();
    expect(mounted.preferenceReloadRevision()).toBe(1);

    mounted.server.identity = BOB;
    login(BOB.user_id);
    await settle();
    expect(mounted.commandOwner()).toBe(BOB.user_id);
    expect(mounted.preferenceReloadRevision()).toBe(1);
  } finally {
    mounted.dispose();
  }
});

test.each([
  ["到期计时先触发", true, FINITE, null],
  ["正文在到期计时前返回", false, FINITE, null],
  ["到期计时已触发且正文带新截止", true, EXTENDED, EXTENDED],
] as const)("对账正文跨过登录截止后再读一次（%s）", async (_name, timerFirst, reply, rechecked) => {
  jest.useFakeTimers();
  jest.setSystemTime(START);
  const mounted = fixture(FINITE);
  const body = Promise.withResolvers<void>();
  try {
    await settle();
    expect(mounted.session()).toEqual(ok(FINITE));
    const from = mounted.requests.length;
    mounted.server.mutate = () =>
      success({ user_id: ALICE.user_id, version: 1, duration_seconds: 60, expires_at: DEADLINE });
    mounted.server.read = () => {
      mounted.server.read = () => success(rechecked);
      return new Response(
        new ReadableStream({
          async start(controller) {
            await body.promise;
            controller.enqueue(
              new TextEncoder().encode(JSON.stringify({ code: "ok", message: "", data: reply })),
            );
            controller.close();
          },
        }),
      );
    };
    const command = { command_id: BOB.user_id, duration_seconds: 60, expected_version: 0 };
    expect((await setSessionPreference(ALICE.user_id, command, mounted.signal)).ok).toBe(true);
    await settle();
    if (timerFirst) {
      jest.advanceTimersByTime(2_000);
      await settle();
    } else {
      jest.setSystemTime(START + 1_001);
    }
    expect(mounted.requests.slice(from).map((request) => request.method)).toEqual(["PUT", "GET"]);
    body.resolve();
    await settle();
    expect(mounted.requests.slice(from).map((request) => request.method)).toEqual([
      "PUT",
      "GET",
      "GET",
    ]);
    expect(mounted.session()).toEqual(ok(rechecked));
    expect(mounted.commandOwner()).toBe(rechecked?.user_id ?? null);
    expect(mounted.sessionEndedElsewhere()).toBe(false);
  } finally {
    body.resolve();
    mounted.dispose();
    jest.useRealTimers();
  }
});

test("到期复核仍返回过期身份时失败关闭，不反复读取", async () => {
  jest.useFakeTimers();
  jest.setSystemTime(START);
  const mounted = fixture(FINITE);
  try {
    await settle();
    const from = mounted.requests.length;
    jest.advanceTimersByTime(2_000);
    await settle();
    expect(mounted.session()).toEqual(err("invalid_response"));
    expect(mounted.sessionEndedElsewhere()).toBe(false);
    jest.advanceTimersByTime(2_000);
    await settle();
    expect(mounted.requests.slice(from).map((request) => request.method)).toEqual(["GET"]);
  } finally {
    mounted.dispose();
    jest.useRealTimers();
  }
});

test.each([true, false])(
  "截止前读回报错仍按上次确认的截止复核一次（服务恢复 %s）",
  async (recovered) => {
    jest.useFakeTimers();
    jest.setSystemTime(START);
    const mounted = fixture(FINITE);
    try {
      await settle();
      const from = mounted.requests.length;
      const unavailable = () => reject("storage_unavailable", 503);
      mounted.server.read = unavailable;
      mounted.browser.focus();
      await settle();
      expect(mounted.session()).toEqual(err("storage_unavailable"));
      expect(mounted.commandOwner()).toBe(ALICE.user_id);
      expect(mounted.requests.slice(from).map(({ method }) => method)).toEqual(["GET"]);

      mounted.server.read = recovered ? () => success(null) : unavailable;
      jest.advanceTimersByTime(2_000);
      await settle();
      expect(mounted.session()).toEqual(recovered ? ok(null) : err("storage_unavailable"));
      expect(mounted.commandOwner()).toBe(recovered ? null : ALICE.user_id);
      expect(mounted.sessionEndedElsewhere()).toBe(false);
      jest.advanceTimersByTime(60_000);
      await settle();
      expect(mounted.requests.slice(from).map(({ method }) => method)).toEqual(["GET", "GET"]);
    } finally {
      mounted.dispose();
      jest.useRealTimers();
    }
  },
);

test.each([false, true])(
  "明确退出即卸载私有宿主，即使读回失败（另一标签页 %s）",
  async (otherTab) => {
    const mounted = fixture(ALICE);
    const stop = otherTab ? registerPublicSessionCheckTrigger(() => {}) : () => {};
    try {
      await settle();
      expect(mounted.commandOwner()).toBe(ALICE.user_id);
      mounted.server.mutate = () => success(null);
      mounted.server.read = () => reject("storage_unavailable", 503);
      expect(await logoutPublic(ALICE.user_id, mounted.signal)).toEqual(ok(null));
      if (otherTab) {
        mounted.browser.notifySessionRevision();
      }
      expect(mounted.commandOwner.peek()).toBeNull();
      await settle();
      expect(mounted.session()).toEqual(err("storage_unavailable"));
      expect(mounted.owners).toEqual([null, ALICE.user_id, null]);
      expect(mounted.sessionEndedElsewhere()).toBe(false);
    } finally {
      stop();
      mounted.dispose();
    }
  },
);

test.each([false, true])(
  "退出与对账都失败后，后续撤销应提示（另一标签页 %s）",
  async (otherTab) => {
    const mounted = fixture(ALICE);
    const stop = otherTab ? registerPublicSessionCheckTrigger(() => {}) : () => {};
    try {
      await settle();
      const from = mounted.requests.length;
      mounted.server.mutate = () => reject("storage_unavailable", 503);
      mounted.server.read = () => reject("storage_unavailable", 503);
      expect(await logoutPublic(ALICE.user_id, mounted.signal)).toEqual(err("storage_unavailable"));
      if (otherTab) {
        mounted.browser.notifySessionRevision();
      }
      await settle();
      expect(mounted.requests.slice(from).map((request) => request.method)).toEqual([
        "DELETE",
        "GET",
      ]);
      expect(mounted.session()).toEqual(err("storage_unavailable"));
      expect(mounted.commandOwner()).toBe(ALICE.user_id);
      expect(mounted.sessionEndedElsewhere()).toBe(false);

      mounted.server.read = () => success(null);
      mounted.browser.focus();
      await settle();
      expect(mounted.session()).toEqual(ok(null));
      expect(mounted.commandOwner()).toBeNull();
      expect(mounted.sessionEndedElsewhere()).toBe(true);
    } finally {
      stop();
      mounted.dispose();
    }
  },
);
