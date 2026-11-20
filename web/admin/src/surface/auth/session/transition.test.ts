// Admin 会话状态机的回执代际、广播、退出与限流序列。

import { expect, test } from "bun:test";

import { stateView } from "./index";
import {
  apply,
  armedTimer,
  authenticated,
  countNotices,
  issuedTicket,
  loggedOut,
  loginAfterOldSessionExpires,
  loginOk,
  loginTimeout,
  logoutTicket,
  NOW,
  projectionAfter,
  readOk,
  readTicket,
  sessionFixture,
  startRenewal,
} from "./session-fixture";
import { startLogin, step } from "./transition";

test("login_receipt_after_broadcast_reads_the_current_cookie_before_opening_writes：广播后的登录回执先重读当前 Cookie 再开放写入", () => {
  const next = sessionFixture("new-cookie", 60);
  let state = loggedOut();
  const login = startLogin(state);
  if (login === null) throw new Error("应开始登录");
  state = login.state;
  expect(stateView(state).loginPending).toBe(true);
  expect(startLogin(state)).toBeNull();
  for (const event of [
    { kind: "broadcast" },
    { kind: "visible" },
    { kind: "broadcast" },
    { kind: "check_requested" },
  ] as const) {
    const [following, effects] = step(state, event, { kind: "missing" }, NOW);
    expect(effects).toHaveLength(0);
    expect(stateView(following).session).toBeNull();
    expect(stateView(following).loggedOut).toBe(false);
    expect(stateView(following).canWrite).toBe(false);
    state = following;
  }

  const completed = {
    kind: "login_finished",
    ticket: login.ticket,
    outcome: loginOk(next),
  } as const;
  const [checking, effects] = apply(next, state, completed);
  const check = readTicket(effects);
  expect(effects).toHaveLength(1);
  expect(stateView(checking).canWrite).toBe(false);
  const [duplicate, duplicateEffects] = apply(next, checking, completed);
  expect(duplicateEffects).toHaveLength(0);
  const [confirmed] = apply(next, duplicate, readOk(next, check));
  expect(stateView(confirmed).canWrite).toBe(true);
  expect(stateView(confirmed).session).toBe(next.session);
});

test("login_completion_with_no_current_session_does_not_repost_or_open_writes：无当前会话的登录完成不重发也不开放写入", () => {
  let state = loggedOut();
  const login = startLogin(state);
  if (login === null) throw new Error("应开始登录");
  state = login.state;
  const [anonymous, effects] = step(
    state,
    { kind: "login_finished", ticket: login.ticket, outcome: loginTimeout() },
    { kind: "missing" },
    NOW,
  );
  expect(stateView(anonymous).loggedOut).toBe(true);
  expect(stateView(anonymous).loginPending).toBe(true);
  expect(stateView(anonymous).canWrite).toBe(false);
  expect(startLogin(anonymous)).toBeNull();
  const [settled, readEffects] = step(
    anonymous,
    {
      kind: "read_finished",
      ticket: readTicket(effects),
      cookie: { kind: "missing" },
      outcome: { kind: "unauthenticated" },
    },
    { kind: "missing" },
    NOW,
  );
  expect(readEffects).toHaveLength(0);
  expect(stateView(settled).loggedOut).toBe(true);
  expect(stateView(settled).loginPending).toBe(false);
  expect(stateView(settled).canWrite).toBe(false);
  expect(stateView(settled).session).toBeNull();
});

test("old_renewal_401_after_broadcast_does_not_remount_the_logging_in_form：广播后旧续期 401 不重挂登录表单", () => {
  const old = sessionFixture("old-cookie", 900);
  const [anonymous, renewal] = loginAfterOldSessionExpires(old);
  const [hidden] = apply(old, anonymous, { kind: "broadcast" });
  expect(stateView(hidden).loggedOut).toBe(false);

  const [renamed, effects] = apply(old, hidden, {
    kind: "renewal_finished",
    ticket: renewal,
    cookie: old.cookie,
    outcome: { kind: "unauthenticated" },
  });
  expect(effects).toHaveLength(0);
  expect(renamed.renewal).toBeNull();
  expect(stateView(renamed).loginPending).toBe(true);
  expect(stateView(renamed).loggedOut).toBe(false);
  expect(stateView(renamed).canWrite).toBe(false);
});

