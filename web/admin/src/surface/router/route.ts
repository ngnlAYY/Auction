// Admin 路由表：Rust `Route` 枚举对应的路径与参数名；页面入口组件由同级 `index` 模块定义。
// 路径契约见 `docs/contracts/routes.md`。管理命令的实现必须走版本化 REST 契约，
// 不在浏览器端重建状态机。

import { lazyPage, type RouteDefinition } from "@auction/web-shared/router";

/** 管理端路由名；含仅 Solid 支持的寄售页，入口组件按名字查页面表。 */
export type AdminRouteName =
  | "Setup"
  | "Dashboard"
  | "DashboardAlias"
  | "Login"
  | "Auctions"
  | "AuctionLots"
  | "AuctionOverview"
  | "Consignment"
  | "ConsignmentListing"
  | "ConsignmentQuarantine"
  | "ConsignmentOrders"
  | "ConsignmentOrderDetail"
  | "ConsignmentLoss"
  | "Mails"
  | "MailOverview"
  | "Cookies"
  | "Credentials"
  | "Users"
  | "Renames"
  | "Deliveries"
  | "DeliveryDetail"
  | "DeliveryOverview"
  | "PlayerSupport"
  | "Outbox"
  | "Settlement"
  | "SettlementReturns"
  | "SettlementReturnDetail"
  | "Config"
  | "RulesContent"
  | "Rules"
  | "AboutContent"
  | "Alerts"
  | "Audit"
  | "Status";

// 路径字面量必须与 auction-http/src/assets/routes.rs 的 Admin 清单同步，未知路径不能获得 SPA 入口。
// `/setup` 在认证外壳之外，其余页面都由 `AdminShell` 渲染。
export const ADMIN_ROUTES: readonly RouteDefinition[] = [
  { name: "Setup", pattern: "/setup" },
  { name: "Dashboard", pattern: "/" },
  { name: "DashboardAlias", pattern: "/dashboard" },
  { name: "Login", pattern: "/login" },
  { name: "Auctions", pattern: "/auctions" },
  // 单场次的拍品清单；场次标识由路径段解出后交给页面。
  { name: "AuctionLots", pattern: "/auctions/:auction_id/lots" },
  { name: "AuctionOverview", pattern: "/auction" },
  { name: "Consignment", pattern: "/consignment" },
  { name: "ConsignmentListing", pattern: "/consignment/listings/:listing_id" },
  // Solid 版专属的邮件隔离详情；运营人员可按隔离 ID 直接打开。
  { name: "ConsignmentQuarantine", pattern: "/consignment/deposits/:quarantine_id" },
  // Solid 版专属的订单列表、订单详情与损失案件；均在 `/consignment` 独立入口前缀下。
  { name: "ConsignmentOrders", pattern: "/consignment/orders" },
  { name: "ConsignmentOrderDetail", pattern: "/consignment/orders/:contract_id" },
  { name: "ConsignmentLoss", pattern: "/consignment/positions/:position_id/loss-case" },
  { name: "Mails", pattern: "/mails" },
  { name: "MailOverview", pattern: "/mail" },
  { name: "Cookies", pattern: "/cookies" },
  { name: "Credentials", pattern: "/credentials" },
  { name: "Users", pattern: "/users" },
  { name: "Renames", pattern: "/renames" },
  { name: "Deliveries", pattern: "/deliveries" },
  // 单条投递的只读证据详情；玩家报出的投递 Reference 可直接定位。
  { name: "DeliveryDetail", pattern: "/deliveries/:delivery_id" },
  { name: "DeliveryOverview", pattern: "/delivery" },
  // 玩家支持：按用户或参考编号只读汇总，一次检索定位相关记录。
  { name: "PlayerSupport", pattern: "/player-support" },
  { name: "Outbox", pattern: "/outbox" },
  { name: "Settlement", pattern: "/settlement" },
  // 结算退件：被拒收或过期退回的货款、退款与流拍/撤拍寄回，按案件处理。
  { name: "SettlementReturns", pattern: "/settlement-returns" },
  // 单个退件案件：资产位置、处理动作与阻断原因、领回、代管、派发链与事件。
  { name: "SettlementReturnDetail", pattern: "/settlement-returns/:return_id" },
  { name: "Config", pattern: "/config" },
  { name: "RulesContent", pattern: "/rules-content" },
  { name: "Rules", pattern: "/rules" },
  // “关于”文档：公开端“关于”页面的多语言内容，与规则共用同一套编辑与发布流程。
  { name: "AboutContent", pattern: "/about-content" },
  // 告警台：活动/历史告警、确认、静默与处置手册锚点。
  { name: "Alerts", pattern: "/alerts" },
  { name: "Audit", pattern: "/audit" },
  { name: "Status", pattern: "/status" },
];
// Dioxus 的 `/:..segments` 未匹配路由由共享路由器直接返回 not-found，不重复登记 catch-all。

/** 只有 Solid 提供寄售运营列表，进入路由后才载入。 */
export const consignmentPage = lazyPage(() =>
  import("../consignment/list").then(({ page }) => page),
);

/** 只有 Solid 提供寄售运营详情，进入路由后才载入。 */
export const consignmentListingPage = lazyPage(() =>
  import("../consignment/detail").then(({ detailPage }) => detailPage),
);
