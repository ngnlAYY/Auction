// 外壳会话资源与路由焦点标识。旧宿主在确认 Cookie 不再属于它时销毁，不等待后续请求。

import { type Cell, createCell } from "@auction/web-shared/reactive/cell";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import { err, ok } from "@auction/web-shared/result";
import type { RouteMatch } from "@auction/web-shared/router";
import { createEffect, onCleanup, untrack } from "solid-js";

import { estimatedNow } from "../../api/clock";
import {
  fetchPublicSession,
  type PublicSession,
  parseStrictRfc3339,
  registerPublicSessionCheckTrigger,
  type SessionRefreshReason,
} from "../../api/session";
import {
  publicSessionIdentityRevision,
  publicSessionPrimaryRevision,
  publicSessionRevisions,
  type SessionIdentityEvidence,
} from "../../api/session/revision";
import { createCatalogProxySnapshots } from "../catalog/proxy-snapshots";
import { createCompetitionCommands } from "../commands/host";
import type { PublicSessionResource, PublicSessionResult } from "../index";
import { usePublicSessionExpiry, usePublicSessionSync } from "../session/lifecycle";
import { invalidateLotWatches, rebaseLotWatches } from "../watches/cache";
import type { SessionConfirmation } from "./session-authority";

type SessionRead = "refresh" | "reconcile" | "cookie_changed" | "expiry";

