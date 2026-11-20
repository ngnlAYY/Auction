// 会话阶段与认证宿主的所有权，含卸载时撤回的许可和任务归属。

import { unreachable } from "@auction/web-shared/result";
import { type AdminSession, adminSessionSameIdentity } from "../../../api/session";

import type { Actions, Draft } from "./draft";
import {
  cookieConflictsWith,
  initialActivity,
  type Phase,
  phaseVerified,
  type Verified,
} from "./index";
import { observeLogoutIdentity } from "./logout";
import { observeEndNoticeIdentity } from "./notices";
import { cancelRead, cancelTimer } from "./requests";

/** Authenticated 被清空后回到 Starting；其余宿主状态只移除当前认证实例。 */
export function takeVerified(draft: Draft): Verified | null {
  const phase = draft.phase;
  switch (phase.kind) {
    case "authenticated":
      draft.phase = { kind: "starting" };
      return phase.verified;
    case "checking":
    case "unavailable":
    case "logging_out":
    case "logout_unconfirmed":
      draft.phase = withVerified(phase, null);
      return phase.verified;
    case "checking_login":
    case "starting":
    case "logged_out":
      return null;
    default:
      return unreachable(phase);
  }
}

/** 对应源码的 `verified_mut`：退出阶段不向续期结果暴露宿主。 */
export function verifiedMut(phase: Phase): Verified | null {
  switch (phase.kind) {
    case "authenticated":
      return phase.verified;
    case "checking":
    case "unavailable":
      return phase.verified;
    case "checking_login":
    case "starting":
    case "logging_out":
    case "logout_unconfirmed":
    case "logged_out":
      return null;
    default:
      return unreachable(phase);
  }
}

/** 用新的认证实例替换当前阶段，其他阶段保持原值。 */
export function withVerified(phase: Phase, verified: Verified | null): Phase {
  switch (phase.kind) {
    case "authenticated":
      return verified === null ? { kind: "starting" } : { kind: "authenticated", verified };
    case "checking":
      return { kind: "checking", verified };
    case "unavailable":
      return { kind: "unavailable", verified, error: phase.error };
    case "logging_out":
      return { kind: "logging_out", verified, logout: phase.logout };
    case "logout_unconfirmed":
      return { kind: "logout_unconfirmed", verified };
    case "checking_login":
    case "starting":
    case "logged_out":
      return phase;
    default:
      return unreachable(phase);
  }
}

/** 撤下敏感页面；旧宿主的草稿、未决命令和弹窗随宿主失效。 */
export function hideSession(draft: Draft, actions: Actions): void {
  if (takeVerified(draft) !== null) {
    draft.viewEpoch += 1;
  }
  draft.activity = initialActivity();
  draft.logoutError = null;
  draft.identityUnconfirmed = false;
  draft.sawOtherGeneration = false;
  cancelTimer(draft, "renewal", actions);
}

/** 每次读取 Cookie 都先经过这里；即使事件或回执已经过时，当前观测仍有效。 */
export function observeIdentity(draft: Draft, actions: Actions): boolean {
  const cookie = actions.cookie;
  if (cookie.kind === "present") {
    observeLogoutIdentity(draft, (target) => cookieConflictsWith(target.cookie, cookie));
    observeEndNoticeIdentity(draft, (session) => session.csrfToken !== cookie.value);
  }
  // 浏览器已切换代际时，旧续期的投影既不能暂存，也不能被新宿主采用。
  if (
    draft.bufferedRenewal !== null &&
    cookieConflictsWith(draft.bufferedRenewal.cookie, actions.cookie)
  ) {
    draft.bufferedRenewal = null;
  }
  const renewal = draft.renewal;
  if (renewal?.source && cookieConflictsWith(renewal.source.cookie, actions.cookie)) {
    renewal.source = null;
  }
  const verified = phaseVerified(draft.phase);
  if (verified === null || !cookieConflictsWith(verified.cookie, actions.cookie)) {
    return false;
  }
  markIdentityUnconfirmed(draft, actions.cookie.kind === "present", actions);
  return true;
}

/** 只接受已核票、归属原宿主的有效正文。异代证据撤回展示和推断资格，不能授权。 */
export function observeSessionBody(
  draft: Draft,
  origin: AdminSession | null,
  session: AdminSession,
  actions: Actions,
): boolean {
  if (origin === null || adminSessionSameIdentity(origin, session)) {
    return false;
  }
  observeLogoutIdentity(draft, (target) => adminSessionSameIdentity(origin, target.session));
  observeEndNoticeIdentity(draft, (target) => adminSessionSameIdentity(origin, target));
  const verified = phaseVerified(draft.phase);
  if (verified !== null && adminSessionSameIdentity(verified.session, origin)) {
    markIdentityUnconfirmed(draft, true, actions);
  }
  return true;
}

/** 关写并遮蔽一次性 Key，旧续期结果不能恢复授权。 */
export function markIdentityUnconfirmed(draft: Draft, sawOther: boolean, actions: Actions): void {
  if (!draft.identityUnconfirmed) {
    draft.identityUnconfirmed = true;
    // 后续即便合并了渲染并重新确认 A，旧的打开动作也不能恢复明文。
    draft.keyVisibilityRevision += 1;
  }
  draft.sawOtherGeneration ||= sawOther;
  if (draft.phase.kind === "authenticated") {
    draft.phase = { kind: "checking", verified: draft.phase.verified };
  }
  retireRenewal(draft);
  draft.activity.latestInput = null;
  cancelTimer(draft, "renewal", actions);
}

/** 撤下认证宿主并结清退出流程。通知归属由独立账本判断。 */
export function endSession(draft: Draft, actions: Actions): void {
  hideSession(draft, actions);
  draft.logoutIntent = null;
  retireRenewal(draft);
  cancelRead(draft, actions);
  cancelTimer(draft, "check", actions);
  draft.phase = { kind: "logged_out" };
}

/** 撤销续期的投影资格，保留原票与 epoch 供失效证据核对。 */
export function retireRenewal(draft: Draft): void {
  draft.bufferedRenewal = null;
  if (draft.renewal !== null) {
    draft.renewal.source = null;
  }
}
