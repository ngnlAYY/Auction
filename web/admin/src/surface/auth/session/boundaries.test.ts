// F11 会话边界的定点序列：按请求票、可见宿主、写许可与结束通知断言。

import { expect, test } from "bun:test";

import { A, B, MISSING, SessionSequence, UNAVAILABLE } from "./boundary-fixture.test";
import { type Event, stateView } from "./index";
import {
  logoutTicket,
  NOW,
  projectionAfter,
  readFailed,
  readOk,
  readTicket,
} from "./session-fixture";
import { step } from "./transition";

for (const [label, changed] of [
  ["B", B.cookie],
  ["缺失", MISSING],
] as const) {
  test(`GET 登记、实发和旧回执收包分别观察到 ${label} 时收起明文并保留宿主`, () => {
    for (const point of ["register", "send", "receive"] as const) {
      const sequence = new SessionSequence();
      const epoch = sequence.view.epoch;
      const revision = sequence.view.keyVisibility.revision;
      const old = readTicket(sequence.step({ kind: "visible" }));
      expect(sequence.view.canWrite).toBe(true);
      expect(sequence.view.keyVisibility.hidden).toBe(false);
      if (point === "register") {
        const effects = sequence.step({ kind: "visible" }, changed);
        expect(readTicket(effects)).not.toBe(old);
      } else if (point === "send") {
        expect(sequence.step({ kind: "read_sending", ticket: old }, changed)).toEqual([]);
      } else {
        const effects = sequence.step(readOk(A, old), changed);
        expect(readTicket(effects)).not.toBe(old);
      }
      sequence.expectCovered(epoch);
      expect(sequence.view.keyVisibility.revision).toBeGreaterThan(revision);
      expect(sequence.step({ kind: "visible" }, changed)).toEqual([]);
      const same = sequence.read();
      sequence.step(readOk(A, same));
      expect(sequence.view.canWrite).toBe(true);
      expect(sequence.view.keyVisibility.hidden).toBe(false);
      expect(sequence.view.epoch).toBe(epoch);
      expect(sequence.view.keyVisibility.revision).toBeGreaterThan(revision);
    }
  });

  test(`活动 POST 登记、实发和收包分别观察到 ${label} 时停止续期并复核`, () => {
    const register = new SessionSequence();
    const [pendingInput, effects] = step(
      register.state,
      { kind: "input", onLogout: false },
      A.cookie,
      NOW,
    );
    const timer = effects.find(
      (effect) => effect.kind === "arm_timer" && effect.timer === "renewal",
    );
    if (timer === undefined || timer.kind !== "arm_timer") throw new Error("应安排活动续期");
    const [checking, changedEffects] = step(
      pendingInput,
      { kind: "timer_elapsed", timer: "renewal", ticket: timer.ticket },
      changed,
      NOW + timer.delay,
    );
    expect(readTicket(changedEffects)).toBeGreaterThan(timer.ticket);
    expect(changedEffects.some((effect) => effect.kind === "renew")).toBe(false);
    expect(stateView(checking).keyVisibility.hidden).toBe(true);

    const send = new SessionSequence();
    const ticket = send.renewal();
    const sending = send.step({ kind: "renewal_sending", ticket }, changed);
    expect(readTicket(sending)).toBeGreaterThan(ticket);
    send.expectCovered();

    const receive = new SessionSequence();
    const renewal = receive.renewal();
    receive.step({ kind: "renewal_sending", ticket: renewal });
    const oldRead = readTicket(receive.step({ kind: "visible" }));
    receive.step(readFailed(oldRead, A.cookie));
    expect(receive.view.canWrite).toBe(false);
    expect(receive.view.keyVisibility.hidden).toBe(false);
    const review = readTicket(
      receive.step(
        {
          kind: "renewal_finished",
          ticket: renewal,
          cookie: A.cookie,
          outcome: { kind: "succeeded", session: projectionAfter(A, 120, 900, NOW).session },
        },
        changed,
      ),
    );
    receive.expectCovered();
    expect(receive.view.session).toEqual(A.session);
    const missingReview = changed.kind === "missing" ? review : receive.read(MISSING);
    receive.unauthorized(missingReview, MISSING);
    expect(receive.notices).toBe(changed.kind === "missing" ? 1 : 0);
  });

  test(`DELETE 登记、实发和收包观察到 ${label} 时按原发送证据裁定`, () => {
    const register = new SessionSequence();
    const effects = register.step({ kind: "logout_clicked" }, changed);
    expect(register.deletes).toBe(0);
    expect(readTicket(effects)).toBeGreaterThan(0);
    register.expectCovered();

    const send = new SessionSequence();
    const ticket = logoutTicket(send.step({ kind: "logout_clicked" }));
    const review = readTicket(send.step({ kind: "logout_sending", ticket }, changed));
    expect(send.view.loggingOut).toBe(false);
    send.expectCovered();
    expect(
      send.unauthorized(review, changed).filter((effect) => effect.kind === "broadcast_ended"),
    ).toHaveLength(changed.kind === "missing" ? 1 : 0);

    const receive = new SessionSequence();
    const sent = receive.logout();
    const result = receive.logoutResult(sent, "succeeded", changed);
    expect(receive.notices).toBe(changed.kind === "missing" ? 1 : 0);
    if (changed.kind === "present") {
      receive.expectCovered();
      expect(readTicket(result)).toBeGreaterThan(sent);
    } else {
      expect(receive.view.loggedOut).toBe(true);
    }
    receive.logoutResult(sent, "succeeded", changed);
    expect(receive.notices).toBe(changed.kind === "missing" ? 1 : 0);
  });
}

