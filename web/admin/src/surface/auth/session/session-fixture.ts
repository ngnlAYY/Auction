// 会话状态机测试共用的服务端投影、Cookie 代际与副作用读取工具。

import { err, ok } from "@auction/web-shared/result";

import type { AdminSession } from "../../../api/session";
import { estimatedExpirationMillis, renewalTimingFromSession } from "../activity";
import {
  type Cookie,
  type Effect,
  type Event,
  initialState,
  type LogoutOutcome,
  phaseVerified,
  type ReadOutcome,
  type RenewalOutcome,
  type State,
  stateView,
  type Ticket,
  type Timer,
} from "./index";
import { startLogin, step } from "./transition";

/** Rust 测试里的单调时钟基准；各用例按需在此基础上推进。 */
export const NOW = 1_000;

const SECOND_NANOS = 1_000_000_000n;
const SERVER_TIME = BigInt(Date.UTC(2026, 10, 18, 0, 0, 0)) * 1_000_000n;

/** 一份会话投影及其可读 Cookie 代际。 */
export interface SessionFixture {
  readonly session: AdminSession;
  readonly cookie: Cookie;
}

/** 建立 2026-11-18T00:00:00Z 的会话；绝对截止固定为服务端时刻加八小时。 */
export function sessionFixture(token: string, idleSeconds: number): SessionFixture {
  const session: AdminSession = {
    actor: "operator",
    csrfToken: token,
    csrf_token: token,
    expiresAt: SERVER_TIME + BigInt(idleSeconds) * SECOND_NANOS,
    absoluteExpiresAt: SERVER_TIME + 8n * 3_600n * SECOND_NANOS,
    serverTime: SERVER_TIME,
    receivedAtMillis: 0,
  };
  return { session, cookie: { kind: "present", value: token } };
}

/** 在现有投影上推进服务端时刻并重算相对截止；接收基准可单独指定。 */
export function projectionAfter(
  fixture: SessionFixture,
  elapsedSeconds: number,
  remainingSeconds: number,
  receivedAt: number,
): SessionFixture {
  const serverTime = fixture.session.serverTime + BigInt(elapsedSeconds) * SECOND_NANOS;
  const session: AdminSession = {
    ...fixture.session,
    serverTime,
    expiresAt: serverTime + BigInt(remainingSeconds) * SECOND_NANOS,
    receivedAtMillis: receivedAt,
  };
  return { session, cookie: fixture.cookie };
}

/** 仅复制归约会改写的顶层字段；嵌套投影按引用保留，便于核对同一份会话对象。 */
export function cloneState(state: State): State {
  return { ...state, activity: { ...state.activity } };
}

/** 用固定 Cookie 观测推进一次事件。 */
export function apply(
  fixture: SessionFixture,
  state: State,
  event: Event,
  now: number = NOW,
): readonly [State, readonly Effect[]] {
  return step(state, event, fixture.cookie, now);
}

/** 构造一次成功的会话读取回执。 */
export function readOk(fixture: SessionFixture, ticket: Ticket): Event {
  return {
    kind: "read_finished",
    ticket,
    cookie: fixture.cookie,
    outcome: { kind: "authenticated", session: fixture.session },
  };
}

/** 启动并完成首次读取，得到已认证状态。 */
export function authenticated(fixture: SessionFixture): State {
  const [started, startEffects] = apply(fixture, initialState(), { kind: "start" });
  return apply(fixture, started, readOk(fixture, readTicket(startEffects)))[0];
}

/** 启动并完成一次匿名读取，得到已登出状态。 */
export function loggedOut(): State {
  const [started, startEffects] = step(initialState(), { kind: "start" }, { kind: "missing" }, NOW);
  return step(
    started,
    {
      kind: "read_finished",
      ticket: readTicket(startEffects),
      cookie: { kind: "missing" },
      outcome: { kind: "unauthenticated" },
    },
    { kind: "missing" },
    NOW,
  )[0];
}

/** 在指定时刻先记录输入并推进续期定时器，返回派发续期时的票号。 */
export function startRenewalAt(
  fixture: SessionFixture,
  state: State,
  now: number,
): readonly [State, Ticket] {
  const [inputState, inputEffects] = apply(fixture, state, { kind: "input", onLogout: false }, now);
  const [timer, delay] = armedTimer(inputEffects, "renewal");
  const [renewState, renewalEffects] = apply(
    fixture,
    inputState,
    { kind: "timer_elapsed", timer: "renewal", ticket: timer },
    now + delay,
  );
  return [
    renewState,
    issuedTicket(renewalEffects, (effect) => (effect.kind === "renew" ? effect.ticket : null)),
  ];
}

