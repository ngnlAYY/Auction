// Cookie 变化的同源通知。跨页标记只存随机值，身份始终由权威读取确认。

import type { PublicSessionCurrentResponse as PublicSession } from "@auction/contracts/dto/public/account";
import { getItem, setItem } from "@auction/web-shared/platform/storage";
import { createUuidV4 } from "@auction/web-shared/platform/uuid";

/** 唤醒其他标签页的会话修订键。 */
export const PUBLIC_SESSION_CHANGE_STORAGE_KEY = "hv.public.session-revision";
const PUBLIC_SESSION_LOGOUT_REVISION_STORAGE_KEY = "hv.public.session-logout-revision";
const PUBLIC_SESSION_LOGOUT_UNKNOWN_REVISION_STORAGE_KEY =
  "hv.public.session-logout-unknown-revision";

/** 只有完整的成功回执确认退出；未知结果的修订只供一次对账判断。 */
export type LogoutReason = "logged_out" | "logout_unknown";

/** 同一 Cookie 读取观察到的修订。存储中的值均为随机标记。 */
export interface SessionRevisions {
  readonly confirmed: string | null;
  readonly unknown: string | null;
}

/** 普通复核可同步持久期，写回后的对账只读当前会话。 */
export type SessionRefreshReason =
  | "refresh"
  | "authentication_failed"
  | "cookie_changed"
  | LogoutReason;

/** 外壳会话重读入口。 */
export type PublicSessionCheckTrigger = (reason: SessionRefreshReason) => void;

/** 权威成功响应确认当前身份；带预期用户的 409 只否定这一次请求的预期归属。 */
export type SessionIdentity =
  | { readonly kind: "session"; readonly session: PublicSession | null }
  | { readonly kind: "user"; readonly userId: string }
  | { readonly kind: "expected-user-rejected"; readonly userId: string };

export type SessionIdentityEvidence = SessionIdentity & {
  readonly revisions: SessionRevisions;
  readonly cookieWritten: boolean;
};

/** 返回本次证据是否结束了先前确认的身份归属。 */
export type PublicSessionIdentityTrigger = (evidence: SessionIdentityEvidence) => boolean;

interface SessionCheckRegistration {
  trigger: PublicSessionCheckTrigger;
  identity: PublicSessionIdentityTrigger | undefined;
}

let registeredSessionCheckTrigger: SessionCheckRegistration | null = null;
let identityRevision = 0;

/** API 由无组件 owner 的任务调用，由常驻外壳登记重读入口并在卸载时注销。 */
export function registerPublicSessionCheckTrigger(
  trigger: PublicSessionCheckTrigger,
  identity?: PublicSessionIdentityTrigger,
): () => void {
  const registration: SessionCheckRegistration = { trigger, identity };
  registeredSessionCheckTrigger = registration;
  return () => {
    if (registeredSessionCheckTrigger === registration) {
      registeredSessionCheckTrigger = null;
    }
  };
}

/** 本页身份证据的修订，用于忽略晚于新证据交付的旧任务结果。 */
export function publicSessionIdentityRevision(): number {
  return identityRevision;
}

interface IdentityPublication {
  readonly revisions?: SessionRevisions;
  readonly cookieWritten?: boolean;
  readonly apply?: PublicSessionIdentityTrigger;
}

/** 完整会话、归属变化及确认写入让更早发出的锁外证据过期，即使仍是同账号。 */
export function publishPublicSessionIdentity(
  identity: SessionIdentity,
  options: IdentityPublication = {},
): number {
  const cookieWritten = options.cookieWritten ?? false;
  const apply = options.apply ?? registeredSessionCheckTrigger?.identity;
  const changed = apply?.({
    ...identity,
    revisions: options.revisions ?? publicSessionRevisions(),
    cookieWritten,
  });
  if (identity.kind === "session" || changed || cookieWritten) {
    identityRevision += 1;
  }
  return identityRevision;
}

/** 锁外请求只在发出时的归属和跨页修订仍有效时交付证据。 */
export function startPublicSessionIdentityObservation() {
  const identity = identityRevision;
  const revision = getItem(PUBLIC_SESSION_CHANGE_STORAGE_KEY);
  const logout = getItem(PUBLIC_SESSION_LOGOUT_REVISION_STORAGE_KEY);
  const registration = registeredSessionCheckTrigger;
  return (result: SessionIdentity, options?: IdentityPublication): number | null => {
    if (
      identity !== identityRevision ||
      registration !== registeredSessionCheckTrigger ||
      revision !== getItem(PUBLIC_SESSION_CHANGE_STORAGE_KEY) ||
      logout !== getItem(PUBLIC_SESSION_LOGOUT_REVISION_STORAGE_KEY)
    ) {
      return null;
    }
    return publishPublicSessionIdentity(result, options);
  };
}

/** 由当前页外壳按 Cookie 重读权威会话。 */
export function requestPublicSessionCheck(reason: SessionRefreshReason): void {
  registeredSessionCheckTrigger?.trigger(reason);
}

/** 通知本页和其他标签页按当前 Cookie 复核。 */
export function notifyPublicSessionChange(reason: SessionRefreshReason): void {
  setItem(PUBLIC_SESSION_CHANGE_STORAGE_KEY, createUuidV4());
  registeredSessionCheckTrigger?.trigger(reason === "refresh" ? "cookie_changed" : reason);
}

/** 在写锁内记录退出结果，排在后面的读取可先于跨页唤醒观察到该修订。 */
export function markPublicLogout(reason: LogoutReason): void {
  const key =
    reason === "logged_out"
      ? PUBLIC_SESSION_LOGOUT_REVISION_STORAGE_KEY
      : PUBLIC_SESSION_LOGOUT_UNKNOWN_REVISION_STORAGE_KEY;
  setItem(key, createUuidV4());
}

/** 在 Cookie 锁内取样，供外壳识别本浏览器的退出。 */
export function publicSessionRevisions(): SessionRevisions {
  return {
    confirmed: getItem(PUBLIC_SESSION_LOGOUT_REVISION_STORAGE_KEY),
    unknown: getItem(PUBLIC_SESSION_LOGOUT_UNKNOWN_REVISION_STORAGE_KEY),
  };
}
