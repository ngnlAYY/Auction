//! Public 路由表与路由到领域页面的薄包装。

import { createValueMemo } from "@auction/web-shared/reactive/memo";
import type {
  RouteDefinition,
  RoutePage,
  RoutePageComponent,
  RoutePageLoader,
  RoutePageProps,
} from "@auction/web-shared/router";
import { lazyPage } from "@auction/web-shared/router";
import type { JSX } from "@solidjs/web";
import { createComponent, Show } from "solid-js";

import { redirectPublicAlias } from "../query";

const NIL_UUID = "00000000-0000-0000-0000-000000000000";

/** 接受 uuid crate 的简单、连字符、花括号与 URN 形式，并输出规范小写连字符 UUID。 */
function canonicalUuid(value: string): string | null {
  let hyphenated = value;
  if (value.length === 38 && value.startsWith("{") && value.endsWith("}")) {
    hyphenated = value.slice(1, -1);
  } else if (value.length === 45 && value.slice(0, 9).toLowerCase() === "urn:uuid:") {
    hyphenated = value.slice(9);
  } else if (value.length === 32 && /^[0-9a-fA-F]{32}$/u.test(value)) {
    hyphenated = `${value.slice(0, 8)}-${value.slice(8, 12)}-${value.slice(12, 16)}-${value.slice(16, 20)}-${value.slice(20)}`;
  }
  if (
    !/^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$/u.test(
      hyphenated,
    )
  ) {
    return null;
  }
  return hyphenated.toLowerCase();
}

/** 路径逐条登记已发布的公开端 route（保持 URL 兼容）与经评审的 Public 扩展。 */
export const PUBLIC_ROUTES: readonly RouteDefinition[] = [
  { name: "AuctionIndex", pattern: "/" },
  { name: "AuctionPage", pattern: "/auction" },
  { name: "AuctionList", pattern: "/auctions" },
  { name: "LotDetail", pattern: "/lots/:lot_code" },
  { name: "BidLog", pattern: "/bidlog" },
  { name: "Fulfillment", pattern: "/fulfillment" },
  { name: "History", pattern: "/history" },
  { name: "Login", pattern: "/login", query: ["return_to"] },
  { name: "Me", pattern: "/me" },
  { name: "Account", pattern: "/account" },
  { name: "Rules", pattern: "/rules" },
  { name: "About", pattern: "/about" },
  { name: "Blacklist", pattern: "/blacklist" },
  { name: "CompensationPool", pattern: "/compensation-pool" },
  { name: "Compensation", pattern: "/compensation" },
  { name: "Forum", pattern: "/forum" },
  { name: "Status", pattern: "/status" },
  // Solid 版专属：语言与主题的设置页，Rust 版没有对应路由。
  { name: "Settings", pattern: "/settings" },
  { name: "ConsignmentCatalog", pattern: "/consignment" },
  { name: "ConsignmentDetail", pattern: "/consignment/listings/:listing_id" },
  { name: "ConsignmentRegistration", pattern: "/consignment/new" },
  { name: "ConsignmentMine", pattern: "/consignment/me" },
  // Solid 版专属的本人寄件指引；必须先于通用挂单详情匹配。
  {
    name: "ConsignmentDepositInstructions",
    pattern: "/consignment/me/:listing_id/deposit-instructions",
  },
  // Solid 版专属的重列页；必须先于通用本人挂单详情登记。
  { name: "ConsignmentRelist", pattern: "/consignment/me/:listing_id/relist" },
  { name: "ConsignmentOwnedDetail", pattern: "/consignment/me/:listing_id" },
  // 第 6、7 期的 Solid 版专属页面：购买确认、本人订单、受限权利与损失回执，全部在独立入口前缀下。
  { name: "ConsignmentPurchase", pattern: "/consignment/listings/:listing_id/purchase" },
  { name: "ConsignmentOrders", pattern: "/consignment/orders" },
  { name: "ConsignmentOrderDetail", pattern: "/consignment/orders/:contract_id" },
  { name: "ConsignmentRights", pattern: "/consignment/rights" },
  { name: "ConsignmentLossReceipt", pattern: "/consignment/loss/:position_id" },
];
// Dioxus 的 `/:..segments` 未匹配路由由共享路由器直接返回 not-found，不重复登记 catch-all。

const loadBidLog: RoutePageLoader = () =>
  import("../reads/bid-log").then((module) => module.BidLog);
const loadFulfillment: RoutePageLoader = () =>
  import("../reads/fulfillment").then((module) => module.Fulfillment);
const loadHistory: RoutePageLoader = () =>
  import("../reads/history").then((module) => module.History);
const loadLogin: RoutePageLoader = () =>
  import("../session/login").then(
    (module) => (props: RoutePageProps) =>
      module.login({
        get returnTo() {
          return props.query.return_to ?? null;
        },
      }),
  );
