// 已认证的管理端外壳。

import type { AdminPendingCountsResponse } from "@auction/contracts/dto/admin/alerts";
import { useRouteFocus } from "@auction/web-shared/platform/a11y";
import { SecondTickerContext, useSecondTicker } from "@auction/web-shared/platform/clock";
import { loadSidebarCollapsed, saveSidebarCollapsed } from "@auction/web-shared/platform/sidebar";
import { ThemePicker } from "@auction/web-shared/platform/theme";
import { type Cell, createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import type { Result } from "@auction/web-shared/result";
import { type RouteMatch, RouteOutlet, useRouter } from "@auction/web-shared/router";
import { LanguagePicker, t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import {
  createContext,
  createEffect,
  onCleanup,
  onSettled,
  Show,
  untrack,
  useContext,
} from "solid-js";

import { fetchAdminPendingCounts } from "../../api/alerts";
import {
  type AdminSession,
  type AdminSessionHost,
  type AdminSessionWriteReview,
  registerAdminSessionHost,
} from "../../api/session";
import type { AdminReadinessState } from "../../api/status";
import { ConfigDraftContext, useConfigDraft } from "../config/draft";
import { ADMIN_LOCALE_CATALOG } from "../index";
import { PendingCommandHostContext, usePendingCommandHost } from "../pending";
import { PendingCommandNotices } from "../pending/panel";
import { adminApiErrorMessage } from "../presentation";
import { ADMIN_PAGES, NotFoundPage } from "../router";
import { AdminPageLoadFailure } from "../router/page-load";
import { DocumentDraftsContext, useRulesDraft } from "../rules/model";
import { LOGOUT_BUTTON_CLASS } from "./activity";
import { AdminReadinessStatus } from "./login";
import { AdminPendingCountsContext } from "./navigation";
import { Navigation as DesktopNavigation } from "./navigation/desktop";
import { Navigation as MobileNavigation, NavigationToggle } from "./navigation/mobile";

/** 本次已认证外壳登记的会话句柄；子树只能经由上下文取得，不读取模块级登记栈。 */
export const AdminSessionHostContext = createContext<AdminSessionHost | null>(null);

/** 读取当前认证外壳的会话句柄；缺少提供方是装配错误。 */
export function useAdminSessionHost(): AdminSessionHost {
  const host = useContext(AdminSessionHostContext);
  if (host === null) {
    throw new Error("认证外壳会话宿主缺失");
  }
  return host;
}

interface LogoutRecoveryProps {
  readonly busy: boolean;
  readonly onContinue: () => void;
  readonly onStay: () => void;
}

/** 只读退出复核期间可用的两个明确选择。 */
export function LogoutRecovery(props: LogoutRecoveryProps): JSX.Element {
  return (
    <div class="admin-shell__banner">
      <p role="alert">{t("admin-logout-unconfirmed")}</p>
      <button type="button" disabled={props.busy} onClick={() => props.onContinue()}>
        {t("admin-logout-continue")}
      </button>
      <button type="button" disabled={props.busy} onClick={() => props.onStay()}>
        {t("admin-logout-stay")}
      </button>
    </div>
  );
}

/** 已认证的外壳；会话代际变化由父组件重挂载本组件，草稿随旧外壳一起销毁。 */
export function AdminAuthenticatedShell(props: {
  readonly session: AdminSession;
  readonly writeReview?: AdminSessionWriteReview;
  readonly readiness: AdminReadinessState;
  readonly readinessRefresh: Cell<number>;
  readonly sessionConfirmed: boolean;
  readonly redirecting: boolean;
  readonly loggingOut: boolean;
  readonly logoutRecovery?: LogoutRecoveryProps | null;
  readonly logoutError: string | null;
  readonly onLogout: () => void;
  readonly onRecheck: () => void;
}): JSX.Element {
  // 已确认身份、写许可与重查入口随本外壳登记；退出或换代时移除，不沿用父组件后续的视图。
  let alive = true;
  onCleanup(() => {
    alive = false;
  });
  // 写许可在渲染的跟踪上下文里读取 props，命令按钮才能随同代会话复核恢复；
  // 命令发出前仍由传输层或未决宿主同步再读一次。
  const writeReview = untrack(() => props.writeReview);
  const registration = registerAdminSessionHost({
    check: () => props.onRecheck(),
    writesAllowed: () => alive && props.sessionConfirmed,
    confirmed: () => (alive ? props.session : null),
    ...(writeReview === undefined ? {} : { writeReview }),
  });
  // 句柄随本次已认证外壳登记；未决命令宿主持有同一句柄并通过既有 Context 交给子树。
  // 旧代际的命令只能核对旧句柄，不能借用父组件后来登记的会话。
  const sessionHost = registration.host;
  onCleanup(registration.release);
  const commandHost = usePendingCommandHost(sessionHost);
  // 草稿由认证外壳持有，站内切页不丢失输入与重试命令；退出会话后随外壳销毁。
  const configDraft = useConfigDraft();
  const rulesDraft = useRulesDraft();
  // 外壳持有唯一的秒级共享时钟；需要每秒刷新的子树通过 `useSecondTick` 订阅同一个 interval。
  const ticker = useSecondTicker();
  const navigationOpen = createCell(false);
  const sidebarCollapsed = createCell(loadSidebarCollapsed());
  const navigationToggle = createCell<HTMLButtonElement | null>(null);
  const router = useRouter();
  const failedPage = createCell<RouteMatch | null>(null);
  const currentPageLoadFailure = () => {
    const failure = failedPage();
    return !props.redirecting && failure === router.current() ? failure : null;
  };
  // 整页重载会释放本会话持有的请求、回执（包括一次性 Key）和草稿。
  // 退出结果未确认时也保留当前文档，让原会话继续跟踪退出并保持写门禁关闭。
  // 保存后即使草稿干净，重新读取配置也无法恢复本会话的通用重启提醒。
  const hasReloadRisk = () =>
    props.loggingOut ||
    Boolean(props.logoutRecovery) ||
    commandHost.entries.peek().length > 0 ||
    commandHost.notices.peek().length > 0 ||
    configDraft.restartRequired.peek();
  const hasDirtyDraft = () =>
    configDraft.dirty() || rulesDraft.rules.dirty() || rulesDraft.about.dirty();
  // 路由身份与 Rust `Route` 一样不含查询；仅查询变化不收起手机导航，也不刷新侧栏待办数。
  const currentRoute = createValueMemo(() => withoutRouteQuery(router.current()));
  const settlementRoute = (): boolean => {
    const route = currentRoute();
    return route.kind === "matched" && route.name === "Settlement";
  };
  // 侧栏待办数与顶栏就绪徽标随页面切换刷新；会话内的命令完成或服务恢复后，下一次
  // 路由变化即可看到最新值（服务端就绪检查有 1 秒缓存，不另起后台轮询）。
  const pendingRefresh = createCell(0);
  const pendingCounts = createTaskResource(
    () => pendingRefresh(),
    (_generation, context): Promise<Result<AdminPendingCountsResponse, string>> =>
      fetchAdminPendingCounts(sessionHost, context.signal),
  );
  // 挂载时沿用外壳已有的就绪快照（首屏门禁正在等它），只在之后真正切换路由时重读。
  // 路由是响应式的；外壳初始化只需要首次挂载的快照，后续变化由下面的 effect 处理。
  const refreshRoute = createCell(untrack(currentRoute));
  // 浏览器前进/后退和页面内跳转也收起手机导航；页面焦点仍由路由 hook 接管。
  createEffect(
    () => currentRoute(),
    (route) => {
      navigationOpen.set(false);
      refreshAfterNavigation(route, refreshRoute, pendingRefresh, props.readinessRefresh);
    },
  );
  useRouteFocus(() => routeFocusToken(currentRoute()), "admin-content");

  return (
    <AdminSessionHostContext value={sessionHost}>
      <PendingCommandHostContext value={commandHost}>
        <ConfigDraftContext value={configDraft}>
          <DocumentDraftsContext value={rulesDraft}>
            <SecondTickerContext value={ticker}>
              <AdminPendingCountsContext value={pendingCounts}>
                <div
                  class={[
                    "admin-shell",
                    "admin-console",
                    { "is-sidebar-collapsed": sidebarCollapsed() },
                  ]}
                >
                  <a class="console-skip" href="#admin-content">
                    {t("skip")}
                  </a>
                  <header class="console-ticker">
                    <div class="console-ticker__left">
                      <NavigationToggle
                        open={navigationOpen()}
                        onMount={(element) => navigationToggle.set(element)}
                        onToggle={() => navigationOpen.update((open) => !open)}
                      />
                      <span class="console-ticker__dot" aria-hidden="true" />
                      <span class="console-ticker__auction">{t("admin-brand")}</span>
                    </div>
                    <div class="console-ticker__right">
                      <AdminReadinessStatus state={props.readiness} />
                      <LanguagePicker label={t("language-label")} catalog={ADMIN_LOCALE_CATALOG} />
                      <ThemePicker
                        label={t("theme-label")}
                        darkLabel={t("theme-dark")}
                        lightLabel={t("theme-light")}
                      />
                      <Show when={!props.logoutRecovery}>
                        <button
                          class={LOGOUT_BUTTON_CLASS}
                          type="button"
                          disabled={props.loggingOut}
                          onClick={() => props.onLogout()}
                        >
                          {t("admin-logout")}
                        </button>
                      </Show>
                    </div>
                  </header>
                  <Show when={props.logoutError !== null}>
                    <p class="admin-shell__banner" role="alert">
                      {adminApiErrorMessage(props.logoutError ?? "")}
                    </p>
                  </Show>
                  <Show when={props.logoutRecovery}>
                    {(recovery) => (
                      <LogoutRecovery
                        busy={recovery().busy}
                        onContinue={() => recovery().onContinue()}
                        onStay={() => recovery().onStay()}
                      />
                    )}
                  </Show>
                  <Show
                    when={!props.logoutRecovery && !props.loggingOut && !props.sessionConfirmed}
                  >
                    <p class="admin-shell__banner" role="alert">
                      {t("admin-session-recheck-blocked")}
                      <button type="button" onClick={() => props.onRecheck()}>
                        {t("admin-retry")}
                      </button>
                    </p>
                  </Show>
                  <div class="console-body">
                    <DesktopNavigation
                      currentRoute={currentRoute()}
                      collapsed={sidebarCollapsed()}
                      onToggle={() => {
                        const next = !sidebarCollapsed();
                        sidebarCollapsed.set(next);
                        saveSidebarCollapsed(next);
                      }}
                    />
                    <MobileNavigation
                      currentRoute={currentRoute()}
                      open={navigationOpen()}
                      onNavigate={() => navigationOpen.set(false)}
                      onKeyDown={(event) => {
                        if (event.key === "Escape" && navigationOpen()) {
                          event.preventDefault();
                          event.stopPropagation();
                          navigationOpen.set(false);
                          const toggle = navigationToggle.peek();
                          if (toggle !== null) {
                            // 菜单收起的 DOM 更新完成后归还焦点；宿主已卸载时丢弃。
                            onSettled(() => {
                              if (alive) {
                                toggle.focus();
                              }
                            });
                          }
                        }
                      }}
                    />
                    <main
                      class={
                        settlementRoute()
                          ? "console-main admin-shell__content--settlement"
                          : "console-main"
                      }
                      id="admin-content"
                      tabindex="-1"
                    >
                      <PendingCommandNotices host={commandHost} />
                      <Show when={currentPageLoadFailure()} keyed>
                        {(failure) => (
                          <AdminPageLoadFailure
                            isCurrent={() => alive && currentPageLoadFailure() === failure}
                            canAutoReload={() => !hasReloadRisk() && !hasDirtyDraft()}
                            confirmManualReload={() => hasReloadRisk() && !hasDirtyDraft()}
                          />
                        )}
                      </Show>
                      <fieldset disabled={!props.sessionConfirmed} style="display: contents;">
                        <Show when={!props.redirecting}>
                          <RouteOutlet
                            pages={ADMIN_PAGES}
                            notFound={<NotFoundPage />}
                            onLoadError={() => {
                              const route = router.current();
                              if (route.kind === "matched") {
                                console.warn("管理后台页面分块加载失败", { route: route.name });
                              }
                              failedPage.set(route);
                              router.restoreScrollAfterCommit();
                            }}
                          />
                        </Show>
                      </fieldset>
                    </main>
                  </div>
                </div>
              </AdminPendingCountsContext>
            </SecondTickerContext>
          </DocumentDraftsContext>
        </ConfigDraftContext>
      </PendingCommandHostContext>
    </AdminSessionHostContext>
  );
}

/** 只有路由身份变化才刷新两个计数；首次挂载沿用父外壳传入的快照。 */
function refreshAfterNavigation(
  route: RouteMatch,
  refreshRoute: Cell<RouteMatch>,
  pendingRefresh: Cell<number>,
  readinessRefresh: Cell<number>,
): void {
  if (sameRoute(refreshRoute.peek(), route)) {
    return;
  }
  refreshRoute.set(route);
  // 使用 peek，读取代数并回写时不会让当前 effect 因订阅了自身再跑一轮。
  pendingRefresh.set(saturatingIncrement(pendingRefresh.peek()));
  readinessRefresh.set(saturatingIncrement(readinessRefresh.peek()));
}

/** Dioxus `Route` 不含查询；查询变化不构成一次导航。 */
function sameRoute(left: RouteMatch, right: RouteMatch): boolean {
  if (left.kind !== right.kind) {
    return false;
  }
  if (left.kind === "not-found" || right.kind === "not-found") {
    return (
      left.kind === "not-found" &&
      right.kind === "not-found" &&
      left.segments.length === right.segments.length &&
      left.segments.every((segment, index) => segment === right.segments[index])
    );
  }
  return left.name === right.name && sameParams(left.params, right.params);
}

function sameParams(
  left: Readonly<Record<string, string>>,
  right: Readonly<Record<string, string>>,
): boolean {
  const leftKeys = Object.keys(left);
  const rightKeys = Object.keys(right);
  return leftKeys.length === rightKeys.length && leftKeys.every((key) => right[key] === left[key]);
}

/** 仅把路由匹配结果去掉查询，供导航与刷新比较使用。 */
function withoutRouteQuery(route: RouteMatch): RouteMatch {
  return route.kind === "matched" ? { ...route, query: {} } : route;
}

/** 路由焦点令牌只编码路径身份；查询变化不抢占用户焦点。 */
function routeFocusToken(route: RouteMatch): string {
  return route.kind === "matched"
    ? JSON.stringify([route.name, route.params])
    : JSON.stringify([route.kind, route.segments]);
}

/** Rust u64 的饱和加一；JavaScript 计数器在安全整数上限停止。 */
function saturatingIncrement(value: number): number {
  return Math.min(value + 1, Number.MAX_SAFE_INTEGER);
}
