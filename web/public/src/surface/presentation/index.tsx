//! 设备专属纯展示入口；页面数据、表单草稿与命令始终由原有稳定宿主持有。
//! 两种控制视图由 CSS 选择，尺寸变化不建立资源、监听器或新的命令生命周期。

import { Link, type RouteMatch } from "@auction/web-shared/router";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { Show, useContext } from "solid-js";

import { PublicSessionResourceContext } from "..";
import { consignmentEnabled, useConsignmentCapability } from "../consignment/capability";
import { LOGIN_ROUTE_HREF } from "../session/login-return";

/** 导航目标与首页查询动作只维护一份；Solid 版的世界范围由外壳传入的链接生成器统一携带。 */
export function NavigationLinks(props: {
  auctionHomeHref: string;
  currentRoute: RouteMatch;
  scopedHref: (path: string) => string;
  resetHomeQuery: boolean;
  onHome: () => void;
}): JSX.Element {
  const session = useContext(PublicSessionResourceContext);
  const consignment = useConsignmentCapability();
  const authenticated = () => {
    const result = session();
    return result?.ok === true && result.value?.authenticated === true;
  };
  const isCurrent = (...names: readonly string[]): boolean => {
    const route = props.currentRoute;
    return route.kind === "matched" && names.includes(route.name);
  };
  return (
    <>
      <Link
        href={props.auctionHomeHref}
        aria-current={isCurrent("AuctionIndex", "AuctionPage") ? "page" : "false"}
        onClickOnly={props.resetHomeQuery}
        onClick={() => props.onHome()}
      >
        {t("nav-auctions")}
      </Link>
      <Show when={consignmentEnabled(consignment())}>
        <Link
          href="/consignment"
          aria-current={isCurrent("ConsignmentCatalog", "ConsignmentDetail") ? "page" : "false"}
        >
          {t("consignment-nav")}
        </Link>
      </Show>
      <Link
        href={props.scopedHref("/bidlog")}
        aria-current={isCurrent("BidLog") ? "page" : "false"}
      >
        {t("page-bidlog-title")}
      </Link>
      <Link
        href={props.scopedHref("/fulfillment")}
        aria-current={isCurrent("Fulfillment") ? "page" : "false"}
      >
        {t("page-fulfillment-title")}
      </Link>
      <Link
        href={props.scopedHref("/auctions")}
        aria-current={isCurrent("AuctionList") ? "page" : "false"}
      >
        {t("nav-auction-history")}
      </Link>
      <Link
        href={props.scopedHref("/history")}
        aria-current={isCurrent("History") ? "page" : "false"}
      >
        {t("nav-price-history")}
      </Link>
      <Link href={props.scopedHref("/rules")} aria-current={isCurrent("Rules") ? "page" : "false"}>
        {t("page-rules-title")}
      </Link>
      <Link href={props.scopedHref("/about")} aria-current={isCurrent("About") ? "page" : "false"}>
        {t("page-about-title")}
      </Link>
      <Link
        href={props.scopedHref("/blacklist")}
        aria-current={isCurrent("Blacklist") ? "page" : "false"}
      >
        {t("nav-blacklist")}
      </Link>
      <Link
        href={props.scopedHref("/compensation-pool")}
        aria-current={isCurrent("CompensationPool") ? "page" : "false"}
      >
        {t("page-compensation-pool-title")}
      </Link>
      <Link href={props.scopedHref("/forum")} aria-current={isCurrent("Forum") ? "page" : "false"}>
        {t("nav-forum")}
      </Link>
      <Show
        when={authenticated()}
        fallback={
          <Link
            href={props.scopedHref(LOGIN_ROUTE_HREF)}
            aria-current={isCurrent("Login") ? "page" : "false"}
          >
            {t("session-login")}
          </Link>
        }
      >
        <Link
          href={props.scopedHref("/account")}
          aria-current={isCurrent("Me", "Account") ? "page" : "false"}
        >
          {t("nav-my-bids")}
        </Link>
      </Show>
    </>
  );
}

/** 同页锚点不会导航 Router，不卸载偏好表单、分页或待核对命令。 */
export function AccountSectionLinks(): JSX.Element {
  return (
    <>
      <a href="#outbid-notifications">{t("outbid-title")}</a>
      <a href="#login-duration">{t("session-duration-title")}</a>
      <a href="#account-watches-title">{t("account-watches-title")}</a>
      <a href="#account-bids-title">{t("account-bids-title")}</a>
      <a href="#account-proxies-title">{t("account-proxies-title")}</a>
    </>
  );
}
