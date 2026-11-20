// F11 定点用例共用的可达会话与动作计数。

import { expect } from "bun:test";

import { type Cookie, type Effect, type Event, type State, stateView, type Ticket } from "./index";
import {
  apply,
  authenticated,
  countNotices,
  logoutTicket,
  NOW,
  readOk,
  readTicket,
  type SessionFixture,
  sessionFixture,
  startRenewal,
} from "./session-fixture";
import { step } from "./transition";

/** 会话 A 的已认证投影与 Cookie。 */
export const A = sessionFixture("cookie-a", 900);
/** 与 A 同账号、不同 token 的新代际。 */
export const B = sessionFixture("cookie-b", 900);
/** 已确认 Cookie 缺失。 */
export const MISSING = { kind: "missing" } as const;
/** 浏览器不可读取 Cookie。 */
export const UNAVAILABLE = { kind: "unavailable" } as const;

/** 从生产已认证状态开始，逐步记下外壳动作。 */
export class SessionSequence {
  state: State;
  notices = 0;
  deletes = 0;
  renewals = 0;

  constructor(state: State = authenticated(A)) {
    this.state = state;
  }

  get view() {
    return stateView(this.state);
  }

  step(event: Event, observed: Cookie = A.cookie): readonly Effect[] {
    const [state, effects] = step(this.state, event, observed, NOW);
    this.state = state;
    this.notices += countNotices(effects);
    this.deletes += effects.filter((effect) => effect.kind === "logout").length;
    this.renewals += effects.filter((effect) => effect.kind === "renew").length;
    return effects;
  }

  read(cookie: Cookie = A.cookie): Ticket {
    return readTicket(this.step({ kind: "check_requested" }, cookie));
  }

  unauthorized(ticket: Ticket, cookie: Cookie = A.cookie): readonly Effect[] {
    return this.step(
      { kind: "read_finished", ticket, cookie, outcome: { kind: "unauthenticated" } },
      cookie,
    );
  }

  logout(continuing = false): Ticket {
    const ticket = logoutTicket(
      this.step({ kind: continuing ? "continue_logout" : "logout_clicked" }),
    );
    this.step({ kind: "logout_sending", ticket });
    return ticket;
  }

  logoutResult(
    ticket: Ticket,
    outcome: "unknown" | "rate_limited" | "succeeded",
    cookie: Cookie = A.cookie,
  ): readonly Effect[] {
    const result = outcome === "succeeded" ? { kind: outcome } : { kind: outcome, error: outcome };
    return this.step(
      { kind: "logout_finished", ticket, cookie: A.cookie, outcome: result },
      cookie,
    );
  }

  renewal(): Ticket {
    const [state, ticket] = startRenewal(A, this.state);
    this.state = state;
    this.renewals += 1;
    return ticket;
  }

  expectCovered(epoch: number = this.view.epoch): void {
    expect(this.view.session?.csrfToken).toBe(A.session.csrfToken);
    expect(this.view.epoch).toBe(epoch);
    expect(this.view.keyVisibility.hidden).toBe(true);
    expect(this.view.canWrite).toBe(false);
  }
}

/** 权威读取换代先终止旧宿主，下一次发现读取才可采用新身份。 */
export function expectReplacementRequiresDiscovery(
  current: SessionFixture,
  state: State,
  ticket: Ticket,
) {
  const [ended, effects] = apply(current, state, readOk(current, ticket));
  expect(stateView(ended).session).toBeNull();
  expect(stateView(ended).epoch).toBeGreaterThan(stateView(state).epoch);
  expect(stateView(ended).loggedOut).toBe(true);
  expect(stateView(ended).canWrite).toBe(false);
  expect(countNotices(effects)).toBe(0);

  const [discovering, discoverEffects] = apply(current, ended, { kind: "visible" });
  const [confirmed] = apply(current, discovering, readOk(current, readTicket(discoverEffects)));
  expect(stateView(confirmed).canWrite).toBe(true);
  expect(stateView(confirmed).session).toBe(current.session);
}
