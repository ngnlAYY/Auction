//! Public 账户当前标记装备、竞价与代理页面。
//! 页面只展示服务端已经完成权限过滤的账户 projection。普通竞价与代理授权分别使用旧版的
//! 独立页码，查询状态保留在 URL 中，登录后由服务端快照重新读取。

import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import { Link } from "@auction/web-shared/router";
import { TableColumns } from "@auction/web-shared/ui/components/table-columns";
import { Time } from "@auction/web-shared/ui/components/time";
import { TruncatedText } from "@auction/web-shared/ui/components/truncated-text";
import { formatCreditAmount } from "@auction/web-shared/ui/format";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Match, Show, Switch, useContext } from "solid-js";

import { fetchMyBids, type MyBidsPage, type OwnedBid, type OwnedProxy } from "../../api/account";
import { lotCodeFor } from "../../api/model";
import { PublicRefreshGenerationContext, useBidRefresh } from "../board/refresh";
import {
  LotDetailLookupButton,
  LotDetailLookupContext,
  LotDetailLookupHost,
  useLotDetailLookup,
} from "../catalog/detail/lookup";
import { PublicCommandOwnerContext, PublicSessionResourceContext } from "../index";
import { useContentLoading } from "../loading";
import { PublicPager } from "../pagination";
import { usePublicPageBounds } from "../pagination/bounds";
import { AccountNavigation } from "../presentation/mobile";
import {
  proxyModeLabel,
  proxyQuotaLabel,
  proxyStatusLabel,
  proxyTransitionSummary,
} from "../proxy";
import { updatePublicQueryState, usePublicQueryState } from "../query";
import { updatePublicQueryPairs } from "../query/location";
import { usePageRouteGuard } from "../query/route-page";
import { loginHrefForCurrentLocation } from "../session";
import { SessionPreference } from "../session-preference";
import { OutbidNotifications } from "../watch-preference";
import { CurrentWatches } from "./watches";

const DEFAULT_ACCOUNT_PAGE_SIZE = 10;

/** 账户页入口；会话未确认时不挂载受保护的竞价、代理与标记读取。 */
export function page(): JSX.Element {
  const session = useContext(PublicSessionResourceContext);
  const commandOwner = useContext(PublicCommandOwnerContext);
  const sessionSnapshot = () => session();
  const sessionFailed = () => session()?.ok === false;
  const loginRequired = () => {
    const snapshot = session();
    return snapshot?.ok === true && (snapshot.value === null || !snapshot.value.authenticated);
  };
  const identityUsers = createValueMemo((): readonly string[] => {
    const snapshot = session();
    return snapshot?.ok === true && snapshot.value !== null && snapshot.value.authenticated
      ? [snapshot.value.user_id]
      : [];
  });
  const preferenceOwners = createValueMemo((): readonly string[] => {
    const owner = commandOwner();
    return owner === null ? [] : [owner];
  });
  useContentLoading(() => sessionSnapshot() === undefined);
  return (
    <section class="public-account-bids-page" aria-label={t("page-account-title")}>
      <Show when={commandOwner() !== null}>
        <AccountNavigation />
      </Show>
      {/* 会话暂时读取失败只禁用新写；与竞价宿主一样，明确换号或登出才销毁未知命令。 */}
      <For each={preferenceOwners()} keyed={(userId) => `preferences-${userId}`}>
        {(userId) => <AccountPreferenceScope userId={userId()} />}
      </For>
      <Switch>
        <Match when={sessionSnapshot() === undefined}>{null}</Match>
        <Match when={sessionFailed()}>
          <div class="public-auction-empty" role="alert">
            <p class="public-auction-empty__title">{t("data-error")}</p>
            <button
              class="public-auction-empty__retry"
              type="button"
              onClick={() => session.restart()}
            >
              {t("data-retry")}
            </button>
          </div>
        </Match>
        <Match when={loginRequired()}>
          <div class="public-account-login-prompt" role="status">
            <p>{t("account-login-required")}</p>
            <Link class="public-account-login-cta" href={loginHrefForCurrentLocation()}>
              {t("account-login-cta")}
            </Link>
          </div>
        </Match>
        <Match when={identityUsers().length > 0}>
          <For each={identityUsers()} keyed={(userId) => `account-${userId}`}>
            {(userId) => <AccountReadScope userId={userId()} />}
          </For>
        </Match>
      </Switch>
    </section>
  );
}

/** 同一稳定账号的两个偏好命令共用 keyed 宿主；布局容器不依赖宽度或会话读取分支。 */
function AccountPreferenceScope(props: { readonly userId: string }): JSX.Element {
  return (
    <div class="public-account-preferences">
      <OutbidNotifications userId={props.userId} />
      <SessionPreference userId={props.userId} />
    </div>
  );
}