test("同代续期采用较新的截止投影，普通同代 503 仍保留 Key 展示", () => {
  const sequence = new SessionSequence();
  const ticket = sequence.renewal();
  sequence.step({ kind: "renewal_sending", ticket });
  const later = projectionAfter(A, 120, 900, NOW).session;
  sequence.step({
    kind: "renewal_finished",
    ticket,
    cookie: A.cookie,
    outcome: { kind: "succeeded", session: later },
  });
  expect(sequence.view.session).toEqual(later);
  expect(sequence.view.canWrite).toBe(true);
  sequence.step(readFailed(sequence.read(), A.cookie));
  expect(sequence.view.canWrite).toBe(false);
  expect(sequence.view.keyVisibility.hidden).toBe(false);
  expect(sequence.view.session).toEqual(later);
});

test("退出正文中断、复核 503、可信 401 依次产生 0、0、1 条通知", () => {
  const sequence = new SessionSequence();
  const sent = sequence.logout();
  const review = readTicket(sequence.logoutResult(sent, "unknown", MISSING));
  expect(sequence.notices).toBe(0);
  expect(sequence.view.logoutUnconfirmed).toBe(true);
  sequence.step(readFailed(review, MISSING), MISSING);
  expect(sequence.notices).toBe(0);
  const retry = sequence.read(MISSING);
  sequence.unauthorized(review, MISSING);
  expect(sequence.notices).toBe(0);
  sequence.unauthorized(retry, MISSING);
  expect(sequence.notices).toBe(1);
  expect(sequence.view.loggedOut).toBe(true);
  sequence.unauthorized(retry, MISSING);
  sequence.logoutResult(sent, "succeeded", MISSING);
  expect(sequence.notices).toBe(1);
});

test("观察到 B 后迟到的旧 DELETE 429 不让 B 或缺失 Cookie 的 401 归给 A", () => {
  for (const observed of [B.cookie, MISSING]) {
    const sequence = new SessionSequence();
    const sent = sequence.logout();
    sequence.step({ kind: "visible" }, B.cookie);
    const read = readTicket(sequence.logoutResult(sent, "rate_limited", B.cookie));
    sequence.unauthorized(observed === B.cookie ? read : sequence.read(observed), observed);
    expect(sequence.notices).toBe(0);
    expect(sequence.view.loggedOut).toBe(true);
  }
});

test("活动回执异代正文仍遮住 Key，旧票的异代正文不影响当前 A", () => {
  for (const current of [true, false]) {
    const sequence = new SessionSequence();
    const sent = sequence.renewal();
    sequence.step({ kind: "renewal_sending", ticket: sent });
    const effects = sequence.step(
      {
        kind: "renewal_finished",
        ticket: current ? sent : sent - 1,
        cookie: A.cookie,
        outcome: { kind: "succeeded", session: B.session },
      },
      UNAVAILABLE,
    );
    expect(sequence.view.keyVisibility.hidden).toBe(current);
    expect(sequence.view.canWrite).toBe(!current);
    if (current) expect(readTicket(effects)).toBeGreaterThan(sent);
    else expect(effects).toEqual([]);
  }
});

