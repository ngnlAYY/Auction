// Admin 会话的纯状态机。只有已认证且身份和退出结果均无疑问时才允许写入。
// 身份未确认或退出未确认时遮蔽一次性 Key，恢复后须由操作员重新打开。
// 退出请求独立持有回执；收到广播立即卸载敏感页面，退出期间只登记事后复核。
// 过期与退出的证据独立于敏感宿主，结束通知按会话身份最多结清一次。
// 同一代际的截止只向后更新，截止与单调计时基准来自同一份服务端投影。
// 续期结果未知只做一次有界的只读复核。限流后仅保留 POST 发出之后的输入，
// 在退避下界与会话截止之间重新安排，等待期间不因输入时效丢弃。
// 确认会话失效也立即撤下敏感页面。

import { type Result, unreachable } from "@auction/web-shared/result";

import type { AdminSession } from "../../../api/session";
import type { RenewalTiming } from "../activity";
import type { EndNotices } from "./notices";

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

/** 登记、实际发送和收包均须有相同的可读代际，Cookie 可稳定缺失。 */
export function requestMatchesGeneration(
  request: Request,
  sent: Cookie,
  observed: Cookie,
): boolean {
  return (
    sent.kind !== "unavailable" &&
    cookiesEqual(request.cookie, sent) &&
    cookiesEqual(sent, observed)
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
  | "discover"
  | "login"
  | "broadcast"
  | "cookie_changed"
  | "after_logout"
  | "stay_after_logout"
  | "after_renewal";

/** 在途的只读复核。 */
export interface Check {
  reason: CheckReason;
  request: Request;
  origin: AdminSession | null;
  logoutTicket: Ticket | null;
}

/** 退出请求及其事后复核登记。 */
export interface Logout {
  request: Request;
  target: Verified;
  sawOtherGeneration: boolean;
  reviewAfter: boolean;
  wasWritable: boolean;
  continuing: boolean;
}

/** 独立于敏感宿主的本地退出意图，只有实际派发 DELETE 时才登记。 */
export interface LogoutIntent {
  readonly target: Verified;
  readonly lastSent: Ticket;
  readonly canInferFromRead: boolean;
}

/** 用户可观察的会话阶段与当前认证宿主。 */
export type Phase =
  | { kind: "starting" }
  | { kind: "authenticated"; verified: Verified }
  | { kind: "checking"; verified: Verified | null }
  | { kind: "checking_login" }
  | { kind: "unavailable"; verified: Verified | null; error: string }
  | { kind: "logging_out"; verified: Verified | null; logout: Logout }
  | { kind: "logout_unconfirmed"; verified: Verified | null }
  | { kind: "logged_out" };

/** 返回当前阶段仍保留的已确认宿主。 */
export function phaseVerified(phase: Phase): Verified | null {
  switch (phase.kind) {
    case "authenticated":
      return phase.verified;
    case "checking":
    case "unavailable":
    case "logging_out":
    case "logout_unconfirmed":
      return phase.verified;
    case "checking_login":
    case "starting":
    case "logged_out":
      return null;
    default:
      return unreachable(phase);
  }
}

/** 人工决定结清前保留退出锁，包括继续退出的 DELETE 在途时。 */
export function logoutUnconfirmed(phase: Phase): boolean {
  return (
    phase.kind === "logout_unconfirmed" || (phase.kind === "logging_out" && phase.logout.continuing)
  );
}

/** 一次性 Key 的展示围栏；遮蔽开始时递增版本，使旧打开动作失效。 */
export interface KeyVisibility {
  readonly hidden: boolean;
  readonly revision: number;
}

/** 人工活动与续期限流的计时状态。 */
export interface Activity {
  timing: RenewalTiming | null;
  latestInput: number | null;
  retryNotBefore: number;
}

/** 在途续期的原宿主；撤销投影后仍可核对当前票的正文与失效证据。 */
export interface Renewal {
  request: Request;
  origin: AdminSession;
  source: Verified | null;
  epoch: number;
}

/** 单个标签页的会话状态及属于它的在途请求。 */
export interface State {
  phase: Phase;
  check: Check | null;
  /** 只收起敏感子树的广播复核；收到回执后一次 GET 确认暂存的续期投影。 */
  bufferedRenewal: Verified | null;
  renewal: Renewal | null;
  /** 已派发的登录 POST，其回执和专属 GET 持有本次登录的归属。 */
  login: Ticket | null;
  /** 记录存在即仍欠通知。能否由 GET 推断结束由意图单独保存。 */
  logoutIntent: LogoutIntent | null;
  notices: EndNotices;
  identityUnconfirmed: boolean;
  sawOtherGeneration: boolean;
  keyVisibilityRevision: number;
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
  logoutUnconfirmed: boolean;
  logoutDecisionBusy: boolean;
  keyVisibility: KeyVisibility;
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
    logoutIntent: null,
    notices: { expiration: null, peer: null, settled: null },
    identityUnconfirmed: false,
    sawOtherGeneration: false,
    keyVisibilityRevision: 0,
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
    logoutUnconfirmed: false,
    logoutDecisionBusy: false,
    keyVisibility: { hidden: false, revision: 0 },
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
  const loggingOut = state.phase.kind === "logging_out";
  const unconfirmed = logoutUnconfirmed(state.phase);
  const keysHidden = state.identityUnconfirmed || unconfirmed;
  return {
    session: verified === null ? null : verified.session,
    epoch: state.viewEpoch,
    canWrite: state.phase.kind === "authenticated" && !keysHidden,
    loggingOut,
    logoutUnconfirmed: unconfirmed,
    logoutDecisionBusy: unconfirmed && (loggingOut || state.check !== null),
    keyVisibility: { hidden: keysHidden, revision: state.keyVisibilityRevision },
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
    left.logoutUnconfirmed === right.logoutUnconfirmed &&
    left.logoutDecisionBusy === right.logoutDecisionBusy &&
    left.keyVisibility.hidden === right.keyVisibility.hidden &&
    left.keyVisibility.revision === right.keyVisibility.revision &&
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
  | { kind: "write_review_identity"; epoch: number; session: AdminSession }
  | { kind: "external_unauthenticated"; epoch: number; session: AdminSession }
  | { kind: "broadcast" }
  | { kind: "visible" }
  | { kind: "input"; onLogout: boolean }
  | { kind: "logout_clicked" }
  | { kind: "continue_logout" }
  | { kind: "stay_after_logout" }
  | { kind: "timer_elapsed"; timer: Timer; ticket: Ticket }
  | { kind: "read_sending"; ticket: Ticket }
  | { kind: "read_finished"; ticket: Ticket; cookie: Cookie; outcome: ReadOutcome }
  | { kind: "logout_sending"; ticket: Ticket }
  | { kind: "logout_finished"; ticket: Ticket; cookie: Cookie; outcome: LogoutOutcome }
  | { kind: "logout_not_sent"; ticket: Ticket; cookie: Cookie; outcome: LogoutOutcome }
  | { kind: "renewal_sending"; ticket: Ticket }
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