test("old_renewal_401_does_not_replace_the_new_cookie_login_get：旧续期 401 不替换新 Cookie 的登录 GET", () => {
  const old = sessionFixture("old-cookie", 900);
  const next = sessionFixture("new-cookie", 60);
  const [anonymous, renewal, login] = loginAfterOldSessionExpires(old);
  const [hidden] = apply(old, anonymous, { kind: "broadcast" });
  const [checking, effects] = apply(next, hidden, {
    kind: "login_finished",
    ticket: login,
    outcome: loginOk(next),
  });
  const check = readTicket(effects);
  const [ignored, lateEffects] = apply(next, checking, {
    kind: "renewal_finished",
    ticket: renewal,
    cookie: old.cookie,
    outcome: { kind: "unauthenticated" },
  });
  expect(lateEffects).toHaveLength(0);
  expect(ignored.renewal).toBeNull();
  expect(stateView(ignored).canWrite).toBe(false);
  expect(ignored.check?.request.ticket).toBe(check);
  const [confirmed] = apply(next, ignored, readOk(next, check));
  expect(stateView(confirmed).canWrite).toBe(true);
});

test("discarded_renewal_schedules_new_input_after_logout_read_confirms_first：被丢弃的续期在退出复核确认后重新安排新输入", () => {
  const current = sessionFixture("current-cookie", 60);
  const renewed = projectionAfter(current, 1, 600, NOW);
  const [renewing, renewal] = startRenewal(current, authenticated(current));
  const [loggingOut, logoutEffects] = apply(current, renewing, { kind: "logout_clicked" });
  const [limited, logoutReadEffects] = apply(current, loggingOut, {
    kind: "logout_finished",
    ticket: logoutTicket(logoutEffects),
    cookie: current.cookie,
    outcome: { kind: "rate_limited", error: "rate_limited" },
  });
  const [confirmed] = apply(current, limited, readOk(current, readTicket(logoutReadEffects)));
  const [input, inputEffects] = apply(
    current,
    confirmed,
    { kind: "input", onLogout: false },
    NOW + 1,
  );
  expect(inputEffects).toHaveLength(0);
  expect(input.activity.latestInput).toBe(NOW + 1);
  const [late, renewalEffects] = apply(
    current,
    input,
    {
      kind: "renewal_finished",
      ticket: renewal,
      cookie: current.cookie,
      outcome: { kind: "succeeded", session: renewed.session },
    },
    NOW + 2,
  );
  expect(stateView(late).session).toBe(current.session);
  const [timer, delay] = armedTimer(renewalEffects, "renewal");
  const [, dueEffects] = apply(
    current,
    late,
    { kind: "timer_elapsed", timer: "renewal", ticket: timer },
    NOW + 2 + delay,
  );
  expect(dueEffects).toHaveLength(1);
  const renewEffect = dueEffects[0];
  if (renewEffect?.kind !== "renew") {
    throw new Error("应只派发一次续期");
  }
  expect(renewEffect.ticket).not.toBe(renewal);
  expect(renewEffect.session).toBe(current.session);
});