test("当前 GET 的 B 正文在过时、缺失和不可读分支均能阻止误播，旧票与坏正文不能", () => {
  for (const [registered, received] of [
    [A.cookie, MISSING],
    [MISSING, MISSING],
    [UNAVAILABLE, UNAVAILABLE],
  ] as const) {
    for (const hasLogout of [false, true]) {
      const sequence = new SessionSequence();
      if (hasLogout) {
        const sent = sequence.logout();
        sequence.logoutResult(sent, "unknown");
      }
      if (registered === UNAVAILABLE) sequence.read(MISSING);
      const ticket = sequence.read(registered);
      sequence.step({ kind: "read_sending", ticket }, registered);
      sequence.step(
        {
          kind: "read_finished",
          ticket,
          cookie: registered,
          outcome: { kind: "authenticated", session: B.session },
        },
        received,
      );
      sequence.expectCovered();
      const after = sequence.read(MISSING);
      sequence.step(readFailed(after, MISSING), MISSING);
      sequence.unauthorized(sequence.read(MISSING), MISSING);
      expect(sequence.notices).toBe(0);
    }
  }
  for (const stale of [false, true]) {
    const sequence = new SessionSequence();
    const ticket = sequence.read(MISSING);
    const ignored: Event = stale
      ? {
          kind: "read_finished",
          ticket: ticket - 1,
          cookie: MISSING,
          outcome: { kind: "authenticated", session: B.session },
        }
      : {
          kind: "read_finished",
          ticket,
          cookie: MISSING,
          outcome: { kind: "failed", error: "invalid_response" },
        };
    sequence.step(ignored, MISSING);
    const effects = sequence.step({ kind: "visible" }, MISSING);
    sequence.unauthorized(stale ? ticket : readTicket(effects), MISSING);
    expect(sequence.notices).toBe(1);
  }
});

test("完整 DELETE 清除 Cookie 可独立结账，人工继续派发后见 B 亦只通知一次", () => {
  for (const continuing of [false, true]) {
    for (const cookie of [MISSING, B.cookie]) {
      const sequence = new SessionSequence();
      if (continuing) {
        const first = sequence.logout();
        const after = readTicket(sequence.logoutResult(first, "unknown"));
        sequence.step(readFailed(after, A.cookie), UNAVAILABLE);
        sequence.step(readFailed(sequence.read(UNAVAILABLE), UNAVAILABLE), UNAVAILABLE);
      }
      const ticket = sequence.logout(continuing);
      sequence.step({ kind: "visible" }, B.cookie);
      sequence.step({ kind: "visible" }, B.cookie);
      const effects = sequence.logoutResult(ticket, "succeeded", cookie);
      const check = readTicket(effects);
      expect(sequence.notices).toBe(cookie.kind === "missing" ? 1 : 0);
      sequence.logoutResult(ticket, "succeeded", cookie);
      sequence.unauthorized(check, cookie);
      expect(sequence.notices).toBe(cookie.kind === "missing" ? 1 : 0);
    }
  }
});

test("人工继续退出发送前失败保留首个发送票，同代明确拒绝或限流仍锁定", () => {
  for (const failure of ["not_sent", "rate_limited", "cookie_changed"] as const) {
    const sequence = new SessionSequence();
    const first = sequence.logout();
    sequence.step(readOk(A, readTicket(sequence.logoutResult(first, "unknown"))));
    const retry = logoutTicket(sequence.step({ kind: "continue_logout" }));
    expect(sequence.step({ kind: "continue_logout" })).toEqual([]);
    expect(sequence.step({ kind: "stay_after_logout" })).toEqual([]);
    if (failure === "cookie_changed") {
      const effects = sequence.step({ kind: "logout_sending", ticket: retry }, MISSING);
      expect(sequence.view.loggingOut).toBe(false);
      sequence.unauthorized(readTicket(effects), MISSING);
    } else {
      sequence.step({ kind: "logout_sending", ticket: retry });
      if (failure === "not_sent") {
        sequence.step({
          kind: "logout_not_sent",
          ticket: retry,
          cookie: A.cookie,
          outcome: { kind: "rejected", error: "invalid_request" },
        });
      } else {
        sequence.logoutResult(retry, "rate_limited");
      }
      expect(sequence.view.logoutUnconfirmed).toBe(true);
      sequence.expectCovered();
      sequence.unauthorized(sequence.read(MISSING), MISSING);
    }
    expect(sequence.view.loggedOut).toBe(true);
    expect(sequence.notices).toBe(1);
  }
});