/** 在 NOW 上推进一次续期。 */
export function startRenewal(fixture: SessionFixture, state: State): readonly [State, Ticket] {
  return startRenewalAt(fixture, state, NOW);
}

/** 旧会话过期后进入匿名状态并开始一次新登录，返回旧续期票与新登录票。 */
export function loginAfterOldSessionExpires(old: SessionFixture): readonly [State, Ticket, Ticket] {
  const [renewing, renewal] = startRenewal(old, authenticated(old));
  const [checking, effects] = apply(old, renewing, { kind: "visible" });
  const [anonymous] = apply(old, checking, {
    kind: "read_finished",
    ticket: readTicket(effects),
    cookie: old.cookie,
    outcome: { kind: "unauthenticated" },
  });
  const started = startLogin(anonymous);
  if (started === null) {
    throw new Error("旧会话过期后应能开始登录");
  }
  return [started.state, renewal, started.ticket];
}

/** 从副作用中取唯一匹配的票号；数量不为一时直接失败。 */
export function issuedTicket(
  effects: readonly Effect[],
  gate: (effect: Effect) => Ticket | null,
): Ticket {
  const tickets = effects.map(gate).filter((ticket): ticket is Ticket => ticket !== null);
  const [ticket] = tickets;
  if (tickets.length !== 1 || ticket === undefined) {
    throw new Error(`副作用应恰好包含一个票号，实际 ${tickets.length}`);
  }
  return ticket;
}

/** 首次读取的票号。 */
export function readTicket(effects: readonly Effect[]): Ticket {
  return issuedTicket(effects, (effect) => (effect.kind === "read" ? effect.ticket : null));
}

/** 注销请求的票号。 */
export function logoutTicket(effects: readonly Effect[]): Ticket {
  return issuedTicket(effects, (effect) => (effect.kind === "logout" ? effect.ticket : null));
}

/** 指定定时器唯一一次激活的票号与延迟。 */
export function armedTimer(effects: readonly Effect[], timer: Timer): readonly [Ticket, number] {
  const timers = effects
    .filter(
      (effect): effect is Extract<Effect, { kind: "arm_timer" }> =>
        effect.kind === "arm_timer" && effect.timer === timer,
    )
    .map((effect) => [effect.ticket, effect.delay] as const);
  const [armed] = timers;
  if (timers.length !== 1 || armed === undefined) {
    throw new Error(`${timer} 定时器应恰好激活一次，实际 ${timers.length}`);
  }
  return armed;
}

/** 本次副作用里广播结束的次数。 */
export function countNotices(effects: readonly Effect[]): number {
  return effects.filter((effect) => effect.kind === "broadcast_ended").length;
}

/** 构造续期结果构造器；用例只关心结果种类时可用。 */
export function renewalSucceeded(fixture: SessionFixture): RenewalOutcome {
  return { kind: "succeeded", session: fixture.session };
}

/** 构造读取失败回执；错误码与 Rust 测试一致。 */
export function readFailed(ticket: Ticket, cookie: Cookie): Event {
  const outcome: ReadOutcome = { kind: "failed", error: "storage_unavailable" };
  return { kind: "read_finished", ticket, cookie, outcome };
}

/** 构造明确登出成功的回执。 */
export function logoutSucceeded(ticket: Ticket, cookie: Cookie): Event {
  const outcome: LogoutOutcome = { kind: "succeeded" };
  return { kind: "logout_finished", ticket, cookie, outcome };
}

/** 构造状态机的登录成功结果。 */
export function loginOk(fixture: SessionFixture) {
  return ok(fixture.session);
}

/** 构造状态机的登录超时结果。 */
export function loginTimeout() {
  return err<string>("request_timeout");
}

/** 当前快照里的预计到期时刻；供续期不变量核对。 */
export function expirationFor(state: State): number | null {
  const verified = phaseVerified(state.phase);
  return verified === null ? null : estimatedExpirationMillis(verified.session);
}

/** 供序列测试复用的当前续期节奏。 */
export function timingFor(fixture: SessionFixture) {
  return renewalTimingFromSession(fixture.session);
}

/** 供序列测试复用的快照读取。 */
export function viewOf(state: State) {
  return stateView(state);
}
