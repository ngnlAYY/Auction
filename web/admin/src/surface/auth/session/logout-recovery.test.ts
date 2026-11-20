// 退出未确认后的人工选择、续期与读取交错，以及跨代际通知。

import { expect, test } from "bun:test";

import { A, B, MISSING, SessionSequence, UNAVAILABLE } from "./boundary-fixture.test";
import { type Event, initialState } from "./index";
import {
  armedTimer,
  loggedOut,
  loginOk,
  loginTimeout,
  NOW,
  projectionAfter,
  readFailed,
  readOk,
  readTicket,
} from "./session-fixture";
import { startLogin } from "./transition";

test("身份未确认时普通退出只发 GET；退出未确认恢复同代后仍要求人工决定", () => {
  const normal = new SessionSequence();
  normal.step({ kind: "visible" }, B.cookie);
  const again = readTicket(normal.step({ kind: "logout_clicked" }));
  expect(normal.deletes).toBe(0);
  normal.expectCovered();
  normal.step(readOk(A, again));
  expect(normal.view.canWrite).toBe(true);
  normal.logout();
  expect(normal.deletes).toBe(1);

  const sequence = new SessionSequence();
  const logout = sequence.logout();
  const checking = readTicket(sequence.logoutResult(logout, "unknown", MISSING));
  expect(sequence.view.logoutDecisionBusy).toBe(true);
  expect(sequence.step({ kind: "continue_logout" })).toEqual([]);
  sequence.step(readFailed(checking, MISSING), MISSING);
  sequence.step(readOk(A, sequence.read()));
  sequence.expectCovered();
  expect(sequence.view.logoutUnconfirmed).toBe(true);
  expect(sequence.view.logoutDecisionBusy).toBe(false);
  const continued = sequence.logout(true);
  expect(sequence.step({ kind: "visible" })).toEqual([]);
  expect(
    sequence
      .logoutResult(continued, "succeeded", MISSING)
      .filter((effect) => effect.kind === "read"),
  ).toEqual([]);
  expect(sequence.notices).toBe(1);
});

test("退出锁阻止例行复核、迟到活动和定时器开写，留在后台仅接受专属同代结果", () => {
  const sequence = new SessionSequence(initialState());
  const initial = readTicket(sequence.step({ kind: "start" }));
  const [checkTimer] = armedTimer(sequence.step(readOk(A, initial)), "check");
  const renewal = sequence.renewal();
  sequence.step({ kind: "renewal_sending", ticket: renewal });
  const logout = sequence.logout();
  const automatic = readTicket(sequence.logoutResult(logout, "unknown"));
  sequence.step(readOk(A, automatic));
  sequence.expectCovered();

  const visible = readTicket(sequence.step({ kind: "visible" }));
  sequence.step(readOk(A, visible));
  sequence.expectCovered();
  for (const event of [
    { kind: "input", onLogout: false },
    { kind: "timer_elapsed", timer: "check", ticket: checkTimer },
    { kind: "timer_elapsed", timer: "renewal", ticket: renewal },
    {
      kind: "renewal_finished",
      ticket: renewal,
      cookie: A.cookie,
      outcome: { kind: "succeeded", session: projectionAfter(A, 120, 900, NOW).session },
    },
  ] satisfies Event[]) {
    expect(sequence.step(event)).toEqual([]);
    sequence.expectCovered();
  }
  sequence.step(readOk(A, sequence.read()));
  sequence.expectCovered();
  expect(sequence.deletes).toBe(1);
  expect(sequence.renewals).toBe(1);

  const firstStay = readTicket(sequence.step({ kind: "stay_after_logout" }));
  expect(sequence.step({ kind: "stay_after_logout" })).toEqual([]);
  sequence.step(readFailed(firstStay, A.cookie));
  sequence.expectCovered();
  const secondStay = readTicket(sequence.step({ kind: "stay_after_logout" }));
  sequence.step(readOk(A, firstStay));
  sequence.expectCovered();
  expect(sequence.view.logoutDecisionBusy).toBe(true);
  sequence.step(readOk(A, secondStay));
  expect(sequence.view.canWrite).toBe(true);
  expect(sequence.view.keyVisibility.hidden).toBe(false);
  expect(sequence.view.logoutUnconfirmed).toBe(false);
  expect(sequence.deletes).toBe(1);
  expect(sequence.notices).toBe(0);
});