/** 整组只读列表以账号隔离；换号时同时重建标记及竞价读取，避免复用另一账号的快照。 */
function AccountReadScope(props: { readonly userId: string }): JSX.Element {
  // 标记、出价与代理三张列表共用一个只读详情抽屉；宿主在各自的读取分支之外，
  // 换页或重读进入加载分支时已打开的抽屉不被卸载。
  const lookup = useLotDetailLookup();
  return (
    <LotDetailLookupContext value={lookup}>
      <CurrentWatches userId={props.userId} />
      <AuthenticatedAccountData userId={props.userId} />
      <Show when={lookup.failed()}>
        <p class="action-feedback action-feedback--error" role="alert">
          {t("data-error")}
        </p>
      </Show>
      <LotDetailLookupHost lookup={lookup} />
    </LotDetailLookupContext>
  );
}

/** 只有共享 session 已确认认证后才挂载受保护列表资源，对齐旧版请求序列。 */
function AuthenticatedAccountData(props: { readonly userId: string }): JSX.Element {
  const refreshGeneration = useContext(PublicRefreshGenerationContext);
  const accountReloadGeneration = createCell(0);
  // 竞价与代理页码与地址栏双向同步。
  const queryState = usePublicQueryState();
  const queryStateSignal = queryState.state;
  const bidPage = createValueMemo(() => queryStateSignal().bidPage);
  const proxyPage = createValueMemo(() => queryStateSignal().proxyPage);
  // /me/bids 的两个列表固定使用 10 条，任一页越界都会使整个请求失败。
  const bounds = usePublicPageBounds(queryStateSignal, "Me", () => [
    ["bidPage", 10],
    ["proxyPage", 10],
  ]);
  const routeStillOnPage = usePageRouteGuard("Me");
  const boundsFailed = () => bounds.failed("bidPage") || bounds.failed("proxyPage");
  const account = createTaskResource(
    () => ({
      refresh: refreshGeneration(),
      reload: accountReloadGeneration(),
      bidPage: bidPage(),
      proxyPage: proxyPage(),
      userId: props.userId,
      blocked: bounds.blocked("bidPage") || bounds.blocked("proxyPage"),
    }),
    async (dependencies, context) => {
      if (dependencies.blocked) {
        return null;
      }
      const requested = [dependencies.bidPage, dependencies.proxyPage] as const;
      const result = await fetchMyBids(
        dependencies.userId,
        dependencies.bidPage,
        dependencies.proxyPage,
        context.signal,
      );
      if (!context.alive()) {
        return null;
      }
      return { requested, result };
    },
  );
  useBidRefresh(account, () => true);

  const current = createValueMemo(() => {
    const loaded = account();
    if (loaded == null || bounds.blocked("bidPage") || bounds.blocked("proxyPage")) {
      return undefined;
    }
    return loaded.requested[0] === bidPage() && loaded.requested[1] === proxyPage()
      ? loaded.result
      : undefined;
  });
  const currentFailed = () => boundsFailed() || current()?.ok === false;
  const currentPageMismatch = () => {
    const result = current();
    return (
      result?.ok === true &&
      (result.value.bid_page !== bidPage() || result.value.proxy_page !== proxyPage())
    );
  };
  const currentPage = createValueMemo((): MyBidsPage | null => {
    const result = current();
    return result?.ok === true &&
      result.value.bid_page === bidPage() &&
      result.value.proxy_page === proxyPage()
      ? result.value
      : null;
  });
  // 没有当前页码的结果（含等待页码纠偏后的重读）时由顶栏提示加载，正文留空。
  useContentLoading(() => {
    if (boundsFailed()) {
      return false;
    }
    const result = current();
    if (result === undefined) {
      return true;
    }
    if (!result.ok) {
      return false;
    }
    return result.value.bid_page !== bidPage() || result.value.proxy_page !== proxyPage();
  });
  const retryAccount = () => {
    if (boundsFailed()) {
      bounds.retry();
      return;
    }
    accountReloadGeneration.set(
      Math.min(accountReloadGeneration.peek() + 1, Number.MAX_SAFE_INTEGER),
    );
  };
  return (
    <Switch>
      <Match when={currentFailed()}>
        <div>
          <div class="public-auction-empty" role="alert">
            <p class="public-auction-empty__title">{t("data-error")}</p>
            <button class="public-auction-empty__retry" type="button" onClick={retryAccount}>
              {t("data-retry")}
            </button>
          </div>
        </div>
      </Match>
      <Match when={current() === undefined}>{null}</Match>
      <Match when={currentPageMismatch()}>{null}</Match>
      <Match when={currentPage()}>
        {(pageData) => (
          <AccountPage
            page={pageData()}
            bidPage={bidPage()}
            proxyPage={proxyPage()}
            onBidPageChange={(nextPage) => {
              return updatePublicQueryState(
                queryStateSignal,
                (query) => {
                  query.bidPage = nextPage;
                },
                "push",
              );
            }}
            onBidPageClamp={(nextPage) => {
              if (!routeStillOnPage()) {
                return false;
              }
              return updatePublicQueryPairs(
                queryStateSignal,
                ["bid_page"],
                nextPage > 1 ? [["bid_page", String(nextPage)]] : [],
                "replace",
              );
            }}
            onProxyPageChange={(nextPage) => {
              return updatePublicQueryState(
                queryStateSignal,
                (query) => {
                  query.proxyPage = nextPage;
                },
                "push",
              );
            }}
            onProxyPageClamp={(nextPage) => {
              if (!routeStillOnPage()) {
                return false;
              }
              return updatePublicQueryPairs(
                queryStateSignal,
                ["proxy_page"],
                nextPage > 1 ? [["proxy_page", String(nextPage)]] : [],
                "replace",
              );
            }}
          />
        )}
      </Match>
    </Switch>
  );
}

