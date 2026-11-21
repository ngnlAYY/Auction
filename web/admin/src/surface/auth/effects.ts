// Admin 会话状态机的浏览器事件与副作用适配。
// 外壳负责启动首次读取、可见性与人工输入监听、跨标签页通知，以及请求与定时器的派发和取消。

import { sleep } from "@auction/web-shared/platform/timer";
import { createPageVisibility, documentIsVisible } from "@auction/web-shared/platform/visibility";
import { type Cell, createCell } from "@auction/web-shared/reactive/cell";
import { createTaskScope, type TaskHandle } from "@auction/web-shared/reactive/tasks";
import { unreachable } from "@auction/web-shared/result";
import { onCleanup, onSettled } from "solid-js";
import {
  type AdminApiFailure,
  adminApiFailureDefiniteRejection,
  adminApiFailureOutcomeUnknown,
} from "../../api/failure";
import {
  type AdminSessionHost,
  type AdminSessionWriteReview,
  fetchAdminSession,
  loginAdmin,
  logoutAdmin,
  registerAdminSessionHost,
  renewAdminSession,
} from "../../api/session";
import { trimRustWhitespace } from "../../api/session-time";
import { LOGOUT_BUTTON_CLASS } from "./activity";
import type { LoginSubmission } from "./login";
import {
  type Cookie,
  type Effect,
  initialState,
  initialView,
  type LogoutOutcome,
  phaseVerified,
  type ReadOutcome,
  type RenewalOutcome,
  type Event as SessionEvent,
  type State,
  stateView,
  type Ticket,
  type Timer,
  type View,
  viewsEqual,
} from "./session";
import { startLogin, step } from "./session/transition";

/** 认证外壳持有的会话快照与事件入口；`view` 是响应式读取，供渲染订阅。 */
export interface AdminSessionHandle {
  readonly view: Cell<View>;
  readonly dispatch: (event: SessionEvent) => void;
  readonly submitLogin: (submission: LoginSubmission) => void;
  readonly host: AdminSessionHost;
  readonly writeReview: (epoch: number) => AdminSessionWriteReview;
}

/**
 * 在认证外壳里挂载会话状态机；返回的快照与派发器只在当前宿主内使用。
 * 首次读取在渲染稳定后启动，监听与跨标签页通道随宿主卸载一起释放。
 */