test("留在后台的可信 401 结账，实发代际变化只复核且不打开写入", () => {
  const gone = new SessionSequence();
  gone.step(readOk(A, readTicket(gone.logoutResult(gone.logout(), "unknown"))));
  const ticket = readTicket(gone.step({ kind: "stay_after_logout" }));
  gone.unauthorized(ticket);
  expect(gone.view.loggedOut).toBe(true);
  expect(gone.notices).toBe(1);
  expect(gone.deletes).toBe(1);

  const changed = new SessionSequence();
  changed.step(readOk(A, readTicket(changed.logoutResult(changed.logout(), "unknown"))));
  const stay = readTicket(changed.step({ kind: "stay_after_logout" }));
  changed.step({ kind: "read_sending", ticket: stay }, MISSING);
  const restore = readTicket(changed.step(readOk(A, stay)));
  changed.step(readOk(A, restore));
  changed.expectCovered();
  expect(changed.view.logoutUnconfirmed).toBe(true);
  expect(changed.deletes).toBe(1);
  expect(changed.notices).toBe(0);

  for (const cookie of [B.cookie, MISSING]) {
    const guarded = new SessionSequence();
    guarded.step(readOk(A, readTicket(guarded.logoutResult(guarded.logout(), "unknown"))));
    expect(readTicket(guarded.step({ kind: "continue_logout" }, cookie))).toBeGreaterThan(0);
    guarded.expectCovered();
    expect(guarded.deletes).toBe(1);
  }
});

test("DELETE 结果未知后复核 503，切回标签读到同代 200 仍禁止写入", () => {
  const sequence = new SessionSequence();
  expect(sequence.view.canWrite).toBe(true);
  const review = readTicket(sequence.logoutResult(sequence.logout(), "unknown"));
  expect(sequence.view.canWrite).toBe(false);

  sequence.step(readFailed(review, A.cookie));
  expect(sequence.view.canWrite).toBe(false);
  const ordinary = readTicket(sequence.step({ kind: "visible" }));
  expect(ordinary).not.toBe(review);
  sequence.step({ kind: "read_sending", ticket: ordinary });
  expect(sequence.view.canWrite).toBe(false);
  sequence.step(readOk(A, ordinary));

  expect(sequence.view.canWrite).toBe(false);
  expect(sequence.view.logoutUnconfirmed).toBe(true);
  expect(sequence.deletes).toBe(1);
  expect(sequence.notices).toBe(0);
});

for (const logoutOutcome of ["rate_limited", "unknown"] as const) {
  for (const oldSuccessFirst of [false, true]) {
    test(`退出 ${logoutOutcome}、旧 GET 200 ${oldSuccessFirst ? "先于" : "晚于"}活动 401 时关写并结账`, () => {
      for (const newSucceeds of [false, true]) {
        const sequence = new SessionSequence();
        const renewal = sequence.renewal();
        sequence.step({ kind: "renewal_sending", ticket: renewal });
        const logout = sequence.logout();
        const old = readTicket(sequence.logoutResult(logout, logoutOutcome));
        if (oldSuccessFirst) {
          sequence.step(readOk(A, old));
          expect(sequence.view.canWrite).toBe(logoutOutcome === "rate_limited");
        }
        const fresh = readTicket(
          sequence.step({
            kind: "renewal_finished",
            ticket: renewal,
            cookie: A.cookie,
            outcome: { kind: "unauthenticated" },
          }),
        );
        expect(fresh).not.toBe(old);
        expect(sequence.view.canWrite).toBe(false);
        expect(sequence.notices).toBe(1);
        if (!oldSuccessFirst) sequence.step(readOk(A, old));
        expect(sequence.view.canWrite).toBe(false);
        if (newSucceeds) sequence.step(readOk(A, fresh));
        else sequence.unauthorized(fresh);
        expect(sequence.view.canWrite).toBe(newSucceeds);
        expect(sequence.notices).toBe(1);
        expect(sequence.deletes).toBe(1);
      }
    });
  }
}

