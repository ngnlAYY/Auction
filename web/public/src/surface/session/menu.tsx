// 页头账号菜单：头像下拉、账户入口、退出与匿名登录入口。

import { focusElementById } from "@auction/web-shared/platform/a11y";
import { usePagePanelEscape } from "@auction/web-shared/platform/a11y/escape";
import { type Cell, createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskScope } from "@auction/web-shared/reactive/tasks";
import { Link } from "@auction/web-shared/router";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Match, Show, Switch, useContext } from "solid-js";

import { logoutPublic } from "../../api/session";
import { consignmentMenuEntries, useConsignmentCapability } from "../consignment/capability";
import { PublicSessionResourceContext } from "../index";

/** 账号面板入口按钮与面板的元素 ID；面板关闭时按 ID 把焦点还给入口。 */
const SESSION_TRIGGER_ID = "public-account-session-trigger";
const SESSION_PANEL_ID = "public-account-session-panel";

type SessionMenuState =
  | { readonly kind: "checking" }
  | { readonly kind: "unavailable" }
  | { readonly kind: "anonymous" }
  | { readonly kind: "authenticated"; readonly userId: string; readonly username: string };

/** 页头账号菜单：头像下拉，集中账户、退出与匿名登录入口。 */
export function SessionMenu(props: { readonly scopedHref: (path: string) => string }): JSX.Element {
  const session = useContext(PublicSessionResourceContext);
  const logoutError = createCell<string | null>(null);
  const logoutBusy = createCell(false);
  const menuOpen = createCell(false);
  const tasks = createTaskScope();
  // 与导航共用同一次能力读取；只在已登录的面板里使用结果。
  const consignment = useConsignmentCapability();
  const consignmentEntries = createValueMemo(() => consignmentMenuEntries(consignment()));

  const sessionState = createValueMemo((): SessionMenuState => {
    const snapshot = session();
    if (snapshot === undefined) {
      return { kind: "checking" };
    }
    if (!snapshot.ok) {
      return { kind: "unavailable" };
    }
    if (snapshot.value === null) {
      return { kind: "anonymous" };
    }
    if (snapshot.value.authenticated) {
      return {
        kind: "authenticated",
        userId: snapshot.value.user_id,
        username: snapshot.value.username,
      };
    }
    return { kind: "anonymous" };
  });
  const authenticated = () => {
    const current = sessionState();
    return current.kind === "authenticated" ? current : null;
  };
  const closeMenu = () => closeSessionMenu(menuOpen);
  usePagePanelEscape(() => authenticated() !== null && menuOpen(), {
    triggerId: SESSION_TRIGGER_ID,
    panelId: SESSION_PANEL_ID,
    dismiss: closeMenu,
  });

  return (
    // biome-ignore lint/a11y/useAriaPropsSupportedByRole: 源码在账号菜单的普通容器上保留 aria-label，不能改 DOM 层级。
    <div class="public-account-session-menu" aria-label={t("session-menu-label")}>
      <Switch>
        <Match when={sessionState().kind === "checking"}>
          <span role="status">{t("common-loading")}</span>
        </Match>
        <Match when={sessionState().kind === "unavailable"}>
          <span class="action-feedback action-feedback--error" role="alert">
            {t("session-error")}
          </span>
          <button
            type="button"
            disabled={session.state() !== "ready"}
            onClick={() => session.restart()}
          >
            {t("data-retry")}
          </button>
        </Match>
        <Match when={sessionState().kind === "anonymous"}>
          {/* 顶栏保留唯一登录入口；目录的查看/出价/标记图标也可引导登录。 */}
          <Link class="public-account-session-login" href={props.scopedHref("/login")}>
            {t("session-login")}
          </Link>
        </Match>
        <Match when={authenticated()}>
          {(identity) => {
            const initial = () => Array.from(identity().username)[0]?.toUpperCase() ?? "";
            return (
              <>
                {/* 展开式（disclosure）面板而非 ARIA 菜单：条目按 Tab 浏览，不提供方向键导航。 */}
                <button
                  id={SESSION_TRIGGER_ID}
                  class="public-account-session-trigger"
                  type="button"
                  aria-controls={SESSION_PANEL_ID}
                  aria-expanded={menuOpen() ? "true" : "false"}
                  onClick={() => menuOpen.set(!menuOpen())}
                >
                  <span class="public-account-session-avatar" aria-hidden="true">
                    {initial()}
                  </span>
                  <span class="public-account-session-name">{identity().username}</span>
                  <svg
                    class="public-account-session-chevron"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="1.5"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                    aria-hidden="true"
                  >
                    <path d="m6 9 6 6 6-6" />
                  </svg>
                </button>
                <Show when={menuOpen()}>
                  {/* biome-ignore lint/a11y/noStaticElementInteractions: 遮罩只承担关闭点击，交互面板在内层。 */}
                  {/* biome-ignore lint/a11y/useKeyWithClickEvents: 遮罩只承担关闭点击，文档监听处理 Escape。 */}
                  <div class="public-account-session-backdrop" onClick={() => closeMenu()} />
                  <div id={SESSION_PANEL_ID} class="public-account-session-panel">
                    {/* “我的”与退出不把焦点还给入口：路由切换接管焦点，退出后入口随会话卸载。 */}
                    <Link
                      class="public-account-session-item"
                      href={props.scopedHref("/me")}
                      onClick={() => menuOpen.set(false)}
                    >
                      {t("nav-my-bids")}
                    </Link>
                    <Show when={consignmentEntries().length > 0}>
                      <div class="public-account-session-heading">
                        {t("session-menu-consignment")}
                      </div>
                      <For each={consignmentEntries()} keyed={(entry) => entry.href}>
                        {(entry) => (
                          <Link
                            class="public-account-session-item"
                            href={entry().href}
                            onClick={() => menuOpen.set(false)}
                          >
                            {t(entry().labelKey)}
                          </Link>
                        )}
                      </For>
                    </Show>
                    <button
                      class="public-account-session-item"
                      type="button"
                      disabled={logoutBusy()}
                      onClick={() => {
                        if (logoutBusy()) {
                          return;
                        }
                        menuOpen.set(false);
                        logoutError.set(null);
                        logoutBusy.set(true);
                        const userId = identity().userId;
                        tasks.spawn(async (context) => {
                          const result = await logoutPublic(userId, context.signal);
                          if (!context.alive()) {
                            return;
                          }
                          if (!result.ok) {
                            logoutError.set(result.error);
                            focusElementById(SESSION_TRIGGER_ID);
                          }
                          logoutBusy.set(false);
                        });
                      }}
                    >
                      {logoutBusy() ? t("common-loading") : t("logout")}
                    </button>
                  </div>
                </Show>
              </>
            );
          }}
        </Match>
      </Switch>
      <Show when={logoutError() !== null}>
        <span class="action-feedback action-feedback--error" role="alert">
          {logoutError() === "session_lock_unavailable"
            ? t("session-lock-unavailable")
            : logoutError() === "session_lock_timeout"
              ? t("session-lock-timeout")
              : t("logout-error")}
        </span>
      </Show>
    </div>
  );
}

/** 收起账号面板并把焦点还给入口按钮；面板卸载时焦点不能落到 `body`。 */
function closeSessionMenu(open: Cell<boolean>): void {
  open.set(false);
  focusElementById(SESSION_TRIGGER_ID);
}