export function useSession(): AdminSessionHandle {
  const view = createCell(initialView());
  const tasks = createTaskScope();
  let state: State = initialState();
  let read: TaskHandle | null = null;
  let checkTimer: TaskHandle | null = null;
  let renewalTimer: TaskHandle | null = null;
  let broadcast: BroadcastNotice | null = null;

  // 该宿主随认证外壳挂载；本外壳发出的请求在 401 时只重查自己，不落到更新一代的宿主。
  const registration = registerAdminSessionHost({
    check: () => accept({ kind: "check_requested" }),
    writesAllowed: () => view.peek().canWrite,
    confirmed: () => phaseVerified(state.phase)?.session ?? null,
    cookieGeneration: () => {
      const cookie = cookieGeneration();
      return cookie.kind === "present" ? cookie.value : null;
    },
  });
  const host = registration.host;
  onCleanup(registration.release);

  function timerSlot(timer: Timer): TaskHandle | null {
    return timer === "check" ? checkTimer : renewalTimer;
  }

  function setTimer(timer: Timer, handle: TaskHandle | null): void {
    if (timer === "check") {
      checkTimer = handle;
    } else {
      renewalTimer = handle;
    }
  }

  function cancel(handle: TaskHandle | null): void {
    handle?.cancel();
  }

  function cancelRead(): void {
    cancel(read);
    read = null;
  }

  function cancelTimer(timer: Timer): void {
    const handle = timerSlot(timer);
    setTimer(timer, null);
    cancel(handle);
  }

  function accept(
    event: SessionEvent,
    cookie: Cookie = event.kind === "input" ? { kind: "unavailable" } : cookieGeneration(),
  ): void {
    const [nextState, effects] = step(state, event, cookie, monotonicMillis());
    state = nextState;
    publish();
    for (const effect of effects) {
      execute(effect);
    }
  }

  /** 归约结果与上次快照值相等时不重复写入信号。 */
  function publish(): void {
    const nextView = stateView(state);
    if (!viewsEqual(view.peek(), nextView)) {
      view.set(nextView);
    }
  }

  function execute(effect: Effect): void {
    switch (effect.kind) {
      case "read":
        startRead(effect.ticket);
        break;
      case "cancel_read":
        cancelRead();
        break;
      case "logout":
        startLogout(effect.ticket);
        break;
      case "renew":
        startRenewal(effect.ticket);
        break;
      case "arm_timer":
        armTimer(effect.timer, effect.ticket, effect.delay);
        break;
      case "cancel_timer":
        cancelTimer(effect.timer);
        break;
      case "broadcast_ended":
        broadcast?.notify();
        break;
      default:
        unreachable(effect);
    }
  }

  function startRead(ticket: Ticket): void {
    const pending = state.check;
    if (pending?.request.ticket !== ticket) {
      return;
    }
    if (pending.reason === "discover" && (!documentIsVisible() || state.login !== null)) {
      state = { ...state, check: null };
      return;
    }
    read = tasks.spawn(async (context) => {
      const sent = cookieGeneration();
      // 复核等待网络前立即发布这次 Cookie 观测，撤回旧身份的写许可和 Key 展示。
      accept({ kind: "read_sending", ticket }, sent);
      if (!context.alive() || state.check?.request.ticket !== ticket) {
        return;
      }
      const result = await fetchAdminSession(host, context.signal);
      if (!context.alive()) {
        return;
      }
      const outcome: ReadOutcome = result.ok
        ? result.value === null
          ? { kind: "unauthenticated" }
          : { kind: "authenticated", session: result.value }
        : { kind: "failed", error: result.error };
      read = null;
      accept({ kind: "read_finished", ticket, cookie: sent, outcome });
    });
  }

  function startLogout(ticket: Ticket): void {
    tasks.spawn(async (context) => {
      const sent = cookieGeneration();
      accept({ kind: "logout_sending", ticket }, sent);
      if (
        !context.alive() ||
        state.phase.kind !== "logging_out" ||
        state.phase.logout.request.ticket !== ticket
      ) {
        return;
      }
      // 派发围栏核对了退出票与本宿主保留的原身份，读取该身份与发出 DELETE 之间没有等待。
      const result = await logoutAdmin(host, context.signal);
      if (!context.alive()) {
        return;
      }
      const dispatched =
        result.ok ||
        result.error.status !== null ||
        !adminApiFailureDefiniteRejection(result.error.code);
      const outcome: LogoutOutcome = result.ok
        ? { kind: "succeeded" }
        : logoutFailure(result.error);
      accept({
        kind: dispatched ? "logout_finished" : "logout_not_sent",
        ticket,
        cookie: sent,
        outcome,
      });
    });
  }

  function startRenewal(ticket: Ticket): void {
    // `spawn` 在任务体内先同步执行到首个 await，等同源码在同一轮启动 fetch。
    tasks.spawn(async (context) => {
      const sent = cookieGeneration();
      accept({ kind: "renewal_sending", ticket }, sent);
      if (!context.alive() || state.renewal?.request.ticket !== ticket) {
        return;
      }
      // 续期同样只读取本宿主已确认的会话与 CSRF，不接受旧 effect 携带的快照。
      const result = await renewAdminSession(host, context.signal);
      if (!context.alive()) {
        return;
      }
      const outcome: RenewalOutcome = result.ok
        ? { kind: "succeeded", session: result.value }
        : renewalFailure(result.error.failure, result.error.retryAfterMillis);
      accept({ kind: "renewal_finished", ticket, cookie: sent, outcome });
    });
  }

  /** 登录 POST 由外壳持有；表单卸载后回执仍落在当前状态机的登录票上。 */
  function startLoginRequest(loginTicket: Ticket, submission: LoginSubmission): void {
    tasks.spawn(async (context) => {
      const result = await loginAdmin(
        submission.host,
        submission.actor,
        submission.secret,
        context.signal,
      );
      if (!context.alive()) {
        return;
      }
      accept({ kind: "login_finished", ticket: loginTicket, outcome: result });
    });
  }

  function armTimer(timer: Timer, ticket: Ticket, delay: number): void {
    let handle: TaskHandle | null = null;
    handle = tasks.spawn(async (context) => {
      if (delay > 0) {
        if (!(await sleep(delay, context.signal))) {
          return;
        }
      } else {
        // Dioxus 的 spawn 不会在 effect 同一调用栈里重入状态机；让出一个微任务再派发。
        await Promise.resolve();
      }
      if (!context.alive()) {
        return;
      }
      if (handle !== null && timerSlot(timer) === handle) {
        setTimer(timer, null);
      }
      accept({ kind: "timer_elapsed", timer, ticket });
    });
    setTimer(timer, handle);
  }

  const dispatch = (event: SessionEvent): void => {
    accept(event);
  };

  const submitLogin = (submission: LoginSubmission): void => {
    const started = startLogin(state);
    if (started === null) {
      return;
    }
    state = started.state;
    publish();
    startLoginRequest(started.ticket, submission);
  };

  broadcast = openBroadcastNotice("hv-auction-admin-session", "session-ended", () => {
    dispatch({ kind: "broadcast" });
  });
  if (broadcast !== null) {
    onCleanup(() => {
      broadcast.close();
    });
  }

  const visibility = createPageVisibility();
  visibility.install(() => dispatch({ kind: "visible" }));
  onCleanup(() => {
    visibility.cleanup();
  });

  const input = installInputActivityListener(`.${LOGOUT_BUTTON_CLASS}`, (onLogout) => {
    dispatch({ kind: "input", onLogout });
  });
  if (input !== null) {
    onCleanup(() => {
      input.close();
    });
  }

  onSettled(() => {
    dispatch({ kind: "start" });
  });

  return {
    view,
    dispatch,
    submitLogin,
    host,
    writeReview: (epoch) => ({
      isCurrent: () => state.viewEpoch === epoch,
      observe: (session) => accept({ kind: "write_review_identity", epoch, session }),
      cookieGeneration: () => {
        const cookie = cookieGeneration();
        return cookie.kind === "present" ? cookie.value : null;
      },
      invalidate: (session) => accept({ kind: "external_unauthenticated", epoch, session }),
    }),
  };
}

