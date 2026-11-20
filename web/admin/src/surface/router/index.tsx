// 管理端路由、页面入口组件和旧版别名跳转；路由表见子模块 `route`。
// 路由路径与页面组件在同一 owner 内维护，避免 surface 装配文件再次聚合所有页面入口。

import type { RoutePage, RoutePageComponent } from "@auction/web-shared/router";
import {
  lazyPage,
  pageComponent,
  useRouter,
  writeLocationPreservingScroll,
} from "@auction/web-shared/router";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { createContext, createEffect, For, untrack, useContext } from "solid-js";

import { page as dashboardPage } from "../dashboard";
import { page as setupPage } from "../setup";

/** 冷启动地址的路径、查询与片段；查询和片段只保存在当前页面内存。 */
export interface AdminLocationSuffix {
  readonly pathname: string;
  readonly search: string;
  readonly hash: string;
}

/** 冷启动地址快照句柄：`take()` 取出后即作废，后续调用返回 `null`。 */
export interface InitialAdminLocation {
  take(): AdminLocationSuffix | null;
}

/** 冷启动地址快照所在上下文；没有提供方时按“没有快照”处理。 */
export const InitialAdminLocationContext = createContext<InitialAdminLocation | null>(null);

/** 在 Router 规范化地址前保留未在路由表声明的查询与片段。
 * 必须在根组件体里调用一次，并由它把返回值放进 {@link InitialAdminLocationContext}。 */
export function initialAdminLocation(): InitialAdminLocation {
  let pending: AdminLocationSuffix | null = null;
  if (typeof window !== "undefined") {
    try {
      const location = window.location;
      pending = captureInitialAdminLocation(location.pathname, location.search, location.hash);
    } catch {
      // 与源码的 `?` 一致：任一地址字段读取失败就整份放弃，不保留半份快照。
      pending = null;
    }
  }
  return {
    take(): AdminLocationSuffix | null {
      const value = pending;
      pending = null;
      return value;
    },
  };
}

/** 只有同时满足“有查询或片段”与“同源绝对路径”的地址才值得恢复。
 * 协议相对地址与含反斜杠的路径都可能被浏览器解释成另一种导航，一律跳过。 */
export function captureInitialAdminLocation(
  pathname: string,
  search: string,
  hash: string,
): AdminLocationSuffix | null {
  if (
    (search.length === 0 && hash.length === 0) ||
    !pathname.startsWith("/") ||
    pathname.startsWith("//") ||
    pathname.includes("\\")
  ) {
    return null;
  }
  return { pathname, search, hash };
}

/** 仅补回 Router 去掉的后缀，已有的新查询、片段或导航优先。 */
export function initialAdminUrl(
  initial: AdminLocationSuffix,
  pathname: string,
  search: string,
  hash: string,
): string | null {
  const trailingSlashDropped = initial.pathname.endsWith("/")
    ? initial.pathname.slice(0, -1)
    : null;
  if (
    (pathname !== initial.pathname && trailingSlashDropped !== pathname) ||
    (search.length > 0 && search !== initial.search) ||
    (hash.length > 0 && hash !== initial.hash) ||
    (search === initial.search && hash === initial.hash)
  ) {
    return null;
  }
  return `${pathname}${initial.search}${initial.hash}`;
}

/** 认证请求前恢复一次冷启动 URL，保留 Router 已写入的历史状态。 */
export function restoreInitialAdminLocation(): void {
  const initial = useContext(InitialAdminLocationContext);
  if (initial === null) {
    return;
  }
  const preserved = initial.take();
  if (preserved === null || typeof window === "undefined") {
    return;
  }
  let pathname: string;
  let search: string;
  let hash: string;
  try {
    const location = window.location;
    pathname = location.pathname;
    search = location.search;
    hash = location.hash;
  } catch {
    // 地址读取失败时放弃恢复；认证请求照常进行，只是不带冷启动后缀。
    return;
  }
  const url = initialAdminUrl(preserved, pathname, search, hash);
  if (url === null) {
    return;
  }
  // 写入失败时地址保持不变；源码同样忽略失败，认证请求不会因此中断。
  writeLocationPreservingScroll(url, "replace");
}

function Setup(): JSX.Element {
  return setupPage();
}

function Dashboard(): JSX.Element {
  return dashboardPage();
}

function DashboardAlias(): JSX.Element {
  return redirectAdminAlias("/");
}

function Login(): JSX.Element {
  // 未认证时由 AdminShell 显示登录表单；已认证后访问旧登录路径回到总览。
  return dashboardPage();
}

function pageForParam(
  param: string,
  renderPage: (value: string) => JSX.Element,
): RoutePageComponent {
  // 同一路由内换标识也要重挂载页面，避免把上一个对象的资源、草稿和对话框带到新地址。
  return (props) => (
    <For each={[props.params[param] ?? ""]} keyed={(value) => value}>
      {(value) => renderPage(untrack(value))}
    </For>
  );
}

function AuctionOverview(): JSX.Element {
  return redirectAdminAlias("/auctions");
}

function MailOverview(): JSX.Element {
  return redirectAdminAlias("/mails");
}

