// 会话结束通知账本。只驻留在外壳内存，随界面卸载的草稿与命令不参与归属判断。

import { type AdminSession, adminSessionSameIdentity } from "../../../api/session";
import type { Actions, Draft } from "./draft";
import { type Cookie, phaseVerified } from "./index";

interface EndEvidence {
  readonly session: AdminSession;
  readonly sawOtherGeneration: boolean;
}

/** 已归属的过期证据、待本地结果核对的对端通知，以及已结清的会话标识。 */
export interface EndNotices {
  expiration: EndEvidence | null;
  peer: EndEvidence | null;
  settled: bigint | null;
}

const FNV_PRIME = 0x100000001b3n;

function endNoticeId(session: AdminSession): bigint {
  // 64 位 FNV-1a 指纹仅用于通知去重。0xff 分隔 UTF-8 字段。
  const encoder = new TextEncoder();
  let hash = 0xcbf29ce484222325n;
  for (const field of [session.actor, session.csrfToken]) {
    for (const byte of encoder.encode(field)) {
      hash = BigInt.asUintN(64, (hash ^ BigInt(byte)) * FNV_PRIME);
    }
    hash = BigInt.asUintN(64, (hash ^ 0xffn) * FNV_PRIME);
  }
  return hash;
}

/** 为复核固定原有身份。界面已卸载时仍可核对账本和实发的退出意图。 */
export function noticeOrigin(draft: Draft): AdminSession | null {
  return (
    phaseVerified(draft.phase)?.session ??
    draft.logoutIntent?.target.session ??
    (draft.phase.kind === "logging_out" ? draft.phase.logout.target.session : null) ??
    draft.notices.expiration?.session ??
    draft.notices.peer?.session ??
    null
  );
}

/** 在撤下宿主前保存已经归属的续期 401；等待退出回执或后续权威读取来结清。 */
export function deferExpiration(draft: Draft, session: AdminSession): void {
  // 同一身份的异代证据是累积的；后到的 401 不能把已经看到的另一代降级掉。
  const previous = draft.notices.expiration;
  const preserved =
    previous !== null && adminSessionSameIdentity(previous.session, session)
      ? previous.sawOtherGeneration
      : false;
  draft.notices.expiration = {
    session,
    sawOtherGeneration: preserved || draft.sawOtherGeneration,
  };
}

/** 固定对端通知到原有身份；同一身份的可信本地结论负责结清。 */
export function rememberPeerEnd(draft: Draft): void {
  const session = noticeOrigin(draft);
  if (session !== null && draft.notices.peer === null) {
    draft.notices.peer = { session, sawOtherGeneration: draft.sawOtherGeneration };
  }
}

/** 另一代 Cookie 或正文使缺失 Cookie 的 401 无法归属原有身份。 */
export function observeEndNoticeIdentity(
  draft: Draft,
  differs: (session: AdminSession) => boolean,
): void {
  for (const kind of ["expiration", "peer"] as const) {
    const evidence = draft.notices[kind];
    if (evidence !== null && !evidence.sawOtherGeneration && differs(evidence.session)) {
      draft.notices[kind] = { ...evidence, sawOtherGeneration: true };
    }
  }
}

/** Cookie 明确属于原会话，或稳定缺失且未观察到异代时，才能归属自然过期。 */
export function cookieCanEndSession(
  session: AdminSession,
  cookie: Cookie,
  sawOtherGeneration: boolean,
): boolean {
  return (
    (cookie.kind === "present" && cookie.value === session.csrfToken) ||
    (cookie.kind === "missing" && !sawOtherGeneration)
  );
}

/** 界面卸载后，稳定的读取也可核对账本里的待结清证据。 */
export function hasEndEvidence(draft: Draft, session: AdminSession, cookie: Cookie): boolean {
  const { expiration, peer } = draft.notices;
  return matchesEvidence(expiration, session, cookie) || matchesEvidence(peer, session, cookie);
}

function matchesEvidence(
  evidence: EndEvidence | null,
  session: AdminSession,
  cookie: Cookie,
): boolean {
  return (
    evidence !== null &&
    adminSessionSameIdentity(evidence.session, session) &&
    cookieCanEndSession(session, cookie, evidence.sawOtherGeneration)
  );
}

/** 完整 DELETE、可归属的 401 和对端通知用同一会话身份结清，迟到的回执不会再广播。 */
export function settleEndNotice(draft: Draft, session: AdminSession, actions: Actions): void {
  const { expiration, peer, settled } = draft.notices;
  const heardFromPeer = peer !== null && adminSessionSameIdentity(peer.session, session);
  if (heardFromPeer) {
    draft.notices.peer = null;
  }
  if (expiration !== null && adminSessionSameIdentity(expiration.session, session)) {
    draft.notices.expiration = null;
  }
  const id = endNoticeId(session);
  if (settled !== id) {
    draft.notices.settled = id;
    if (!heardFromPeer) {
      actions.effects.push({ kind: "broadcast_ended" });
    }
  }
}

/** 确认新代际时，旧读取与退出票已失效，旧续期也无法归属新宿主。 */
export function retainSettledEndNotice(draft: Draft, confirmed: AdminSession): void {
  if (draft.notices.settled !== null && draft.notices.settled !== endNoticeId(confirmed)) {
    draft.notices.settled = null;
  }
}

/** 权威读取确认会话仍在或已换代时，结束本轮推断；已结清的身份仍用于去重。 */
export function clearPendingEndNotices(draft: Draft): void {
  draft.notices.expiration = null;
  draft.notices.peer = null;
}
