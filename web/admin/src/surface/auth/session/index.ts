// Admin 会话的纯状态机。已认证才允许写入，例行 GET 在途仍属于已认证状态。
// 退出请求独立持有回执；收到广播立即卸载敏感页面，退出期间只登记事后复核。
// 退出成功只通知一次。旧 Cookie 代际的回执只能要求重读当前 Cookie。
// 同一代际的截止只向后更新，截止与单调计时基准来自同一份服务端投影。
// 续期结果未知只做一次有界的只读复核。限流后仅保留 POST 发出之后的输入，
// 在退避下界与会话截止之间重新安排，等待期间不因输入时效丢弃。
// 确认会话失效也立即撤下敏感页面。

import { type Result, unreachable } from "@auction/web-shared/result";

import type { AdminSession } from "../../../api/session";
import type { RenewalTiming } from "../activity";

/** 可读的 CSRF Cookie 仅用于本地判断会话代际。 */
export type Cookie =
  | { readonly kind: "unavailable" }
  | { readonly kind: "missing" }
  | { readonly kind: "present"; readonly value: string };

/** 从会话投影得到当前可读的 CSRF 代际。 */
export function cookieFromSession(session: AdminSession): Cookie {
  return { kind: "present", value: session.csrfToken };
}

/** 两份 Cookie 是否表示同一代际。 */
export function cookiesEqual(left: Cookie, right: Cookie): boolean {
  if (left.kind !== right.kind) {
    return false;
  }
  if (left.kind === "present" && right.kind === "present") {
    return left.value === right.value;
  }
  return true;
}

/** 两个可读代际不相同时才算冲突；任一侧不可读时不推断代际变化。 */
export function cookieConflictsWith(left: Cookie, right: Cookie): boolean {
  return left.kind !== "unavailable" && right.kind !== "unavailable" && !cookiesEqual(left, right);
}

/** 只有可读且相同的 `Present` 代际才算一致。 */
export function cookieAgreesWith(left: Cookie, right: Cookie): boolean {
  return left.kind === "present" && cookiesEqual(left, right);
}

/** 请求与定时器的标识只在本外壳内使用。 */
export type Ticket = number;

/** 一次派发时携带的请求票与 Cookie 代际。 */
export interface Request {
  readonly ticket: Ticket;
  readonly cookie: Cookie;
}

/** 判断回执是否属于旧代际；成功 DELETE 清空 Cookie 时另按 `clearedByLogout` 放宽。 */
export function requestIsStale(
  request: Request,
  sent: Cookie,
  observed: Cookie,
  clearedByLogout: boolean,
): boolean {
  // 成功 DELETE 的 Set-Cookie 会清空代际。此时仍须排除派发期间换了实际请求 Cookie。
  return (
    cookieConflictsWith(request.cookie, sent) ||
    (!clearedByLogout &&
      (cookieConflictsWith(sent, observed) || cookieConflictsWith(request.cookie, observed)))
  );
}

/** 已确认的会话宿主与当时的 Cookie 代际。 */
export interface Verified {
  session: AdminSession;
  cookie: Cookie;
}

/** 用会话投影建立已确认宿主。 */
export function verifiedNew(session: AdminSession): Verified {
  return { cookie: cookieFromSession(session), session };
}

/** 会话复核的原因。 */
export type CheckReason =
  | "startup"
  | "routine"
  | "explicit"
  | "login"
  | "broadcast"
  | "cookie_changed"
  | "after_logout"
  | "after_renewal";

/** 在途的只读复核。 */
export interface Check {
  reason: CheckReason;
  request: Request;
}

/** 退出请求及其事后复核登记。 */
export interface Logout {
  request: Request;
  reviewAfter: boolean;
  wasWritable: boolean;
}

/** 用户可观察的会话阶段与当前认证宿主。 */
export type Phase =
  | { kind: "starting" }
  | { kind: "authenticated"; verified: Verified }
  | { kind: "checking"; verified: Verified | null }
  | { kind: "checking_login" }
  | { kind: "unavailable"; verified: Verified | null; error: string }
  | { kind: "logging_out"; verified: Verified | null; logout: Logout }
  | { kind: "logged_out" };

/** 返回当前阶段仍保留的已确认宿主。 */
export function phaseVerified(phase: Phase): Verified | null {
  switch (phase.kind) {
    case "authenticated":
      return phase.verified;
    case "checking":
    case "unavailable":
    case "logging_out":
      return phase.verified;
    case "checking_login":
    case "starting":
    case "logged_out":
      return null;
    default:
      return unreachable(phase);
  }
}

/** 人工活动与续期限流的计时状态。 */
export interface Activity {
  timing: RenewalTiming | null;
  latestInput: number | null;
  retryNotBefore: number;
}

/** 在途续期及其派发时的已确认宿主；Cookie 代际变化后 source 置空。 */
export interface Renewal {
  request: Request;
  source: Verified | null;
}

/** 单个标签页的会话状态及属于它的在途请求。 */
export interface State {
  phase: Phase;
  check: Check | null;
  /** 只收起敏感子树的广播复核；收到回执后一次 GET 确认暂存的续期投影。 */
  bufferedRenewal: Verified | null;
  renewal: Renewal | null;
  /** 已派发的登录 POST；收到广播后只收起表单，不重新派发。 */
  login: Ticket | null;
  /** 登录失败码留在内存，供复核为匿名后重新挂载的表单显示。 */
  loginError: string | null;
  activity: Activity;
  checkTimer: Ticket | null;
  renewalTimer: Ticket | null;
  nextTicket: Ticket;
  viewEpoch: number;
  logoutError: string | null;
}