/** 外壳常驻的会话读取、命令宿主与重新登录提示。 */
export function createPublicShellSession(refreshGeneration: Cell<number>) {
  const readKind = createCell<SessionRead>("refresh");
  const resource = createTaskResource(readKind, async (kind, context) => {
    const previousUser = commandOwner.peek();
    const observation = await fetchPublicSession(context.signal, kind === "refresh", applyIdentity);
    if (!context.alive()) {
      return null;
    }
    return { kind, previousUser, ...observation };
  });
  const snapshot = createCell<PublicSessionResult | undefined>(undefined);
  const confirmation = createCell<SessionConfirmation | null>(null);
  const sessionEndedElsewhere = createCell(false);
  const commandOwner = createCell<string | null>(null);
  const preferenceReloadRevision = createCell(0);
  const proxySnapshots = createCatalogProxySnapshots();
  const lastIdentity = createCell<PublicSession | null>(null);
  let consumed = publicSessionRevisions();
  let localLogoutUnknown = false;
  let expiryPending = false;

  const setCommandOwner = (userId: string | null): boolean => {
    const previous = commandOwner.peek();
    if (previous === userId) {
      return false;
    }
    // 先失效离开账号的快照，再替换宿主；旧会话的截止和展示也不能沿用。
    if (previous !== null) {
      invalidateLotWatches(previous);
    }
    consumed = { ...consumed, login: publicSessionRevisions().login };
    lastIdentity.set(null);
    confirmation.set(null);
    snapshot.set(undefined);
    proxySnapshots.replaceIdentity(userId);
    commandOwner.set(userId);
    competition.replaceIdentity(userId);
    return true;
  };
  const clearSession = (): void => {
    confirmation.set(null);
    snapshot.set(undefined);
    resource.clear();
  };
  const restart = (kind: SessionRead): void => {
    if (readKind.peek() !== kind) {
      readKind.set(kind);
    }
    resource.restart();
  };
  const checkPendingExpiry = (): void => {
    if (expiryPending && untrack(resource.state) !== "pending") {
      restart("expiry");
    }
  };
  const onExpiry = (): void => {
    // 截止可能发生在对账读取之后、收完正文之前；到期检查不能被普通刷新去重吞掉。
    expiryPending = true;
    snapshot.set(undefined);
    checkPendingExpiry();
  };
  const consumeConfirmedLogout = (revision: string | null, confirmedLocally = false): boolean => {
    if (!confirmedLocally && (revision === null || revision === consumed.confirmed)) {
      return false;
    }
    consumed = { ...consumed, confirmed: revision ?? consumed.confirmed };
    // 成功退出已经终止旧会话。读回失败也不能继续保留私有子树或旧写命令。
    setCommandOwner(null);
    snapshot.set(ok(null));
    return true;
  };
  const checkSession = (reason: SessionRefreshReason): void => {
    if (
      (reason === "refresh" || reason === "cookie_changed") &&
      readKind.peek() !== reason &&
      resource.state() === "pending"
    ) {
      // The active authority read already owns this reconciliation. Cookie-write failure can
      // publish a follow-up signal before its result is delivered; restarting here would discard
      // that concrete failure and replace it with a second, potentially successful GET.
      return;
    }
    if (reason === "authentication_failed") {
      // 停止使用未经复核的会话提交新命令，冻结命令仍归最后确认的账号。
      clearSession();
    }
    if (reason === "logout_unknown") {
      localLogoutUnknown = true;
    }
    const revisions = publicSessionRevisions();
    if (consumeConfirmedLogout(revisions.confirmed)) {
      resource.clear();
    }
    restart(reason === "refresh" || reason === "cookie_changed" ? reason : "reconcile");
  };
  const session: PublicSessionResource & {
    confirmation: Cell<SessionConfirmation | null>;
  } = Object.assign(snapshot, {
    state: resource.state,
    restart: () => checkSession("refresh"),
    cancel: resource.cancel,
    clear: clearSession,
    confirmation,
  });
  const competition = createCompetitionCommands(session, refreshGeneration);

  const applyIdentity = (evidence: SessionIdentityEvidence): boolean => {
    if (evidence.kind === "expected-user-rejected") {
      // 迟到的冲突只否定该请求的预期用户，不得撤销随后确认的另一个账号。
      return commandOwner.peek() === evidence.userId && setCommandOwner(null);
    }
    const previous = lastIdentity.peek();
    const { revisions, cookieWritten } = evidence;
    const confirmedLocally =
      cookieWritten && evidence.kind === "session" && evidence.session === null;
    const confirmedLogout = consumeConfirmedLogout(revisions.confirmed, confirmedLocally);
    const userId = evidence.kind === "user" ? evidence.userId : (evidence.session?.user_id ?? null);
    const changed = setCommandOwner(userId);
    if (userId !== null && sessionEndedElsewhere.peek()) {
      sessionEndedElsewhere.set(false);
    }
    if (evidence.kind === "user") {
      // 只有账号的读回能确定归属，完整会话由独立读取确认。持锁的偏好写入已有通知。
      if (changed && !cookieWritten) {
        checkSession("cookie_changed");
      }
    } else if (evidence.session !== null) {
      if (sessionWasStillValid(evidence.session)) {
        lastIdentity.set(evidence.session);
      } else if (readKind.peek() !== "expiry") {
        // 即使随后的持久期同步失败，也要对已经越过截止的会话复核一次。
        expiryPending = true;
      }
    } else {
      const browserLogout =
        confirmedLogout ||
        localLogoutUnknown ||
        (revisions.unknown !== null && revisions.unknown !== consumed.unknown);
      if (!browserLogout && previous !== null && sessionWasStillValid(previous)) {
        sessionEndedElsewhere.set(true);
      }
    }
    return changed || confirmedLogout;
  };

  createEffect(resource, (observation) => {
    if (observation == null) {
      return;
    }
    const { kind, result, revisions, previousUser } = observation;
    // 只丢弃旧修订的成功结果；本轮失败（含 Cookie 同步失败）仍要显示，不能替换成第二次可能成功的读取。
    if (result.ok && revisions.primary !== publicSessionPrimaryRevision()) {
      restart("cookie_changed");
      return;
    }
    // 归属在解码时提交；异步任务晚交付的展示结果不能盖掉新证据。
    if (observation.identityRevision !== publicSessionIdentityRevision()) {
      checkPendingExpiry();
      return;
    }
    const cacheRebase = observation.watchCacheRebase;
    if (
      cacheRebase !== null &&
      cacheRebase !== undefined &&
      commandOwner.peek() === cacheRebase.userId
    ) {
      rebaseLotWatches(cacheRebase.userId, cacheRebase.fromRevision, cacheRebase.toRevision);
    }
    consumeConfirmedLogout(revisions.confirmed);
    // 这轮对账即使读取失败也消耗未知退出提示；下次例行复核仍能报告真正的非本地结束。
    localLogoutUnknown = false;
    consumed = {
      ...consumed,
      unknown: revisions.unknown ?? consumed.unknown,
    };

    const identity = result.ok ? result.value : null;
    const expired = identity !== null && !sessionWasStillValid(identity);
    if (kind === "expiry") {
      expiryPending = false;
    } else if (expired) {
      expiryPending = true;
    }
    if (expired) {
      // 迟到的正文不能恢复过期身份。到期复核仍返回已过期身份时显示错误，避免循环读取。
      snapshot.set(kind === "expiry" ? err("invalid_response") : undefined);
    } else {
      // 只提交发出权威 GET 时的修订，不能在结果交付时把新共享修订当成已确认。
      if (identity !== null && confirmation.peek()?.revision !== revisions.primary) {
        confirmation.set({ revision: revisions.primary });
      }
      snapshot.set(result);
      // 登录标记只请求重读偏好。旧宿主和冻结命令仍由权威确认的用户决定。
      if (
        identity !== null &&
        previousUser === identity.user_id &&
        revisions.login !== null &&
        revisions.login !== consumed.login &&
        revisions.login === publicSessionRevisions().login
      ) {
        consumed = { ...consumed, login: revisions.login };
        preferenceReloadRevision.update((value) => value + 1);
      }
      // Cookie 变更后同一账号的读取就地重验；复核结果不改变已由证据确认的归属。
      if (kind === "cookie_changed" && identity !== null && previousUser === identity.user_id) {
        refreshGeneration.update((value) => Math.min(value + 1, Number.MAX_SAFE_INTEGER));
      }
    }
    checkPendingExpiry();
  });
  onCleanup(registerPublicSessionCheckTrigger(checkSession, applyIdentity));
  usePublicSessionSync(checkSession);
  usePublicSessionExpiry(lastIdentity, onExpiry);
  return {
    session,
    sessionEndedElsewhere,
    commandOwner,
    preferenceReloadRevision,
    checkSession,
    competition,
    proxySnapshots,
  };
}

