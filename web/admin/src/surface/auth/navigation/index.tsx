// 桌面侧栏与手机折叠菜单的纯展示；路由、会话与菜单状态仍由认证外壳唯一持有。
// 两端共用分组导航骨架（概览 / 拍卖运营 / 系统），桌面外壳负责品牌与收起。

import type { AdminPendingCountsResponse } from "@auction/contracts/dto/admin/alerts";
import type { TaskResource } from "@auction/web-shared/reactive/resource";
import type { Result } from "@auction/web-shared/result";
import { Link } from "@auction/web-shared/router";
import type { RouteMatch } from "@auction/web-shared/router/pattern";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { createContext, Show, useContext } from "solid-js";

import { ADMIN_ROUTES } from "../../router/route";

/** 外壳持有唯一待办计数资源；导航只读取，不发起请求。 */
export const AdminPendingCountsContext = createContext<TaskResource<
  Result<AdminPendingCountsResponse, string>
> | null>(null);

const DEFAULT_ICON_PATH = "M12 2a10 10 0 1 0 0 20 10 10 0 0 0 0-20z";

const ICON_PATHS: Readonly<Record<string, string>> = {
  grid: "M3 3h7v7H3zM14 3h7v7h-7zM3 14h7v7H3zM14 14h7v7h-7z",
  bell: "M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9M10.3 21a1.94 1.94 0 0 0 3.4 0",
  gavel: "m14 13-7.5 7.5a2.12 2.12 0 0 1-3-3L11 10M16 16l6-6M8 8l6-6M9 7l8 8M21 11l-8-8",
  send: "m22 2-7 20-4-9-9-4ZM22 2 11 13",
  check: "M22 11.08V12a10 10 0 1 1-5.93-9.14M22 4 12 14.01l-3-3",
  return: "M9 14 4 9l5-5M4 9h10.5a5.5 5.5 0 0 1 0 11H11",
  inbox:
    "M22 12h-6l-2 3h-4l-2-3H2M5.45 5.11 2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.45-6.89A2 2 0 0 0 16.76 4H7.24a2 2 0 0 0-1.79 1.11z",
  users:
    "M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2M9 11a4 4 0 1 0 0-8 4 4 0 0 0 0 8M22 21v-2a4 4 0 0 0-3-3.87M16 3.13a4 4 0 0 1 0 7.75",
  cookie:
    "M12 2a10 10 0 1 0 10 10 4 4 0 0 1-5-5 4 4 0 0 1-5-5M8.5 8.5h.01M16 15.5h.01M12 12h.01M11 17h.01M7 14h.01",
  edit: "M12 20h9M16.5 3.5a2.12 2.12 0 0 1 3 3L7 19l-4 1 1-4z",
  message: "M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z",
  settings:
    "M12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.6 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.6h.09A1.65 1.65 0 0 0 10 3.09V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9v.09a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z",
  file: "M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8zM14 2v6h6M16 13H8M16 17H8M10 9H8",
  shield: "M12 22s8-3 8-10V5l-8-3-8 3v7c0 7 8 10 8 10z",
  chevron: "m15 18-6-6 6-6",
};

export interface NavigationIconProps {
  readonly kind: string;
}

/** 未知图标名回退默认圆形；只认自有键，避免命中 `constructor` 等原型属性。 */
function iconPath(kind: string): string {
  return Object.hasOwn(ICON_PATHS, kind)
    ? (ICON_PATHS[kind] ?? DEFAULT_ICON_PATH)
    : DEFAULT_ICON_PATH;
}

/** 侧栏图标；统一 18px、1.5 stroke，折叠态仍能识别导航。 */
export function NavigationIcon(props: NavigationIconProps): JSX.Element {
  return (
    <svg
      class="console-nav__icon"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="1.5"
      stroke-linecap="round"
      stroke-linejoin="round"
      aria-hidden="true"
    >
      <path d={iconPath(props.kind)} />
    </svg>
  );
}

type AdminNavName =
  | "Dashboard"
  | "Alerts"
  | "Auctions"
  | "Consignment"
  | "Deliveries"
  | "Settlement"
  | "SettlementReturns"
  | "Mails"
  | "Users"
  | "Cookies"
  | "Renames"
  | "PlayerSupport"
  | "Outbox"
  | "Config"
  | "RulesContent"
  | "AboutContent"
  | "Audit";

/** 子页面归属的侧栏分区：lot 列表属于拍卖场次，投递详情属于投递队列，退件案件属于结算退件。 */
function navTarget(route: RouteMatch): string | null {
  if (route.kind === "not-found") {
    return null;
  }
  switch (route.name) {
    case "AuctionLots":
      return "Auctions";
    case "ConsignmentListing":
      return "Consignment";
    case "DeliveryDetail":
      return "Deliveries";
    case "SettlementReturnDetail":
      return "SettlementReturns";
    default:
      return route.name;
  }
}

function adminHref(name: AdminNavName): string {
  const definition = ADMIN_ROUTES.find((candidate) => candidate.name === name);
  if (definition === undefined) {
    throw new Error(`管理端路由表缺少 ${name}`);
  }
  return definition.pattern;
}