test("同代对端通知加 503 后静默结清本地退出，独立成功先到也不重复", () => {
  for (const successFirst of [false, true]) {
    const sequence = new SessionSequence();
    const sent = sequence.logout();
    if (successFirst) {
      sequence.logoutResult(sent, "succeeded", MISSING);
    } else {
      sequence.logoutResult(sent, "unknown", MISSING);
    }
    const peerReview = readTicket(sequence.step({ kind: "broadcast" }, MISSING));
    sequence.step(readFailed(peerReview, MISSING), MISSING);
    const retry = successFirst
      ? readTicket(sequence.step({ kind: "visible" }, MISSING))
      : sequence.read(MISSING);
    sequence.unauthorized(retry, MISSING);
    expect(sequence.notices).toBe(successFirst ? 1 : 0);
    expect(sequence.view.loggedOut).toBe(true);
  }
});

test("不可归属的复核撤销 GET 补播，但人工继续的完整成功独立通知", () => {
  for (const continued of [false, true]) {
    const sequence = new SessionSequence();
    const first = sequence.logout();
    const review = readTicket(sequence.logoutResult(first, "unknown"));
    sequence.step(readFailed(review, UNAVAILABLE), UNAVAILABLE);
    sequence.expectCovered();
    if (continued) {
      const retry = sequence.logout(true);
      expect(sequence.step({ kind: "continue_logout" })).toEqual([]);
      sequence.logoutResult(retry, "succeeded", MISSING);
      sequence.logoutResult(retry, "succeeded", MISSING);
      expect(sequence.notices).toBe(1);
    } else {
      sequence.unauthorized(sequence.read(MISSING), MISSING);
      expect(sequence.notices).toBe(0);
    }
    expect(sequence.view.loggedOut).toBe(true);
  }
});

test("暂时见到另一代仍为 A 去重，权威确认 B 后 A 的结束通知可以重新结清", () => {
  const sequence = new SessionSequence();
  sequence.unauthorized(sequence.read());
  expect(sequence.notices).toBe(1);
  sequence.step({ kind: "visible" }, B.cookie);
  const same = readTicket(sequence.step({ kind: "visible" }));
  sequence.step(readOk(A, same));
  sequence.unauthorized(sequence.read());
  expect(sequence.notices).toBe(1);

  const replacement = readTicket(sequence.step({ kind: "visible" }, B.cookie));
  sequence.step(readOk(B, replacement), B.cookie);
  expect(sequence.view.canWrite).toBe(true);
  const retire = readTicket(sequence.step({ kind: "visible" }));
  sequence.step(readOk(A, retire));
  expect(sequence.view.loggedOut).toBe(true);
  const newA = readTicket(sequence.step({ kind: "visible" }));
  sequence.step(readOk(A, newA));
  sequence.unauthorized(sequence.read());
  expect(sequence.notices).toBe(2);
});

test("对端成功与本页可信 401 合计一次，普通接收方和本地退出未确认方均不回播", () => {
  for (const hasLocalExit of [false, true]) {
    for (const replacedRead of [false, true]) {
      const sender = new SessionSequence();
      const receiver = new SessionSequence();
      if (hasLocalExit) {
        const ticket = receiver.logout();
        receiver.logoutResult(ticket, "unknown", MISSING);
      }
      sender.logoutResult(sender.logout(), "succeeded", MISSING);
      expect(sender.notices).toBe(1);
      const initial = readTicket(receiver.step({ kind: "broadcast" }, MISSING));
      receiver.step(readFailed(initial, MISSING), MISSING);
      if (replacedRead) receiver.step({ kind: "visible" }, B.cookie);
      receiver.unauthorized(receiver.read(MISSING), MISSING);
      expect(receiver.view.loggedOut).toBe(true);
      expect(receiver.notices).toBe(0);
      expect(sender.notices + receiver.notices).toBe(1);
    }
  }
});