test("late_logout_cookie_removal_keeps_new_login_closed_after_broadcast：迟到的注销清 Cookie 后广播仍让新登录关闭", () => {
  const old = sessionFixture("old-cookie", 900);
  const next = sessionFixture("new-cookie", 60);
  const [tabA, logoutEffects] = apply(old, authenticated(old), { kind: "logout_clicked" });
  let tabB = loggedOut();
  const login = startLogin(tabB);
  if (login === null) throw new Error("应开始登录");
  tabB = login.state;
  const [checking, effects] = apply(next, tabB, {
    kind: "login_finished",
    ticket: login.ticket,
    outcome: loginOk(next),
  });
  const [confirmed] = apply(next, checking, readOk(next, readTicket(effects)));
  expect(stateView(confirmed).canWrite).toBe(true);

  // 浏览器在收到 DELETE 响应时已清除同名 Cookie，外壳只能按当前 Cookie 复核。
  const [, endedEffects] = step(
    tabA,
    {
      kind: "logout_finished",
      ticket: logoutTicket(logoutEffects),
      cookie: old.cookie,
      outcome: { kind: "succeeded" },
    },
    { kind: "missing" },
    NOW,
  );
  expect(countNotices(endedEffects)).toBe(1);
  const [hidden, broadcastEffects] = step(
    confirmed,
    { kind: "broadcast" },
    { kind: "missing" },
    NOW,
  );
  expect(stateView(hidden).session).toBeNull();
  expect(stateView(hidden).canWrite).toBe(false);
  const [anonymous] = step(
    hidden,
    {
      kind: "read_finished",
      ticket: readTicket(broadcastEffects),
      cookie: { kind: "missing" },
      outcome: { kind: "unauthenticated" },
    },
    { kind: "missing" },
    NOW,
  );
  expect(stateView(anonymous).loggedOut).toBe(true);
  expect(stateView(anonymous).canWrite).toBe(false);
});

test("late_old_401_does_not_end_another_tabs_new_login：迟到的旧 401 不结束另一标签页的新登录", () => {
  const old = sessionFixture("old-cookie", 900);
  const next = sessionFixture("new-cookie", 60);
  const [tabA, effects] = apply(old, authenticated(old), { kind: "logout_clicked" });
  const logout = logoutTicket(effects);
  const [pending, readEffects] = apply(old, tabA, {
    kind: "logout_finished",
    ticket: logout,
    cookie: old.cookie,
    outcome: { kind: "unknown", error: "request_timeout" },
  });
  const lateCheck = readTicket(readEffects);

  let tabB = loggedOut();
  const login = startLogin(tabB);
  if (login === null) throw new Error("应开始登录");
  tabB = login.state;
  const [checking, loginEffects] = apply(next, tabB, {
    kind: "login_finished",
    ticket: login.ticket,
    outcome: loginOk(next),
  });
  const [confirmed] = apply(next, checking, readOk(next, readTicket(loginEffects)));
  expect(stateView(confirmed).canWrite).toBe(true);

  const [verified, lateEffects] = apply(next, pending, {
    kind: "read_finished",
    ticket: lateCheck,
    cookie: old.cookie,
    outcome: { kind: "unauthenticated" },
  });
  expect(countNotices(lateEffects)).toBe(0);
  expect(stateView(verified).session).toBeNull();
  expect(stateView(verified).canWrite).toBe(false);
  expect(stateView(confirmed).canWrite).toBe(true);
  expect(stateView(confirmed).session).toBe(next.session);
  const [restored] = apply(next, verified, readOk(next, readTicket(lateEffects)));
  expect(stateView(restored).canWrite).toBe(true);
  expect(stateView(restored).session).toBe(next.session);

  // 固定通知可能在新登录后才送达。接收方须先隐藏，再凭当前代际恢复一棵新宿主。
  const epoch = stateView(confirmed).epoch;
  const [hidden, broadcastEffects] = apply(next, confirmed, { kind: "broadcast" });
  expect(stateView(hidden).session).toBeNull();
  expect(stateView(hidden).canWrite).toBe(false);
  const [reconfirmed] = apply(next, hidden, readOk(next, readTicket(broadcastEffects)));
  expect(stateView(reconfirmed).canWrite).toBe(true);
  expect(stateView(reconfirmed).epoch).toBeGreaterThan(epoch);
  expect(stateView(reconfirmed).session).toBe(next.session);
});