interface AccountPageProps {
  readonly page: MyBidsPage;
  readonly bidPage: number;
  readonly proxyPage: number;
  readonly onBidPageChange: (page: number) => boolean;
  readonly onBidPageClamp: (page: number) => boolean;
  readonly onProxyPageChange: (page: number) => boolean;
  readonly onProxyPageClamp: (page: number) => boolean;
}

function AccountPage(props: AccountPageProps): JSX.Element {
  const pageSize = () =>
    props.page.page_size > 0 ? props.page.page_size : DEFAULT_ACCOUNT_PAGE_SIZE;
  return (
    <div class="public-account-activity">
      <BidSection
        bids={props.page.bids}
        page={props.bidPage}
        total={props.page.bid_total}
        pageSize={pageSize()}
        onPageChange={props.onBidPageChange}
        onPageClamp={props.onBidPageClamp}
      />
      <ProxySection
        proxies={props.page.proxies}
        page={props.proxyPage}
        total={props.page.proxy_total}
        pageSize={pageSize()}
        onPageChange={props.onProxyPageChange}
        onPageClamp={props.onProxyPageClamp}
      />
    </div>
  );
}

interface BidSectionProps {
  readonly bids: readonly OwnedBid[];
  readonly page: number;
  readonly total: number;
  readonly pageSize: number;
  readonly onPageChange: (page: number) => boolean;
  readonly onPageClamp: (page: number) => boolean;
}

/** 同一拍品的多笔出价分别标识，刷新时查看按钮随各自的记录保留。 */
export function ownedBidRowKey(bid: Pick<OwnedBid, "id">): string {
  return bid.id;
}

function BidSection(props: BidSectionProps): JSX.Element {
  return (
    <section class="public-account-section" aria-labelledby="account-bids-title">
      <h2 id="account-bids-title">{t("account-bids-title")}</h2>
      <Show
        when={props.bids.length > 0}
        fallback={<p class="public-account-empty">{t("account-no-bids")}</p>}
      >
        <div class="public-account-table-wrap">
          <table class="public-account-table" aria-label={t("account-bids-title")}>
            <TableColumns layout="public-account-bids" widths={[12, 42, 16, 22, 8]} />
            <thead>
              <tr>
                <th scope="col">{t("account-lot")}</th>
                <th scope="col">{t("account-watches-equipment")}</th>
                <th scope="col" class="numeric-column public-account-number">
                  {t("account-amount")}
                </th>
                <th scope="col">{t("account-time")}</th>
                <th scope="col" class="public-lot-view-cell">
                  {t("catalog-actions")}
                </th>
              </tr>
            </thead>
            <tbody>
              <For each={props.bids} keyed={ownedBidRowKey}>
                {(bid) => <BidRow bid={bid()} />}
              </For>
            </tbody>
          </table>
        </div>
      </Show>
      <PublicPager
        label={t("account-bids-title")}
        page={props.page}
        total={props.total}
        pageSize={props.pageSize}
        hideWhenSingle={true}
        showSizePicker={false}
        jumpLabel={t("catalog-page-input")}
        onPageChange={props.onPageChange}
        onPageClamp={props.onPageClamp}
        onPageSizeChange={() => {}}
      />
    </section>
  );
}