interface NavigationLinkProps {
  readonly to: AdminNavName;
  readonly label: string;
  readonly icon: string;
  readonly currentRoute: RouteMatch;
  readonly count: number;
}

function NavigationLink(props: NavigationLinkProps): JSX.Element {
  const href = (): string => adminHref(props.to);
  const active = (): boolean => navTarget(props.currentRoute) === props.to;
  return (
    <Link
      class="console-nav__link"
      href={href()}
      aria-current={active() ? "page" : "false"}
      title={props.label}
    >
      <NavigationIcon kind={props.icon} />
      <span class="console-nav__label">{props.label}</span>
      <Show when={props.count > 0}>
        <span class="console-nav__count" title={t("nav-pending-count", { count: props.count })}>
          {props.count}
        </span>
      </Show>
    </Link>
  );
}

/** 认证外壳共用的分组导航；待办计数来自外壳提供的唯一资源。 */
export function NavigationLinks(props: { readonly currentRoute: RouteMatch }): JSX.Element {
  // 外壳持有唯一待办计数资源；登录过程中尚未取得结果时不显示徽标。
  const countsResource = useContext(AdminPendingCountsContext);
  const counts = (): AdminPendingCountsResponse | null => {
    const result = countsResource?.();
    if (result === undefined || !result.ok) {
      return null;
    }
    return result.value;
  };
  const pending = (route: AdminNavName): number => {
    const current = counts();
    if (current === null) {
      return 0;
    }
    switch (route) {
      case "Deliveries":
        return current.delivery_manual_reconcile;
      case "Settlement":
        return current.recovery_confirmation;
      case "SettlementReturns":
        return current.settlement_returns ?? 0;
      case "Alerts":
        return current.active_alerts;
      default:
        return 0;
    }
  };

  return (
    <div class="console-nav">
      <div class="console-nav__group">
        <div class="console-nav__head">{t("console-nav-overview")}</div>
        <NavigationLink
          to="Dashboard"
          icon="grid"
          label={t("nav-dashboard")}
          currentRoute={props.currentRoute}
          count={0}
        />
        <NavigationLink
          to="Alerts"
          icon="bell"
          label={t("nav-alerts")}
          currentRoute={props.currentRoute}
          count={pending("Alerts")}
        />
      </div>
      <div class="console-nav__group">
        <div class="console-nav__head">{t("console-nav-auction-ops")}</div>
        <NavigationLink
          to="Auctions"
          icon="gavel"
          label={t("nav-auctions")}
          currentRoute={props.currentRoute}
          count={0}
        />
        {/* Admin 会话和当前状态读取未提供寄售开关，认证后的历史入口保持可用。 */}
        <NavigationLink
          to="Consignment"
          icon="file"
          label={t("consignment-admin-title")}
          currentRoute={props.currentRoute}
          count={0}
        />
        <NavigationLink
          to="Deliveries"
          icon="send"
          label={t("nav-deliveries")}
          currentRoute={props.currentRoute}
          count={pending("Deliveries")}
        />
        <NavigationLink
          to="Settlement"
          icon="check"
          label={t("nav-settlement")}
          currentRoute={props.currentRoute}
          count={pending("Settlement")}
        />
        <NavigationLink
          to="SettlementReturns"
          icon="return"
          label={t("nav-settlement-returns")}
          currentRoute={props.currentRoute}
          count={pending("SettlementReturns")}
        />
        <NavigationLink
          to="Mails"
          icon="inbox"
          label={t("nav-mails")}
          currentRoute={props.currentRoute}
          count={0}
        />
        <NavigationLink
          to="Users"
          icon="users"
          label={t("nav-users")}
          currentRoute={props.currentRoute}
          count={0}
        />
        <NavigationLink
          to="Cookies"
          icon="cookie"
          label={t("nav-cookies")}
          currentRoute={props.currentRoute}
          count={0}
        />
        <NavigationLink
          to="Renames"
          icon="edit"
          label={t("nav-renames")}
          currentRoute={props.currentRoute}
          count={0}
        />
        <NavigationLink
          to="PlayerSupport"
          icon="message"
          label={t("nav-support")}
          currentRoute={props.currentRoute}
          count={0}
        />
      </div>
      <div class="console-nav__group">
        <div class="console-nav__head">{t("console-nav-system")}</div>
        <NavigationLink
          to="Outbox"
          icon="send"
          label={t("page-outbox-title")}
          currentRoute={props.currentRoute}
          count={0}
        />
        <NavigationLink
          to="Config"
          icon="settings"
          label={t("nav-config")}
          currentRoute={props.currentRoute}
          count={0}
        />
        <NavigationLink
          to="RulesContent"
          icon="file"
          label={t("page-rules-content-title")}
          currentRoute={props.currentRoute}
          count={0}
        />
        <NavigationLink
          to="AboutContent"
          icon="file"
          label={t("page-about-title")}
          currentRoute={props.currentRoute}
          count={0}
        />
        <NavigationLink
          to="Audit"
          icon="shield"
          label={t("nav-audit")}
          currentRoute={props.currentRoute}
          count={0}
        />
      </div>
    </div>
  );
}