/** 组件渲染与写门禁共用的快照。 */
export interface View {
  session: AdminSession | null;
  epoch: number;
  canWrite: boolean;
  loggingOut: boolean;
  loggedOut: boolean;
  loginPending: boolean;
  loginError: string | null;
  error: string | null;
  logoutError: string | null;
}

/** 初始阶段为启动中；其余字段按 Rust `Default` 清空。 */
export function initialPhase(): Phase {
  return { kind: "starting" };
}

/** 人工活动与续期限流的初始计时状态。 */
export function initialActivity(): Activity {
  return { timing: null, latestInput: null, retryNotBefore: 0 };
}

/** 会话状态机的初始状态。 */
export function initialState(): State {
  return {
    phase: initialPhase(),
    check: null,
    bufferedRenewal: null,
    renewal: null,
    login: null,
    loginError: null,
    activity: initialActivity(),
    checkTimer: null,
    renewalTimer: null,
    nextTicket: 0,
    viewEpoch: 0,
    logoutError: null,
  };
}

/** 渲染快照的初始值。 */
export function initialView(): View {
  return {
    session: null,
    epoch: 0,
    canWrite: false,
    loggingOut: false,
    loggedOut: false,
    loginPending: false,
    loginError: null,
    error: null,
    logoutError: null,
  };
}

/** 从状态机生成组件与写门禁共用的快照。 */
export function stateView(state: State): View {
  const verified = phaseVerified(state.phase);
  return {
    session: verified === null ? null : verified.session,
    epoch: state.viewEpoch,
    canWrite: state.phase.kind === "authenticated",
    loggingOut: state.phase.kind === "logging_out",
    loggedOut: state.phase.kind === "logged_out" || state.phase.kind === "checking_login",
    loginPending: state.login !== null || state.phase.kind === "checking_login",
    loginError: state.loginError,
    error: state.phase.kind === "unavailable" ? state.phase.error : null,
    logoutError: state.logoutError,
  };
}

/** 两份渲染快照是否值相等；相等时外壳不重复写入信号。 */
export function viewsEqual(left: View, right: View): boolean {
  return (
    sessionsEqual(left.session, right.session) &&
    left.epoch === right.epoch &&
    left.canWrite === right.canWrite &&
    left.loggingOut === right.loggingOut &&
    left.loggedOut === right.loggedOut &&
    left.loginPending === right.loginPending &&
    left.loginError === right.loginError &&
    left.error === right.error &&
    left.logoutError === right.logoutError
  );
}

function sessionsEqual(left: AdminSession | null, right: AdminSession | null): boolean {
  if (left === null || right === null) {
    return left === right;
  }
  return (
    left.actor === right.actor &&
    left.csrfToken === right.csrfToken &&
    left.expiresAt === right.expiresAt &&
    left.absoluteExpiresAt === right.absoluteExpiresAt &&
    left.serverTime === right.serverTime &&
    left.receivedAtMillis === right.receivedAtMillis
  );
}

/** 只读会话复核的服务端结果。 */
export type ReadOutcome =
  | { kind: "authenticated"; session: AdminSession }
  | { kind: "unauthenticated" }
  | { kind: "failed"; error: string };

/** 退出结果，明确拒绝与需要复核的结果使用不同迁移。 */
export type LogoutOutcome =
  | { kind: "succeeded" }
  | { kind: "rejected"; error: string }
  | { kind: "unknown"; error: string }
  | { kind: "rate_limited"; error: string };

/** 续期结果，退避只约束下一批人工输入。 */
export type RenewalOutcome =
  | { kind: "succeeded"; session: AdminSession }
  | { kind: "unauthenticated" }
  | { kind: "rate_limited"; retryAfterMillis: number | null }
  | { kind: "csrf_rejected" }
  | { kind: "unknown" }
  | { kind: "rejected" };

/** 例行复核与活动续期各持有一个可取消的定时器。 */
export type Timer = "check" | "renewal";

/** 浏览器输入和请求结果。回执携带实际派发时读取的 Cookie 代际。 */
export type Event =
  | { kind: "start" }
  | { kind: "login_finished"; ticket: Ticket; outcome: Result<AdminSession, string> }
  | { kind: "login_input_changed" }
  | { kind: "check_requested" }
  | { kind: "broadcast" }
  | { kind: "visible" }
  | { kind: "input"; onLogout: boolean }
  | { kind: "logout_clicked" }
  | { kind: "timer_elapsed"; timer: Timer; ticket: Ticket }
  | { kind: "read_finished"; ticket: Ticket; cookie: Cookie; outcome: ReadOutcome }
  | { kind: "logout_finished"; ticket: Ticket; cookie: Cookie; outcome: LogoutOutcome }
  | { kind: "renewal_finished"; ticket: Ticket; cookie: Cookie; outcome: RenewalOutcome };

/** 外壳按顺序执行的副作用；广播内容由浏览器适配固定。 */
export type Effect =
  | { kind: "read"; ticket: Ticket }
  | { kind: "cancel_read" }
  | { kind: "logout"; ticket: Ticket; session: AdminSession }
  | { kind: "renew"; ticket: Ticket; session: AdminSession }
  | { kind: "arm_timer"; timer: Timer; ticket: Ticket; delay: number }
  | { kind: "cancel_timer"; timer: Timer }
  | { kind: "broadcast_ended" };