test("活动 401 先卸载宿主再遇 DELETE 429，后续 503 不清除归属账本", () => {
  for (const peerFirst of [false, true]) {
    const sequence = new SessionSequence();
    const renewal = sequence.renewal();
    sequence.step({ kind: "renewal_sending", ticket: renewal });
    const sent = sequence.logout();
    expect(
      sequence.step({
        kind: "renewal_finished",
        ticket: renewal,
        cookie: A.cookie,
        outcome: { kind: "unauthenticated" },
      }),
    ).toEqual([]);
    expect(sequence.view.session).toBeNull();
    expect(sequence.view.canWrite).toBe(false);
    expect(sequence.notices).toBe(0);
    if (peerFirst) sequence.step({ kind: "broadcast" });
    const review = readTicket(sequence.logoutResult(sent, "rate_limited"));
    sequence.step(readFailed(review, A.cookie));
    expect(sequence.notices).toBe(0);
    const fresh = sequence.read();
    sequence.unauthorized(fresh);
    expect(sequence.notices).toBe(peerFirst ? 0 : 1);
    expect(sequence.view.loggedOut).toBe(true);
    sequence.unauthorized(fresh);
    expect(sequence.notices).toBe(peerFirst ? 0 : 1);
  }
});

test("确认不同会话后独立结账，作废的 A 读取、续期和退出票不会结束 B", () => {
  const sequence = new SessionSequence();
  const renewal = sequence.renewal();
  sequence.step({ kind: "renewal_sending", ticket: renewal });
  const oldRead = sequence.read();
  const logout = sequence.logout();
  sequence.logoutResult(logout, "succeeded", MISSING);
  expect(sequence.notices).toBe(1);
  const oldEpoch = sequence.view.epoch;
  const discover = readTicket(sequence.step({ kind: "visible" }, B.cookie));
  sequence.step(readOk(B, discover), B.cookie);
  expect(sequence.view.epoch).toBeGreaterThan(oldEpoch);

  expect(
    sequence.step(
      {
        kind: "renewal_finished",
        ticket: renewal,
        cookie: A.cookie,
        outcome: { kind: "unauthenticated" },
      },
      B.cookie,
    ),
  ).toEqual([]);
  expect(sequence.unauthorized(oldRead, B.cookie)).toEqual([]);
  expect(sequence.logoutResult(logout, "succeeded", B.cookie)).toEqual([]);
  expect(sequence.view.canWrite).toBe(true);
  expect(sequence.view.session).toEqual(B.session);
  expect(sequence.notices).toBe(1);
  sequence.unauthorized(sequence.read(B.cookie), B.cookie);
  expect(sequence.notices).toBe(2);
  expect(sequence.view.loggedOut).toBe(true);
});

test("同代广播保留本地登录复核，换代只替换该票，旧 200/401 不得抢占", () => {
  for (const replacement of [false, true]) {
    for (const staleSuccess of [false, true]) {
      const started = startLogin(loggedOut());
      if (started === null) throw new Error("应能开始本页登录");
      const sequence = new SessionSequence(started.state);
      expect(sequence.step({ kind: "visible" })).toEqual([]);
      const loginRead = readTicket(
        sequence.step({ kind: "login_finished", ticket: started.ticket, outcome: loginTimeout() }),
      );
      expect(sequence.view.loginPending).toBe(true);
      const observed = replacement ? B : A;
      const broadcast = sequence.step({ kind: "broadcast" }, observed.cookie);
      if (!replacement) expect(broadcast).toEqual([]);
      const active = replacement ? readTicket(broadcast) : loginRead;
      expect(sequence.view.loginPending).toBe(true);
      if (replacement) {
        if (staleSuccess) expect(sequence.step(readOk(A, loginRead), B.cookie)).toEqual([]);
        else expect(sequence.unauthorized(loginRead, B.cookie)).toEqual([]);
        expect(sequence.view.loginPending).toBe(true);
        expect(sequence.view.canWrite).toBe(false);
      }
      sequence.step(readOk(observed, active), observed.cookie);
      expect(sequence.view.canWrite).toBe(true);
      expect(sequence.view.session).toEqual(observed.session);
      expect(sequence.notices).toBe(0);
    }
  }
});