test("broadcast_then_old_get_success_cannot_interrupt_delete_success：广播后旧 GET 成功不能打断注销成功", () => {
  const current = sessionFixture("current-cookie", 900);
  const [checking, checkEffects] = apply(current, authenticated(current), { kind: "visible" });
  const oldCheck = readTicket(checkEffects);
  expect(stateView(checking).canWrite).toBe(true);
  const [loggingOut, logoutEffects] = apply(current, checking, { kind: "logout_clicked" });
  const logout = logoutTicket(logoutEffects);
  expect(logoutEffects.some((effect) => effect.kind === "cancel_read")).toBe(true);
  const [hidden, broadcastEffects] = apply(current, loggingOut, { kind: "broadcast" });
  expect(stateView(hidden).loggingOut).toBe(true);
  expect(stateView(hidden).canWrite).toBe(false);
  expect(stateView(hidden).session).toBeNull();
  expect(broadcastEffects).toHaveLength(0);

  const [afterOldRead, oldEffects] = apply(current, hidden, readOk(current, oldCheck));
  expect(stateView(afterOldRead).loggingOut).toBe(true);
  expect(stateView(afterOldRead).canWrite).toBe(false);
  expect(oldEffects).toHaveLength(0);

  const succeeded = {
    kind: "logout_finished",
    ticket: logout,
    cookie: current.cookie,
    outcome: { kind: "succeeded" },
  } as const;
  const [ended, endedEffects] = step(afterOldRead, succeeded, { kind: "missing" }, NOW);
  expect(countNotices(endedEffects)).toBe(1);
  expect(stateView(ended).canWrite).toBe(false);
  expect(stateView(ended).loggingOut).toBe(false);
  const check = readTicket(endedEffects);
  const [duplicate, duplicateEffects] = step(ended, succeeded, { kind: "missing" }, NOW);
  expect(duplicateEffects).toHaveLength(0);
  expect(stateView(duplicate).canWrite).toBe(false);
  const [anonymous, settledEffects] = step(
    duplicate,
    {
      kind: "read_finished",
      ticket: check,
      cookie: { kind: "missing" },
      outcome: { kind: "unauthenticated" },
    },
    { kind: "missing" },
    NOW,
  );
  expect(stateView(anonymous).loggedOut).toBe(true);
  expect(stateView(anonymous).session).toBeNull();
  expect(settledEffects).toHaveLength(0);
});

test("renewal_timeout_starts_one_read_when_routine_503_closed_writes：例行 503 关闭写入后，续期超时只发起一次读取", () => {
  const current = sessionFixture("current-cookie", 900);
  const [renewing, renewal] = startRenewal(current, authenticated(current));
  const [checking, checkEffects] = apply(current, renewing, { kind: "visible" });
  const [unavailable, unavailableEffects] = apply(current, checking, {
    kind: "read_finished",
    ticket: readTicket(checkEffects),
    cookie: current.cookie,
    outcome: { kind: "failed", error: "storage_unavailable" },
  });
  expect(stateView(unavailable).canWrite).toBe(false);
  expect(stateView(unavailable).session).not.toBeNull();
  expect(armedTimer(unavailableEffects, "check")[1]).toBe(30_000);

  const timedOut = {
    kind: "renewal_finished",
    ticket: renewal,
    cookie: current.cookie,
    outcome: { kind: "unknown" },
  } as const;
  const [recovering, recoveryEffects] = apply(current, unavailable, timedOut);
  const read = readTicket(recoveryEffects);
  expect(stateView(recovering).canWrite).toBe(false);
  expect(
    recoveryEffects.some((effect) => effect.kind === "cancel_timer" && effect.timer === "check"),
  ).toBe(true);
  expect(
    recoveryEffects.some(
      (effect) =>
        effect.kind === "renew" || (effect.kind === "arm_timer" && effect.timer === "renewal"),
    ),
  ).toBe(false);
  const [duplicate, duplicateEffects] = apply(current, recovering, timedOut);
  expect(duplicateEffects).toHaveLength(0);
  const [confirmed, confirmedEffects] = apply(current, duplicate, readOk(current, read));
  expect(stateView(confirmed).canWrite).toBe(true);
  expect(
    confirmedEffects.some(
      (effect) =>
        effect.kind === "renew" || (effect.kind === "arm_timer" && effect.timer === "renewal"),
    ),
  ).toBe(false);
});

