// 管理端认证外壳。会话状态机决定可见宿主与写许可，外壳负责路由和渲染。

import { useDialogFocusTrap } from "@auction/web-shared/platform/a11y";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import { useRouter } from "@auction/web-shared/router";
import { useAppBootReady } from "@auction/web-shared/ui/bootstrap";
import { t } from "@auction/web-shared/ui/i18n";
import { createContext, For, Match, Show, Switch } from "solid-js";
import type { AdminReadinessState } from "../../api/status";
import { fetchAdminReadiness } from "../../api/status";
import type { AdminReadinessResource } from "../index";
import { adminApiErrorMessage } from "../presentation";
import { restoreInitialAdminLocation } from "../router";
import { useSession } from "./effects";
import { KeyVisibilityContext } from "./key-visibility";
import { AdminLogin } from "./login";
import { AdminLoginRedirect, AdminReturnRedirect } from "./redirect";
import { AdminAuthenticatedShell, AdminSessionHostContext, LogoutRecovery } from "./shell";

/** 已认证外壳唯一拥有的 readiness 快照；Dashboard 也从这里协调首屏门禁。 */
export const AdminReadinessContext = createContext<AdminReadinessResource>();

/** 管理端外壳。与公开端外壳完全独立，并沿用旧版的左侧纵向导航布局。 */
export function AdminShell() {
  restoreInitialAdminLocation();
  // 与 Public 外壳一致：弹窗打开期间锁定背景滚动。
  useDialogFocusTrap(true);
  const router = useRouter();
  // layout 在登录与认证路由间不会重挂载，登录成功后的地址变化必须更新当前渲染分支。
  const currentRoute = createValueMemo(() => router.current());
  const isLoginPath = createValueMemo(() => {
    const route = currentRoute();
    return route.kind === "matched" && route.name === "Login";
  });
  const isDashboardPath = createValueMemo(() => {
    const route = currentRoute();
    return route.kind === "matched" && route.name === "Dashboard";
  });
  const { view, dispatch, submitLogin, host, writeReview } = useSession();
  const recheck = () => dispatch({ kind: "check_requested" });
  const continueLogout = () => dispatch({ kind: "continue_logout" });
  const stayAfterLogout = () => dispatch({ kind: "stay_after_logout" });
  const logoutRecovery = () =>
    view().logoutUnconfirmed
      ? { busy: view().logoutDecisionBusy, onContinue: continueLogout, onStay: stayAfterLogout }
      : null;
  const keyVisibility = () => view().keyVisibility;

  // 已认证外壳在站内路由切换时递增，顶栏就绪徽标随之重读；重读期间保留旧值，不影响首屏门禁。
  const readinessRefresh = createCell(0);
  const readiness = createTaskResource(
    () => readinessRefresh(),
    (_revision, context) => fetchAdminReadiness(host, context.signal),
  );
  const snapshot = view;
  const shellBootReady = createValueMemo(() => {
    const current = snapshot();
    return (
      readiness() !== undefined &&
      (current.error !== null ||
        logoutRecovery() !== null ||
        (current.loggedOut && isLoginPath()) ||
        (current.session !== null && !isLoginPath() && !isDashboardPath()))
    );
  });
  // 已认证 Dashboard 由页面快照解除门禁；重定向路径保持骨架，避免中间页面短暂露出。
  useAppBootReady(shellBootReady);
  const readinessState = createValueMemo((): AdminReadinessState => {
    const result = readiness();
    if (result === undefined) {
      return "checking";
    }
    return result.ok && result.value ? "ready" : "degraded";
  });

  return (
    // 未认证子树也拿到同一个外壳句柄；登录表单本身不再持有请求。
    <AdminSessionHostContext value={host}>
      <AdminReadinessContext value={readiness}>
        <Switch fallback={<div role="status">{t("admin-session-loading")}</div>}>
          <Match when={snapshot().session}>
            {(current) => (
              <>
                {/* 身份变化必须销毁整棵草稿/命令 Scope；单个静态组件的 key 不会形成 keyed diff。 */}
                <For
                  each={[{ epoch: snapshot().epoch, session: current() }]}
                  keyed={(entry) => entry.epoch}
                >
                  {(entry) => (
                    <KeyVisibilityContext value={keyVisibility}>
                      <AdminAuthenticatedShell
                        session={entry().session}
                        writeReview={writeReview(entry().epoch)}
                        readiness={readinessState()}
                        readinessRefresh={readinessRefresh}
                        sessionConfirmed={snapshot().canWrite}
                        redirecting={isLoginPath()}
                        loggingOut={snapshot().loggingOut}
                        logoutRecovery={logoutRecovery()}
                        logoutError={snapshot().logoutError}
                        onLogout={() => dispatch({ kind: "logout_clicked" })}
                        onRecheck={recheck}
                      />
                    </KeyVisibilityContext>
                  )}
                </For>
                {/* 已认证访问登录地址只重定向，不卸载会话草稿和未决命令宿主。 */}
                <Show when={isLoginPath()}>
                  <AdminReturnRedirect />
                </Show>
              </>
            )}
          </Match>
          <Match when={snapshot().error !== null}>
            <main class="admin-login-page">
              <section
                class="admin-login-card"
                role="alert"
                aria-labelledby="admin-session-error-title"
              >
                <h1 id="admin-session-error-title">{t("page-login-title")}</h1>
                <p class="admin-panel__note">{adminApiErrorMessage(snapshot().error ?? "")}</p>
                <button type="button" onClick={() => recheck()}>
                  {t("admin-retry")}
                </button>
              </section>
            </main>
          </Match>
          <Match when={logoutRecovery()}>
            {(recovery) => (
              <main class="admin-login-page">
                <LogoutRecovery
                  busy={recovery().busy}
                  onContinue={continueLogout}
                  onStay={stayAfterLogout}
                />
              </main>
            )}
          </Match>
          <Match when={snapshot().loggedOut}>
            <Show when={isLoginPath()} fallback={<AdminLoginRedirect />}>
              <AdminLogin
                submitting={snapshot().loginPending}
                loginError={snapshot().loginError}
                onEdit={() => dispatch({ kind: "login_input_changed" })}
                onSubmit={submitLogin}
              />
            </Show>
          </Match>
        </Switch>
      </AdminReadinessContext>
    </AdminSessionHostContext>
  );
}