test("被动探测先于 POST 或晚于专属 GET 都无法替代本页登录结果", () => {
  for (const postFirst of [false, true]) {
    const sequence = new SessionSequence(loggedOut());
    const discovered = readTicket(sequence.step({ kind: "visible" }));
    expect(sequence.step({ kind: "broadcast" })).toEqual([]);
    const started = startLogin(sequence.state);
    if (started === null) throw new Error("被动探测不可拦本页登录");
    sequence.state = started.state;
    if (!postFirst) {
      expect(sequence.step(readOk(A, discovered))).toEqual([]);
      expect(sequence.view.loginPending).toBe(true);
      expect(sequence.view.canWrite).toBe(false);
    }
    const loginGet = readTicket(
      sequence.step(
        { kind: "login_finished", ticket: started.ticket, outcome: loginOk(B) },
        B.cookie,
      ),
    );
    if (postFirst) expect(sequence.step(readOk(A, discovered), B.cookie)).toEqual([]);
    sequence.step(readOk(B, loginGet), B.cookie);
    expect(sequence.view.session).toEqual(B.session);
    expect(sequence.view.canWrite).toBe(true);
  }
});

test("登录 POST 的有效 B 正文影响原过期账本，写入仍只由登录专属 GET 确认", () => {
  for (const returned of [A, B]) {
    const sequence = new SessionSequence();
    const activity = sequence.renewal();
    sequence.step({ kind: "renewal_sending", ticket: activity });
    const sent = sequence.logout();
    sequence.step({
      kind: "renewal_finished",
      ticket: activity,
      cookie: A.cookie,
      outcome: { kind: "unauthenticated" },
    });
    const review = readTicket(sequence.logoutResult(sent, "rate_limited"));
    sequence.unauthorized(review, UNAVAILABLE);
    expect(sequence.view.loggedOut).toBe(true);
    expect(sequence.notices).toBe(0);

    const login = startLogin(sequence.state);
    if (login === null) throw new Error("应有登录票");
    sequence.state = login.state;
    const loginRead = readTicket(
      sequence.step(
        { kind: "login_finished", ticket: login.ticket, outcome: loginOk(returned) },
        MISSING,
      ),
    );
    expect(sequence.view.loginPending).toBe(true);
    expect(sequence.view.canWrite).toBe(false);
    sequence.unauthorized(loginRead, MISSING);
    expect(sequence.view.loggedOut).toBe(true);
    expect(sequence.notices).toBe(returned === A ? 1 : 0);
  }
});

test("当前 GET 的可信 401 与旧续期 200 互换顺序，结束后旧票仍不能开写", () => {
  for (const renewalFirst of [true, false]) {
    const sequence = new SessionSequence();
    const renewal = sequence.renewal();
    sequence.step({ kind: "renewal_sending", ticket: renewal });
    const old = sequence.read();
    sequence.step({ kind: "visible" }, B.cookie);
    const current = sequence.read();
    sequence.expectCovered();

    const renewed: Event = {
      kind: "renewal_finished",
      ticket: renewal,
      cookie: A.cookie,
      outcome: { kind: "succeeded", session: projectionAfter(A, 120, 900, NOW).session },
    };
    if (renewalFirst) sequence.step(renewed);
    expect(sequence.view.canWrite).toBe(false);
    sequence.unauthorized(current);
    sequence.step(readOk(A, old));
    if (!renewalFirst) sequence.step(renewed);
    expect(sequence.view.canWrite).toBe(false);
    expect(sequence.view.loggedOut).toBe(true);
    expect(sequence.notices).toBe(1);
  }
});

test("连续两个外部 401 作废两张早先的读票，旧 200 与旧宿主不改变当前视图", () => {
  for (const authorize of [false, true]) {
    const sequence = new SessionSequence();
    const original = sequence.read();
    const epoch = sequence.view.epoch;
    const evidence = { kind: "external_unauthenticated", epoch, session: A.session } as const;
    const first = readTicket(sequence.step(evidence));
    const second = readTicket(sequence.step(evidence));
    expect(new Set([original, first, second]).size).toBe(3);
    sequence.expectCovered();
    for (const obsolete of [original, first]) {
      expect(sequence.step(readOk(A, obsolete))).toEqual([]);
      sequence.expectCovered();
    }
    expect(sequence.step({ ...evidence, epoch: epoch - 1 }, B.cookie)).toEqual([]);
    expect(sequence.step({ ...evidence, session: B.session })).toEqual([]);
    sequence.expectCovered();
    if (authorize) sequence.step(readOk(A, second));
    else sequence.unauthorized(second);
    expect(sequence.view.canWrite).toBe(authorize);
    expect(sequence.view.loggedOut).toBe(!authorize);
    expect(sequence.notices).toBe(authorize ? 0 : 1);
  }
});