const loadMe: RoutePageLoader = () => import("../account").then((module) => module.page);
// 别名沿用异步路由交付，跳转使用入口已加载的查询模块。
const loadAccount: RoutePageLoader = async () => () => redirectPublicAlias("/me");
const loadRules: RoutePageLoader = () =>
  import("../rules").then((module) => () => module.page("rules"));
const loadAbout: RoutePageLoader = () =>
  import("../rules").then((module) => () => module.page("about"));
const loadBlacklist: RoutePageLoader = () => import("../blacklist").then((module) => module.page);
const loadCompensationPool: RoutePageLoader = () =>
  import("../compensation").then((module) => module.poolPage);
const loadCompensation: RoutePageLoader = async () => () =>
  redirectPublicAlias("/compensation-pool");
const loadForum: RoutePageLoader = () => import("../forum").then((module) => module.page);
const loadStatus: RoutePageLoader = () => import("../pages/status").then((module) => module.page);
const loadSettings: RoutePageLoader = () => import("../settings").then((module) => module.page);

/** 拍品直达链接：UUID 解析成目录位置并打开详情抽屉，其余取值回到拍卖页。 */
const loadLotDetail: RoutePageLoader = () =>
  Promise.all([import("../catalog/detail/deep-link"), import("../board")]).then(
    ([deepLink, board]): RoutePageComponent =>
      (props) => {
        const lotId = createValueMemo(() => {
          const canonical = canonicalUuid(props.params.lot_code ?? "");
          return canonical === NIL_UUID ? null : canonical;
        });
        return Show({
          keyed: true,
          get when() {
            return lotId();
          },
          get fallback() {
            return createComponent(board.AuctionPage, {});
          },
          children: (id: string): JSX.Element =>
            createComponent(deepLink.LotDeepLink, { lotId: id }),
        });
      },
  );

export const PUBLIC_PAGES: Readonly<Record<string, RoutePage>> = {
  AuctionIndex: lazyPage(() => import("../board").then((module) => module.AuctionIndex)),
  AuctionPage: lazyPage(() => import("../board").then((module) => module.AuctionPage)),
  AuctionList: lazyPage(() => import("../pages/history").then((module) => module.AuctionList)),
  LotDetail: lazyPage(loadLotDetail),
  BidLog: lazyPage(loadBidLog),
  Fulfillment: lazyPage(loadFulfillment),
  History: lazyPage(loadHistory),
  Login: lazyPage(loadLogin),
  Me: lazyPage(loadMe),
  Account: lazyPage(loadAccount),
  Rules: lazyPage(loadRules),
  About: lazyPage(loadAbout),
  Blacklist: lazyPage(loadBlacklist),
  CompensationPool: lazyPage(loadCompensationPool),
  Compensation: lazyPage(loadCompensation),
  Forum: lazyPage(loadForum),
  Status: lazyPage(loadStatus),
  Settings: lazyPage(loadSettings),
  ConsignmentCatalog: lazyPage(() =>
    import("../consignment/catalog").then((module) => module.ConsignmentCatalog),
  ),
  ConsignmentDetail: lazyPage(() =>
    import("../consignment/detail").then((module) => module.PublicDetail),
  ),
  ConsignmentRegistration: lazyPage(() =>
    import("../consignment/registration").then((module) => module.RegistrationForm),
  ),
  ConsignmentMine: lazyPage(() =>
    import("../consignment/owned").then((module) => module.MyListings),
  ),
  ConsignmentDepositInstructions: lazyPage(() =>
    import("../consignment/deposit-instructions").then((module) => module.DepositInstructionsPage),
  ),
  ConsignmentRelist: lazyPage(() =>
    import("../consignment/relist").then((module) => module.RelistPage),
  ),
  ConsignmentOwnedDetail: lazyPage(() =>
    import("../consignment/owned-detail").then((module) => module.OwnedDetail),
  ),
  ConsignmentPurchase: lazyPage(() =>
    import("../consignment/purchase-confirm").then((module) => module.PurchaseConfirm),
  ),
  ConsignmentOrders: lazyPage(() =>
    import("../consignment/orders").then((module) => module.MyOrders),
  ),
  ConsignmentOrderDetail: lazyPage(() =>
    import("../consignment/order-detail").then((module) => module.OrderDetail),
  ),
  ConsignmentRights: lazyPage(() =>
    import("../consignment/rights").then((module) => module.RightsPage),
  ),
  ConsignmentLossReceipt: lazyPage(() =>
    import("../consignment/loss-receipt").then((module) => module.LossReceiptPage),
  ),
};

/** 未匹配任何已冻结路由时回到公开端首页，保留旧 URL 中的筛选查询。 */
export function NotFoundPage(): JSX.Element {
  return redirectPublicAlias("/");
}