export function routeFocusToken(route: RouteMatch): string {
  if (route.kind === "not-found") {
    return `NotFound { segments: [${route.segments.map(rustDebugString).join(", ")}] }`;
  }
  // Dioxus 的 Route 调试串只包含枚举字段；普通路由的查询变化不应抢占主内容焦点。
  switch (route.name) {
    case "LotDetail":
      return `LotDetail { lot_code: ${rustDebugString(route.params.lot_code ?? "")} }`;
    case "Login": {
      const returnTo = route.query.return_to;
      return `Login { return_to: ${returnTo === undefined ? "None" : `Some(${rustDebugString(returnTo)})`} }`;
    }
    default:
      return route.name;
  }
}

/** Rust `Debug` 的字符串引号与转义足够覆盖路由字段；令牌只用于检测路由身份变化。 */
function rustDebugString(value: string): string {
  return JSON.stringify(value);
}

/** 依据上次确认的有效截止区分自然到期。 */
function sessionWasStillValid(identity: PublicSession): boolean {
  if (identity.expires_at === null) {
    return true;
  }
  const expiry = parseStrictRfc3339(identity.expires_at);
  return expiry !== null && expiry > BigInt(estimatedNow()) * 1_000_000n;
}

/** 旧版只把显式 Isekai 视为第二世界；缺失、Main 与非法 query 都在内存回退 Main。 */
export function selectedWorldFromQuery(value: string | null): string {
  return value === "isekai" ? "isekai" : "main";
}