interface BroadcastNotice {
  notify(): void;
  close(): void;
}

/** 同源标签页的固定消息通道；构造失败时外壳继续使用本地复核。 */
function openBroadcastNotice(
  name: string,
  message: string,
  onReceive: () => void,
): BroadcastNotice | null {
  if (typeof BroadcastChannel === "undefined") {
    return null;
  }
  let channel: BroadcastChannel;
  try {
    channel = new BroadcastChannel(name);
  } catch {
    return null;
  }
  channel.onmessage = (event: MessageEvent) => {
    if (typeof event.data === "string" && event.data === message) {
      onReceive();
    }
  };
  return {
    notify() {
      try {
        channel.postMessage(message);
      } catch {
        // 发送失败只影响其他标签页的通知，本页仍按本地状态继续复核。
      }
    },
    close() {
      channel.onmessage = null;
      channel.close();
    },
  };
}

interface InputActivityListener {
  close(): void;
}

const INPUT_EVENTS = ["pointerdown", "keydown", "wheel", "touchstart"] as const;

/** 捕获阶段监听窗口内的真实输入，并报告它是否来自注销按钮。 */
function installInputActivityListener(
  selector: string,
  onActivity: (onLogout: boolean) => void,
): InputActivityListener | null {
  if (typeof window === "undefined") {
    return null;
  }
  const callback = (event: globalThis.Event): void => {
    if (!event.isTrusted) {
      return;
    }
    let onLogout = false;
    for (const target of event.composedPath()) {
      if (!(target instanceof Element)) {
        continue;
      }
      try {
        if (target.matches(selector)) {
          onLogout = true;
          break;
        }
      } catch {
        // 选择器匹配失败按不匹配处理；其余路径节点仍可命中。
      }
    }
    onActivity(onLogout);
  };
  const options: AddEventListenerOptions = { passive: true, capture: true };
  const installed: (typeof INPUT_EVENTS)[number][] = [];
  for (const name of INPUT_EVENTS) {
    try {
      window.addEventListener(name, callback, options);
      installed.push(name);
    } catch {
      for (const added of installed) {
        window.removeEventListener(added, callback, true);
      }
      return null;
    }
  }
  return {
    close() {
      for (const name of INPUT_EVENTS) {
        window.removeEventListener(name, callback, true);
      }
    },
  };
}