test("rate_limit_preserves_input_recorded_after_post_until_retry_after：限流保留 POST 之后的输入直到重试时间", () => {
  const current = sessionFixture("current-cookie", 60);
  const [renewing, renewal] = startRenewal(current, authenticated(current));
  const [input, inputEffects] = apply(
    current,
    renewing,
    { kind: "input", onLogout: false },
    NOW + 10,
  );
  expect(inputEffects).toHaveLength(0);

  const [limited, effects] = apply(
    current,
    input,
    {
      kind: "renewal_finished",
      ticket: renewal,
      cookie: current.cookie,
      outcome: { kind: "rate_limited", retryAfterMillis: 10_000 },
    },
    NOW + 20,
  );
  const [timer, delay] = armedTimer(effects, "renewal");
  expect(delay).toBe(10_000);
  expect(limited.activity.latestInput).toBe(NOW + 10);

  const dueAt = NOW + 20 + delay;
  const [due, dueEffects] = apply(
    current,
    limited,
    {
      kind: "timer_elapsed",
      timer: "renewal",
      ticket: timer,
    },
    dueAt,
  );
  const next = issuedTicket(dueEffects, (effect) =>
    effect.kind === "renew" ? effect.ticket : null,
  );
  expect(next).not.toBe(renewal);
  expect(due.activity.latestInput).toBeNull();
  const [settled, settledEffects] = apply(
    current,
    due,
    {
      kind: "renewal_finished",
      ticket: next,
      cookie: current.cookie,
      outcome: { kind: "rate_limited", retryAfterMillis: 10_000 },
    },
    dueAt + 1,
  );
  expect(settled.renewalTimer).toBeNull();
  expect(settledEffects).toHaveLength(0);
});

test("rate_limit_drops_fresh_input_if_retry_cannot_reach_deadline：重试赶不上截止时限流丢弃新输入", () => {
  const current = sessionFixture("current-cookie", 60);
  const [renewing, renewal] = startRenewal(current, authenticated(current));
  const [input] = apply(current, renewing, { kind: "input", onLogout: false }, NOW + 10);
  const [limited, effects] = apply(
    current,
    input,
    {
      kind: "renewal_finished",
      ticket: renewal,
      cookie: current.cookie,
      outcome: { kind: "rate_limited", retryAfterMillis: 58_980 },
    },
    NOW + 20,
  );
  expect(effects).toHaveLength(0);
  expect(limited.activity.latestInput).toBeNull();
  expect(limited.checkTimer).not.toBeNull();
});

test("rate_limit_keeps_new_input_for_a_ninety_second_backoff_before_expiration：截止前的九十秒退避保留新输入", () => {
  const current = sessionFixture("current-cookie", 600);
  const [renewing, renewal] = startRenewal(current, authenticated(current));
  const [input] = apply(current, renewing, { kind: "input", onLogout: false }, NOW + 10);
  const [limited, effects] = apply(
    current,
    input,
    {
      kind: "renewal_finished",
      ticket: renewal,
      cookie: current.cookie,
      outcome: { kind: "rate_limited", retryAfterMillis: 90_000 },
    },
    NOW + 20,
  );
  const [timer, delay] = armedTimer(effects, "renewal");
  expect(delay).toBe(90_000);
  const [due, dueEffects] = apply(
    current,
    limited,
    {
      kind: "timer_elapsed",
      timer: "renewal",
      ticket: timer,
    },
    NOW + 20 + delay,
  );
  const next = issuedTicket(dueEffects, (effect) =>
    effect.kind === "renew" ? effect.ticket : null,
  );
  expect(next).not.toBe(renewal);
  expect(due.activity.latestInput).toBeNull();
});

