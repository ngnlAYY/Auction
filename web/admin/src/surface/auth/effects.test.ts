// 认证副作用适配：登录广播后的单次复核、注销失败分类与续期失败分类。

import { afterEach, expect, spyOn, test } from "bun:test";
import { flush } from "solid-js";
import { installManualTimers, settleAuth as settle } from "./auth-environment-fixture";
import { authenticated, verifyBroadcastLogin } from "./effects-mount-fixture.test";
import { type FetchReply, failure, transport } from "./effects-session-fixture";
import type { LogoutOutcome, RenewalOutcome } from "./session";

afterEach(() => {
  flush();
});

test("login_success_after_broadcast_waits_for_one_cookie_read：广播后登录成功只等待一次 Cookie 读取", async () => {
  await verifyBroadcastLogin(null);
});

test("login_rejection_after_broadcast_is_shown_by_the_remounted_form：广播后登录拒绝由重挂表单展示", async () => {
  await verifyBroadcastLogin("invalid_credentials");
});

test("unknown_login_outcome_after_broadcast_reads_once_and_never_reposts：广播后未知登录结果只读一次且不重发", async () => {
  await verifyBroadcastLogin("timeout");
});

/** 在状态机入口截获分类结果；分类函数本身不导出，只核对实际交给归约器的结果种类。 */
async function captureFinishedOutcomes() {
  const transition = await import("./session/transition");
  const outcomes: (LogoutOutcome | RenewalOutcome)[] = [];
  const realStep = transition.step;
  const spy = spyOn(transition, "step").mockImplementation((state, event, cookie, now) => {
    if (event.kind === "logout_finished" || event.kind === "renewal_finished") {
      outcomes.push(event.outcome);
    }
    return realStep(state, event, cookie, now);
  });
  return {
    outcomes,
    restore: () => spy.mockRestore(),
  };
}

interface LogoutCase {
  readonly failure?: FetchReply;
  readonly beforeSend?: boolean;
  readonly outcome: LogoutOutcome;
}

const LOGOUT_CASES: readonly LogoutCase[] = [
  { failure: failure(403, "csrf_invalid"), outcome: { kind: "unknown", error: "csrf_invalid" } },
  {
    failure: failure(401, "unauthenticated"),
    outcome: { kind: "unknown", error: "unauthenticated" },
  },
  { failure: failure(504, "timeout"), outcome: { kind: "unknown", error: "timeout" } },
  { failure: { kind: "network" }, outcome: { kind: "unknown", error: "network_error" } },
  {
    failure: { kind: "response", status: 200, body: "<html>" },
    outcome: { kind: "unknown", error: "invalid_response" },
  },
  {
    failure: failure(429, "http_429"),
    outcome: { kind: "rate_limited", error: "http_429" },
  },
  {
    failure: failure(403, "origin_forbidden"),
    outcome: { kind: "rejected", error: "origin_forbidden" },
  },
  { beforeSend: true, outcome: { kind: "rejected", error: "invalid_request" } },
];

test("logout_failure_requires_a_read_when_session_or_outcome_is_uncertain：注销失败在会话或结果不确定时要求重读", async () => {
  for (const entry of LOGOUT_CASES) {
    const stubs = transport();
    const captured = await captureFinishedOutcomes();
    let mounted: Awaited<ReturnType<typeof authenticated>> | null = null;
    try {
      mounted = await authenticated(stubs);
      captured.outcomes.length = 0;
      const readsBeforeLogout = stubs.get.calls;
      stubs.rejectLogoutRequest = entry.beforeSend === true;
      mounted.session.dispatch({ kind: "logout_clicked" });
      await settle();
      expect(stubs.logout.calls).toBe(entry.beforeSend === true ? 0 : 1);
      if (entry.failure !== undefined) {
        stubs.logout.finish(entry.failure);
        await settle();
      }
      expect(captured.outcomes).toEqual([entry.outcome]);
      if (entry.outcome.kind === "rejected") {
        expect(stubs.get.calls).toBe(readsBeforeLogout);
        expect(mounted.session.view.peek().canWrite).toBe(true);
        expect(mounted.session.view.peek().loggedOut).toBe(false);
        expect(mounted.session.view.peek().logoutError).toBe(entry.outcome.error);
      } else {
        if (entry.outcome.kind === "succeeded") {
          throw new Error("注销失败用例不应产生成功结果");
        }
        expect(stubs.get.calls).toBe(readsBeforeLogout + 1);
        expect(mounted.session.view.peek().canWrite).toBe(false);
        expect(mounted.session.view.peek().loggingOut).toBe(false);
        expect(mounted.session.view.peek().logoutError).toBe(entry.outcome.error);
      }
    } finally {
      try {
        mounted?.dispose();
      } finally {
        captured.restore();
      }
    }
  }
});