function Credentials(): JSX.Element {
  return redirectAdminAlias("/cookies");
}

function DeliveryOverview(): JSX.Element {
  return redirectAdminAlias("/deliveries");
}

function Rules(): JSX.Element {
  return redirectAdminAlias("/rules-content");
}

/** 把旧版管理端别名无历史记录地收敛到 canonical URL，并保留查询字符串和片段。
 * Admin 旧链接仍保留在路由表中以兼容书签；`replace` 确保返回键不会在别名与正式地址间
 * 形成额外的导航环。片段由旧版路由行为保留，便于外部链接定位页面内锚点。 */
function redirectAdminAlias(path: string): JSX.Element {
  const router = useRouter();
  const suffix = currentBrowserSuffix();
  const hashIndex = suffix.indexOf("#");
  const search = hashIndex === -1 ? suffix : suffix.slice(0, hashIndex);
  const hash = hashIndex === -1 ? "" : suffix.slice(hashIndex);
  const target = `${path}${suffix}`;
  createEffect(
    () => null,
    () => {
      if (hash === "") {
        router.replace(target);
        return;
      }
      // 共享路由器会把带片段的 replace 登记为待定位落点；Rust 的 WebHistory::replace
      // 只把 URL 片段保留下来，然后滚到页首。先按无片段地址替换条目，再把旧后缀补回。
      router.replace(`${path}${search}`);
      writeLocationPreservingScroll(target, "replace");
    },
  );
  return null;
}

/** 当前地址的查询与片段；任一项读取失败时按该项为空处理。 */
function currentBrowserSuffix(): string {
  if (typeof window === "undefined") {
    return "";
  }
  let search = "";
  let hash = "";
  try {
    search = window.location.search;
  } catch {
    // 读取失败按无查询处理：别名跳转仍应到达正式地址。
  }
  try {
    hash = window.location.hash;
  } catch {
    // 读取失败按无片段处理：别名跳转仍应到达正式地址。
  }
  return `${search}${hash}`;
}

/** 404 位于认证外壳的 `main#admin-content` 之内：只输出页面区块，不再嵌套第二个 `main`
 * 地标，也不套用整屏高度的外壳类。 */
export function NotFoundPage(): JSX.Element {
  return (
    <section class="admin-panel" aria-labelledby="admin-not-found-title">
      <h1 id="admin-not-found-title">{t("not-found-title")}</h1>
      <p class="admin-panel__note">{t("not-found-note")}</p>
    </section>
  );
}

/** 认证外壳与 `/setup` 共用的页面表。仪表盘随入口交付，登录后可以立即开始读取首屏数据。 */
export const ADMIN_PAGES: Readonly<Record<string, RoutePage>> = {
  Setup: pageComponent(Setup),
  Dashboard: pageComponent(Dashboard),
  DashboardAlias: pageComponent(DashboardAlias),
  Login: pageComponent(Login),
  Auctions: lazyPage(() => import("../auctions").then(({ page }) => page)),
  AuctionLots: lazyPage(() =>
    import("../lots/page").then(({ page }) => pageForParam("auction_id", page)),
  ),
  AuctionOverview: pageComponent(AuctionOverview),
  Mails: lazyPage(() => import("../mail/page").then(({ page }) => page)),
  MailOverview: pageComponent(MailOverview),
  Cookies: lazyPage(() => import("../credentials/page").then(({ page }) => page)),
  Credentials: pageComponent(Credentials),
  Users: lazyPage(() => import("../users").then(({ page }) => page)),
  Renames: lazyPage(() => import("../renames").then(({ page }) => page)),
  Deliveries: lazyPage(() => import("../deliveries/page").then(({ page }) => page)),
  DeliveryDetail: lazyPage(() =>
    import("../deliveries/detail").then(({ detailPage }) =>
      pageForParam("delivery_id", detailPage),
    ),
  ),
  DeliveryOverview: pageComponent(DeliveryOverview),
  PlayerSupport: lazyPage(() => import("../support/page").then(({ page }) => page)),
  Outbox: lazyPage(() => import("../realtime/page").then(({ outboxPage }) => outboxPage)),
  Settlement: lazyPage(() => import("../settlement/page").then(({ page }) => page)),
  SettlementReturns: lazyPage(() => import("../returns/list").then(({ page }) => page)),
  SettlementReturnDetail: lazyPage(() =>
    import("../returns/detail").then(({ detailPage }) => pageForParam("return_id", detailPage)),
  ),
  Config: lazyPage(() => import("../config/page").then(({ page }) => page)),
  RulesContent: lazyPage(async () => {
    const { page } = await import("../rules/page");
    return () => page("rules");
  }),
  Rules: pageComponent(Rules),
  AboutContent: lazyPage(async () => {
    const { page } = await import("../rules/page");
    return () => page("about");
  }),
  Alerts: lazyPage(() => import("../alerts").then(({ page }) => page)),
  Audit: lazyPage(() => import("../audit").then(({ page }) => page)),
  Status: lazyPage(() => import("../status").then(({ page }) => page)),
};