test("logout_gesture_never_renews_and_logout_429_stays_read_only_until_checked：退出手势不续期，退出 429 在复核前保持只读", () => {
  const current = sessionFixture("current-cookie", 900);
  const [gesture, gestureEffects] = apply(current, authenticated(current), {
    kind: "input",
    onLogout: true,
  });
  expect(gestureEffects).toHaveLength(0);
  expect(gesture.activity.latestInput).toBeNull();
  const [input, inputEffects] = apply(
    current,
    gesture,
    { kind: "input", onLogout: false },
    NOW + 1,
  );
  const [timer, delay] = armedTimer(inputEffects, "renewal");
  expect(delay).toBe(0);
  const [loggingOut, logoutEffects] = apply(current, input, { kind: "logout_clicked" }, NOW + 1);
  expect(
    logoutEffects.some((effect) => effect.kind === "cancel_timer" && effect.timer === "renewal"),
  ).toBe(true);
  const logout = logoutTicket(logoutEffects);
  const [afterTimer, timerEffects] = apply(
    current,
    loggingOut,
    {
      kind: "timer_elapsed",
      timer: "renewal",
      ticket: timer,
    },
    NOW + 1,
  );
  expect(timerEffects).toHaveLength(0);
  const [afterInput, afterInputEffects] = apply(
    current,
    afterTimer,
    {
      kind: "input",
      onLogout: false,
    },
    NOW + 2,
  );
  expect(afterInputEffects).toHaveLength(0);
  expect(stateView(afterInput).canWrite).toBe(false);

  const [limited, reviewEffects] = apply(
    current,
    afterInput,
    {
      kind: "logout_finished",
      ticket: logout,
      cookie: current.cookie,
      outcome: { kind: "rate_limited", error: "rate_limited" },
    },
    NOW + 3,
  );
  expect(stateView(limited).canWrite).toBe(false);
  expect(stateView(limited).loggingOut).toBe(false);
  expect(limited.check?.reason).toBe("after_logout");
  const [reviewed] = apply(current, limited, readOk(current, readTicket(reviewEffects)), NOW + 4);
  expect(stateView(reviewed).canWrite).toBe(true);
  expect(stateView(reviewed).logoutError).toBe("rate_limited");
});

test("stale_successful_logout_only_checks_the_replacement_cookie：迟到注销成功只复核替换后的 Cookie", () => {
  const previous = sessionFixture("old-cookie", 900);
  const current = sessionFixture("new-cookie", 900);
  const [loggingOut, logoutEffects] = apply(previous, authenticated(previous), {
    kind: "logout_clicked",
  });
  const [stale, effects] = apply(current, loggingOut, {
    kind: "logout_finished",
    ticket: logoutTicket(logoutEffects),
    cookie: previous.cookie,
    outcome: { kind: "succeeded" },
  });
  expect(countNotices(effects)).toBe(0);
  expect(stateView(stale).session).toBeNull();
  expect(stateView(stale).canWrite).toBe(false);
  const [confirmed] = apply(current, stale, readOk(current, readTicket(effects)));
  expect(stateView(confirmed).session).toBe(current.session);
});

test("request_dispatched_after_cookie_change_only_rereads_current_cookie：Cookie 变化后派发的请求只重读当前 Cookie", () => {
  const previous = sessionFixture("old-cookie", 900);
  const current = sessionFixture("new-cookie", 900);
  const [checking, checkEffects] = apply(previous, authenticated(previous), { kind: "visible" });
  const [stale, effects] = apply(current, checking, readOk(current, readTicket(checkEffects)));
  expect(stateView(stale).session).toBeNull();
  expect(stateView(stale).canWrite).toBe(false);
  expect(countNotices(effects)).toBe(0);
  const [confirmed] = apply(current, stale, readOk(current, readTicket(effects)));
  expect(stateView(confirmed).session).toBe(current.session);
});