function BidRow(props: { readonly bid: OwnedBid }): JSX.Element {
  const rowClass = () => (props.bid.leading ? "public-account-leading" : "");
  const bidStatus = createValueMemo(() =>
    props.bid.leading ? t("account-bid-leading") : t("account-bid-outbid"),
  );
  const lotCode = createValueMemo(() =>
    lotCodeFor(props.bid.equipment.category, props.bid.lot_number),
  );
  return (
    <tr class={rowClass()} aria-description={bidStatus()}>
      {/* 可见的领先状态由行底色表达；同一语义以行描述提供给非视觉用户。 */}
      {/* 编号只作标识；详情经操作栏的查看按钮在页内抽屉打开。 */}
      <td class="public-account-code" data-label={t("account-lot")}>
        {lotCode()}
      </td>
      {/* 名称外没有链接承接焦点，截断时由文本自身进入 Tab 顺序以查看完整名称。 */}
      <td data-label={t("account-watches-equipment")}>
        <TruncatedText value={props.bid.equipment.name} size="wide" portal={true} />
      </td>
      <td
        class="public-account-amount public-account-number numeric-column"
        data-label={t("account-amount")}
      >
        {formatCreditAmount(props.bid.amount)}
      </td>
      <td data-label={t("account-time")}>
        <Time value={props.bid.created_at} />
      </td>
      <td class="public-lot-view-cell" data-label={t("catalog-actions")}>
        <LotDetailLookupButton
          auctionId={props.bid.auction_id}
          lotId={props.bid.lot_id}
          lotCode={lotCode()}
        />
      </td>
    </tr>
  );
}

interface ProxySectionProps {
  readonly proxies: readonly OwnedProxy[];
  readonly page: number;
  readonly total: number;
  readonly pageSize: number;
  readonly onPageChange: (page: number) => boolean;
  readonly onPageClamp: (page: number) => boolean;
}

/** 代理状态变化可更新原行，授权更换后使用新的行。 */
export function ownedProxyRowKey(proxy: Pick<OwnedProxy, "authorization_id">): string {
  return proxy.authorization_id;
}

function ProxySection(props: ProxySectionProps): JSX.Element {
  return (
    <section class="public-account-section" aria-labelledby="account-proxies-title">
      <h2 id="account-proxies-title">{t("account-proxies-title")}</h2>
      <Show
        when={props.proxies.length > 0}
        fallback={<p class="public-account-empty">{t("account-no-proxies")}</p>}
      >
        <div class="public-account-table-wrap">
          <table class="public-account-table" aria-label={t("account-proxies-title")}>
            <TableColumns layout="public-account-proxies" widths={[10, 16, 34, 10, 10, 13, 7]} />
            <thead>
              <tr>
                <th scope="col">{t("account-lot")}</th>
                <th scope="col" class="numeric-column public-account-number">
                  {t("account-secret-maximum")}
                </th>
                <th scope="col">{t("account-transition-history")}</th>
                <th scope="col">{t("account-mode")}</th>
                <th scope="col">{t("account-proxy-uses")}</th>
                <th scope="col">{t("account-status")}</th>
                <th scope="col" class="public-lot-view-cell">
                  {t("catalog-actions")}
                </th>
              </tr>
            </thead>
            <tbody>
              <For each={props.proxies} keyed={ownedProxyRowKey}>
                {(proxy) => <ProxyRow proxy={proxy()} />}
              </For>
            </tbody>
          </table>
        </div>
      </Show>
      <PublicPager
        label={t("account-proxies-title")}
        page={props.page}
        total={props.total}
        pageSize={props.pageSize}
        hideWhenSingle={true}
        showSizePicker={false}
        jumpLabel={t("catalog-page-input")}
        onPageChange={props.onPageChange}
        onPageClamp={props.onPageClamp}
        onPageSizeChange={() => {}}
      />
    </section>
  );
}

function ProxyRow(props: { readonly proxy: OwnedProxy }): JSX.Element {
  const transitions = createValueMemo(() => proxyTransitionSummary(props.proxy));
  const quota = createValueMemo(() => proxyQuotaLabel(props.proxy));
  const lotCode = createValueMemo(() =>
    lotCodeFor(props.proxy.equipment.category, props.proxy.lot_number),
  );
  return (
    <tr>
      {/* 编号只作标识；详情经操作栏的查看按钮在页内抽屉打开。 */}
      <td class="public-account-code" data-label={t("account-lot")}>
        {lotCode()}
      </td>
      <td
        class="public-account-amount public-account-number numeric-column"
        data-label={t("account-secret-maximum")}
      >
        {formatCreditAmount(props.proxy.maximum_amount)}
      </td>
      <td data-label={t("account-transition-history")}>{transitions()}</td>
      <td data-label={t("account-mode")}>{proxyModeLabel(props.proxy.mode)}</td>
      <td data-label={t("account-proxy-uses")}>{quota()}</td>
      <td data-label={t("account-status")}>{proxyStatusLabel(props.proxy.status)}</td>
      <td class="public-lot-view-cell" data-label={t("catalog-actions")}>
        <LotDetailLookupButton
          auctionId={props.proxy.auction_id}
          lotId={props.proxy.lot_id}
          lotCode={lotCode()}
        />
      </td>
    </tr>
  );
}