/** 读取脚本可见的 CSRF Cookie；同名 Cookie 让代际无法确认。 */
function cookieGeneration(): Cookie {
  if (typeof document === "undefined") {
    return { kind: "unavailable" };
  }
  let cookies: string;
  try {
    cookies = document.cookie;
  } catch {
    return { kind: "unavailable" };
  }
  const values: string[] = [];
  for (const pair of cookies.split(";")) {
    const trimmed = trimRustWhitespace(pair);
    const separator = trimmed.indexOf("=");
    if (separator < 0 || trimmed.slice(0, separator) !== "auction_admin_csrf") {
      continue;
    }
    values.push(trimmed.slice(separator + 1));
  }
  if (values.length > 1) {
    return { kind: "unavailable" };
  }
  const value = values.at(0);
  if (value === undefined || value === "") {
    return { kind: "missing" };
  }
  return { kind: "present", value };
}

/** 注销失败只有在明确拒绝时才可结束本地退出，其余结果都要求重读会话。 */
function logoutFailure(error: AdminApiFailure): LogoutOutcome {
  if (error.status === 429 || error.code === "rate_limited") {
    return { kind: "rate_limited", error: error.code };
  }
  if (
    error.code === "csrf_invalid" ||
    error.status === 401 ||
    error.status === 408 ||
    (error.code !== "invalid_request" && adminApiFailureOutcomeUnknown(error))
  ) {
    return { kind: "unknown", error: error.code };
  }
  return { kind: "rejected", error: error.code };
}

/** 续期失败分类：到期、限流、代际拒绝、明确拒绝与未知结果分别迁移。 */
function renewalFailure(failure: AdminApiFailure, retryAfter: number | null): RenewalOutcome {
  if (failure.code === "unauthenticated" && failure.status === 401) {
    return { kind: "unauthenticated" };
  }
  if (failure.code === "rate_limited") {
    return { kind: "rate_limited", retryAfterMillis: retryAfter };
  }
  if (failure.code === "csrf_invalid") {
    return { kind: "csrf_rejected" };
  }
  if (failure.code === "invalid_request") {
    return { kind: "rejected" };
  }
  if (failure.status === 429) {
    return { kind: "rate_limited", retryAfterMillis: retryAfter };
  }
  if (failure.status === 401 || failure.status === 408 || adminApiFailureOutcomeUnknown(failure)) {
    return { kind: "unknown" };
  }
  return { kind: "rejected" };
}

/** 当前页面的单调毫秒；只用于计时，不参与服务端时刻比较。 */
function monotonicMillis(): number {
  return performance.now();
}