interface RenewalCase {
  readonly failure?: FetchReply;
  readonly beforeSend?: boolean;
  readonly timeout?: boolean;
  readonly outcome: RenewalOutcome;
}

const RENEWAL_CASES: readonly RenewalCase[] = [
  { failure: failure(401, "unauthenticated"), outcome: { kind: "unauthenticated" } },
  {
    failure: failure(429, "rate_limited", { "retry-after": "10" }),
    outcome: { kind: "rate_limited", retryAfterMillis: 10_000 },
  },
  { failure: failure(403, "csrf_invalid"), outcome: { kind: "csrf_rejected" } },
  { failure: failure(503, "storage_unavailable"), outcome: { kind: "unknown" } },
  { failure: failure(504, "http_504"), outcome: { kind: "unknown" } },
  { failure: { kind: "network" }, outcome: { kind: "unknown" } },
  { timeout: true, outcome: { kind: "unknown" } },
  { failure: failure(403, "origin_forbidden"), outcome: { kind: "rejected" } },
  { beforeSend: true, outcome: { kind: "rejected" } },
];

test("renewal_failure_classifies_expiration_backoff_and_uncertainty：续期失败区分到期、退避、代际与未知结果", async () => {
  for (const entry of RENEWAL_CASES) {
    const stubs = transport();
    const timers = installManualTimers();
    let captured: Awaited<ReturnType<typeof captureFinishedOutcomes>> | null = null;
    let clock: { mockRestore(): void } | null = null;
    let mounted: Awaited<ReturnType<typeof authenticated>> | null = null;
    try {
      captured = await captureFinishedOutcomes();
      // 续期退避以单调时钟为基准；冻结它，避免全量套件运行时的毫秒漂移把 10_000 变成 9_999。
      clock = spyOn(performance, "now").mockReturnValue(1_000);
      mounted = await authenticated(stubs);
      captured.outcomes.length = 0;
      const session = mounted.session;
      const readsBeforeRenewal = stubs.get.calls;
      stubs.rejectActivityRequest = entry.beforeSend === true;
      session.dispatch({ kind: "input", onLogout: false });
      await settle();
      expect(stubs.activity.calls).toBe(entry.beforeSend === true ? 0 : 1);
      if (entry.timeout === true) {
        // 真正的传输超时：让共享传输自己的 10 秒预算到期并取消请求。
        timers.fire(10_000);
        await settle();
      } else if (entry.failure !== undefined) {
        stubs.activity.finish(entry.failure);
        await settle();
      }
      expect(captured.outcomes).toEqual([entry.outcome]);
      const view = session.view.peek();
      switch (entry.outcome.kind) {
        case "unauthenticated":
          expect(view.loggedOut).toBe(true);
          expect(view.canWrite).toBe(false);
          break;
        case "rate_limited":
          expect(view.canWrite).toBe(true);
          expect(view.loggedOut).toBe(false);
          // 再次活动时按 retry-after 的 10 秒下界安排，而不是普通合并间隔。
          session.dispatch({ kind: "input", onLogout: false });
          expect(timers.smallestDelay()).toBe(10_000);
          break;
        case "csrf_rejected":
        case "unknown":
          expect(view.canWrite).toBe(false);
          expect(view.loggedOut).toBe(false);
          expect(stubs.get.calls).toBe(readsBeforeRenewal + 1);
          break;
        case "rejected":
          expect(view.canWrite).toBe(true);
          expect(view.loggedOut).toBe(false);
          expect(stubs.get.calls).toBe(readsBeforeRenewal);
          break;
      }
    } finally {
      try {
        mounted?.dispose();
      } finally {
        try {
          clock?.mockRestore();
        } finally {
          try {
            captured?.restore();
          } finally {
            timers.restore();
          }
        }
      }
    }
  }
});
