// 会话归约期间的可变草稿与提交边界。
// 状态机只在副本上迁移，返回给调用方的旧快照不被就地修改。

import type {
  Activity,
  Check,
  Cookie,
  Effect,
  Phase,
  Renewal,
  State,
  Ticket,
  Verified,
} from "./index";

/** 单次事件携带的观测值与顺序执行的副作用队列。 */
export interface Actions {
  readonly cookie: Cookie;
  readonly now: number;
  readonly effects: Effect[];
}

/** 归约期间可直接改写的状态副本；提交时整体替换。 */
export interface Draft {
  phase: Phase;
  check: Check | null;
  bufferedRenewal: Verified | null;
  renewal: Renewal | null;
  login: Ticket | null;
  loginError: string | null;
  activity: Activity;
  checkTimer: Ticket | null;
  renewalTimer: Ticket | null;
  nextTicket: Ticket;
  viewEpoch: number;
  logoutError: string | null;
}

/** 复制状态的可变字段；嵌套的活动与在途续期对象也单独复制，避免写入影响旧快照。 */
export function stateDraft(state: State): Draft {
  return {
    phase: state.phase,
    check: state.check,
    bufferedRenewal: state.bufferedRenewal,
    renewal:
      state.renewal === null
        ? null
        : { request: state.renewal.request, source: state.renewal.source },
    login: state.login,
    loginError: state.loginError,
    activity: { ...state.activity },
    checkTimer: state.checkTimer,
    renewalTimer: state.renewalTimer,
    nextTicket: state.nextTicket,
    viewEpoch: state.viewEpoch,
    logoutError: state.logoutError,
  };
}

/** 用草稿覆盖归约涉及的字段；未识别的字段原样保留。 */
export function commitDraft(state: State, draft: Draft): State {
  return {
    ...state,
    phase: draft.phase,
    check: draft.check,
    bufferedRenewal: draft.bufferedRenewal,
    renewal: draft.renewal,
    login: draft.login,
    loginError: draft.loginError,
    activity: draft.activity,
    checkTimer: draft.checkTimer,
    renewalTimer: draft.renewalTimer,
    nextTicket: draft.nextTicket,
    viewEpoch: draft.viewEpoch,
    logoutError: draft.logoutError,
  };
}
