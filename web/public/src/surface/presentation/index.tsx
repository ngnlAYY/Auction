//! 设备专属纯展示入口；页面数据、表单草稿与命令始终由原有稳定宿主持有。
//! 两种控制视图由 CSS 选择，尺寸变化不建立资源、监听器或新的命令生命周期。

import { Link } from "@auction/web-shared/router";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { Show, useContext } from "solid-js";

import { PublicSessionResourceContext } from "..";
import { LOGIN_ROUTE_HREF } from "../session/login-return";

/** 导航目标与首页查询动作只维护一份；两端视图不复制会话或世界选择器。 */
export function NavigationLinks(props: {
  auctionHomeHref: string;
  resetHomeQuery: boolean;
  onHome: () => void;
}): JSX.Element {
  const session = useContext(PublicSessionResourceContext);
  const authenticated = () => {
    const result = session();
    return result?.ok === true && result.value?.authenticated === true;
  };
  return (
    <>
      <Link
        href={props.auctionHomeHref}
        onClickOnly={props.resetHomeQuery}
        onClick={() => props.onHome()}
      >
        {t("nav-auctions")}
      </Link>
      <Link href="/bidlog">{t("page-bidlog-title")}</Link>
      <Link href="/fulfillment">{t("page-fulfillment-title")}</Link>
      <Link href="/auctions">{t("nav-auction-history")}</Link>
      <Link href="/history">{t("nav-price-history")}</Link>
      <Link href="/rules">{t("page-rules-title")}</Link>
      <Link href="/about">{t("page-about-title")}</Link>
      <Link href="/blacklist">{t("nav-blacklist")}</Link>
      <Link href="/compensation-pool">{t("page-compensation-pool-title")}</Link>
      <Link href="/forum">{t("nav-forum")}</Link>
      <Show
        when={authenticated()}
        fallback={<Link href={LOGIN_ROUTE_HREF}>{t("session-login")}</Link>}
      >
        <Link href="/account">{t("nav-my-bids")}</Link>
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
