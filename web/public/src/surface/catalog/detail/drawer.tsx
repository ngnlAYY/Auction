// 目录拍品详情抽屉：右侧滑出，展示所选行的实时投影、装备属性与最近出价。
// 抽屉不重算出价资格、不创建 `command_id`：出价入口照行内操作栏写入的投影渲染，
// 点击后交给页面级命令弹窗。

import { MAX_PUBLIC_BID_AMOUNT } from "@auction/contracts/dto/public/commands";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import { err, ok } from "@auction/web-shared/result";
import { Link } from "@auction/web-shared/router";
import { formatCreditAmount } from "@auction/web-shared/ui/format";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Match, onCleanup, Show, Switch, useContext } from "solid-js";

import { fetchLotDetail, LOT_DETAIL_FETCH_ERROR } from "../../../api/queries/lots";
import { loginHrefForCurrentLocation } from "../../session";
import { LotWatchButton } from "../../watches/control";
import { detailBidReason } from "../actions";
import { CommandDialogContext } from "../command-scope";
import {
  catalogCategoryKey,
  catalogCategoryLabel,
  countdownClass,
  equipmentGroupLabel,
  isActivationKey,
  lotStatusClass,
  lotStatusLabel,
  useCountdownDisplay,
} from "../presentation";
import type { CatalogDetailInput, DetailWorld } from ".";
import { CatalogDetailDialogContext } from ".";
import { DetailBidHistoryDialog, DetailLatestBids } from "./bid-history";
import { DetailOwnProxy, ownProxy } from "./own-proxy";
import { DetailPriceHistoryDialog } from "./price-history";
import { formatOneDecimal, statRows } from "./stat-format";

/** 属性默认展示的条数；超过时提供“显示全部（N）”。 */
const COLLAPSED_STATS = 6;
const TITLE_ID = "public-catalog-detail-title";
const BID_REASON_ID = "public-catalog-detail-bid-reason";

/** 抽屉之上再开弹窗（出价、完整竞价历史、价格历史）时记下入口。弹窗关闭后焦点陷阱把焦点交给
 * 抽屉内带 autofocus 的控件，于是回到打开它的按钮；刚打开抽屉时落在关闭按钮。 */
type DetailFocus = "close" | "bid" | "history" | "price_history";

/** 详情抽屉宿主。挂在拍卖作用域内（标记上下文之下），拍卖切换即卸载并关闭抽屉。 */
export function CatalogDetailHost(props: { readonly auctionId: string }): JSX.Element {
  const detail = useContext(CatalogDetailDialogContext);
  // 身份宿主先于本宿主销毁时信号已不可写；这里的清理只负责拍卖切换。
  const cleanup = () => detail.set(null);
  onCleanup(cleanup);
  // 页面上下文中只有打开中的输入；用单元素 keyed 列表建立真正的拍品生命周期。
  const current = () => {
    const input = detail();
    return input === null ? [] : [input];
  };
  const world: DetailWorld = { kind: "selected" };
  return (
    <For each={current()} keyed={(input) => input.row.lotId}>
      {(input) => (
        <CatalogDetailDrawer
          auctionId={props.auctionId}
          input={input()}
          world={world}
          onClose={cleanup}
        />
      )}
    </For>
  );
}
/** 抽屉本体。展示状态归调用方所有：目录宿主清空页面级信号，只读入口清空自己的查找状态。 */
export function CatalogDetailDrawer(props: {
  readonly auctionId: string;
  readonly input: CatalogDetailInput;
  /** 价格历史按哪个世界查询；见 `DetailWorld`。 */
  readonly world: DetailWorld;
  readonly onClose: () => void;
}): JSX.Element {
  // 只读入口不在命令宿主之下，此时出价入口必为 `unavailable`，命令弹窗信号不会被用到。
  // `CommandDialogContext` 按 `try_use_context` 的口径以 null 作为“没有提供方”。
  const commandDialog = useContext(CommandDialogContext);
  const focus = createCell<DetailFocus>("close");
  const historyOpen = createCell(false);
  const priceHistoryOpen = createCell(false);
  const close = () => props.onClose();

  const row = createValueMemo(() => props.input.row);
  const bid = createValueMemo(() => props.input.bid);
  const isPending = createValueMemo(() => row().status === "pending");
  const isLive = createValueMemo(() => row().status === "live");
  const title = createValueMemo(() => row().name);
  const category = createValueMemo(() => catalogCategoryLabel(catalogCategoryKey(row().category)));
  const group = createValueMemo(() => equipmentGroupLabel(row().equipmentGroup));
  const status = createValueMemo(() => lotStatusLabel(row().status));
  const statusClass = createValueMemo(() => lotStatusClass(row().status));
  const startPrice = createValueMemo(() => formatCreditAmount(row().startPrice));
  const currentPrice = createValueMemo(() => formatCreditAmount(row().currentPrice));
  const nextBid = createValueMemo(() =>
    row().nextBid > MAX_PUBLIC_BID_AMOUNT
      ? t("bid-limit-reached")
      : formatCreditAmount(row().nextBid),
  );
  const leader = createValueMemo(() => row().leaderName ?? t("value-empty"));
  const primary = createValueMemo(() => row().primaryAttributeInitials ?? t("value-empty"));
  const avgRoll = createValueMemo(() => {
    const value = row().avgRoll;
    return value == null ? null : `${formatOneDecimal(value)}%`;
  });
  const level = createValueMemo(() => {
    const value = row().level;
    return value == null ? null : String(value);
  });
  const remaining = createValueMemo(() => (isLive() ? row().remaining : ""));
  const gameUrl = createValueMemo(() => row().gameEquipmentUrl);

  // 本人对该拍品的代理授权（上限、已用次数）；抽屉是它在目录里唯一的展示位置。
  const ownProxyValue = createValueMemo(() => ownProxy(bid()));
  // 出价入口三态与行内操作栏一致：命令按钮（可禁用并说明原因）、登录链接或不渲染。
  const bidReason = createValueMemo(() => detailBidReason(bid()));
  const commandBid = createValueMemo(() => {
    const current = bid();
    return current.kind === "command" ? current : null;
  });
  const entryLabel = createValueMemo(() => {
    const current = commandBid();
    if (current === null) {
      return "";
    }
    return current.input.action === "proxy" ? t("catalog-proxy") : t("catalog-bid");
  });

  return (
    <>
      {/* 抽屉没有未提交的输入，点遮罩即关闭；关闭会同步卸载抽屉，阻止事件继续冒泡到已丢弃的处理器。 */}
      {/* biome-ignore lint/a11y/noStaticElementInteractions: 遮罩只承担关闭点击，键盘关闭由抽屉根处理。 */}
      {/* biome-ignore lint/a11y/useKeyWithClickEvents: 抽屉根统一处理 Escape，遮罩不单独接收键盘。 */}
      <div
        class="public-catalog-detail__scrim"
        onClick={(event) => {
          event.stopPropagation();
          close();
        }}
      />
      <aside
        class="public-catalog-detail"
        role="dialog"
        aria-modal="true"
        aria-labelledby={TITLE_ID}
        tabindex="-1"
        onKeyDown={(event) => {
          if (event.key === "Escape") {
            event.preventDefault();
            event.stopPropagation();
            close();
          } else if (event.repeat && isActivationKey(event.key)) {
            // 在目录行上按住 Enter/Space 打开抽屉后，重复按键落到刚获得焦点的关闭按钮上；
            // 不拦住的话抽屉会被同一次按键立刻关掉。
            event.preventDefault();
          }
        }}
      >
        <header class="public-catalog-detail__head">
          <div id={TITLE_ID} class="public-catalog-detail__heading">
            <div class="public-catalog-detail__eyebrow">
              <span>{t("catalog-detail")}</span>
              <span class="public-catalog-detail__code">{row().code}</span>
            </div>
            <h2 class="public-catalog-detail__title" title={title()}>
              {title()}
            </h2>
          </div>
          <button
            class="public-catalog-detail__close"
            type="button"
            aria-label={t("catalog-close")}
            title={t("catalog-close")}
            autofocus={focus() === "close"}
            onClick={(event) => {
              event.stopPropagation();
              close();
            }}
          >
            <DetailCloseIcon />
          </button>
        </header>
        <div class="public-catalog-detail__body">
          <dl class="public-catalog-detail__facts">
            <div>
              <dt>{t("equipment-group")}</dt>
              <dd>{group()}</dd>
            </div>
            <div>
              <dt>{t("equipment-category")}</dt>
              <dd>{category()}</dd>
            </div>
            <Show when={level() !== null}>
              <div>
                <dt>{t("equipment-level")}</dt>
                <dd>{level()}</dd>
              </div>
            </Show>
            <div>
              <dt>{t("catalog-primary-attributes")}</dt>
              <dd>{primary()}</dd>
            </div>
            <Show when={avgRoll() !== null}>
              <div>
                <dt>{t("equipment-avg-roll")}</dt>
                <dd>{avgRoll()}</dd>
              </div>
            </Show>
            <div>
              <dt>{t("catalog-seller")}</dt>
              <dd>{row().sellerName}</dd>
            </div>
            <Show
              when={isPending()}
              fallback={
                <>
                  <div>
                    <dt>{t("catalog-leader")}</dt>
                    <dd>{leader()}</dd>
                  </div>
                  <div>
                    <dt>{t("catalog-current")}</dt>
                    <dd>{currentPrice()}</dd>
                  </div>
                </>
              }
            >
              {/* 预览阶段没有出价；目录投影里的当前价与下一口价都等于起拍价。 */}
              <div>
                <dt>{t("catalog-start-price")}</dt>
                <dd>{startPrice()}</dd>
              </div>
            </Show>
            <Show when={isLive()}>
              <div>
                <dt>{t("catalog-next-bid")}</dt>
                <dd>{nextBid()}</dd>
              </div>
            </Show>
            <Show when={remaining() !== ""}>
              <div>
                <dt>{t("catalog-remaining")}</dt>
                <dd>
                  <DetailCountdown endsAt={remaining()} />
                </dd>
              </div>
            </Show>
            <div>
              <dt>{t("catalog-status")}</dt>
              <dd>
                <span class={["public-auction-status", `public-auction-status--${statusClass()}`]}>
                  {status()}
                </span>
              </dd>
            </div>
          </dl>
          <Show when={ownProxyValue()}>{(proxy) => <DetailOwnProxy proxy={proxy()} />}</Show>
          <DetailStats lotId={row().lotId} />
          <Show when={!isPending()}>
            <DetailLatestBids
              auctionId={props.auctionId}
              lotId={row().lotId}
              returnFocus={focus() === "history"}
              onViewAll={() => {
                focus.set("history");
                historyOpen.set(true);
              }}
            />
          </Show>
        </div>
        <footer class="public-catalog-detail__foot">
          <Show when={bidReason() !== ""}>
            <p id={BID_REASON_ID} class="public-catalog-detail__reason" role="note">
              {bidReason()}
            </p>
          </Show>
          <div class="public-catalog-detail__foot-actions">
            <Switch>
              <Match when={commandBid()}>
                {(current) => (
                  <button
                    class="public-catalog-detail__bid"
                    type="button"
                    aria-haspopup="dialog"
                    disabled={current().disabled}
                    autofocus={focus() === "bid"}
                    aria-describedby={bidReason() !== "" ? BID_REASON_ID : undefined}
                    onClick={() => {
                      // 已打开的模态命令不接受另一个入口替换负载或目标；没有命令宿主时失败关闭。
                      if (commandDialog != null && commandDialog.peek() === null) {
                        focus.set("bid");
                        commandDialog.set(structuredClone(current().input));
                      }
                    }}
                  >
                    {entryLabel()}
                  </button>
                )}
              </Match>
              <Match when={bid().kind === "login"}>
                <Link class="public-catalog-detail__bid" href={loginHrefForCurrentLocation()}>
                  {t("catalog-login-to-bid")}
                </Link>
              </Match>
            </Switch>
            <Show when={props.input.watchable}>
              <LotWatchButton lotId={row().lotId} lotCode={row().code} iconOnly={false} />
            </Show>
            <button
              class="public-catalog-detail__price-history"
              type="button"
              aria-haspopup="dialog"
              autofocus={focus() === "price_history"}
              onClick={() => {
                focus.set("price_history");
                priceHistoryOpen.set(true);
              }}
            >
              {t("nav-price-history")}
            </button>
            <Show when={gameUrl() !== ""}>
              <a
                class="public-catalog-detail__game-link"
                href={gameUrl()}
                target="_blank"
                rel="noopener noreferrer"
              >
                {t("equipment-view-in-game")}
              </a>
            </Show>
          </div>
        </footer>
      </aside>
      <Show when={historyOpen()}>
        <DetailBidHistoryDialog
          auctionId={props.auctionId}
          lotId={row().lotId}
          lotCode={row().code}
          lotName={row().name}
          onClose={() => historyOpen.set(false)}
        />
      </Show>
      <Show when={priceHistoryOpen()}>
        <DetailPriceHistoryDialog
          auctionId={props.auctionId}
          lotCode={row().code}
          lotName={row().name}
          world={props.world}
          onClose={() => priceHistoryOpen.set(false)}
        />
      </Show>
    </>
  );
}

/** 装备属性。目录行不带属性，打开抽屉时按拍品读取一次（经公开读缓存）；
 * 读取中保留区块高度而不放“加载中”文字，失败时提供重试。 */
function DetailStats(props: { readonly lotId: string }): JSX.Element {
  const expanded = createCell(false);
  const lotId = createValueMemo(() => props.lotId);
  const stats = createTaskResource(
    () => lotId(),
    async (requestedLotId, context) => {
      const page = await fetchLotDetail(requestedLotId, context.signal);
      if (!context.alive()) {
        return err(LOT_DETAIL_FETCH_ERROR);
      }
      if (!page.ok) {
        return err(page.error);
      }
      const lot = page.value.lot;
      if (lot === null) {
        return ok([]);
      }
      return ok(statRows(lot.equipment.stats));
    },
  );
  const total = createValueMemo(() => {
    const snapshot = stats();
    return snapshot?.ok ? snapshot.value.length : 0;
  });
  const emptySuccess = createValueMemo(() => {
    const snapshot = stats();
    return snapshot?.ok === true && snapshot.value.length === 0;
  });
  const failed = createValueMemo(() => {
    const snapshot = stats();
    return snapshot !== undefined && !snapshot.ok;
  });
  const loaded = createValueMemo(() => {
    const snapshot = stats();
    return snapshot?.ok ? snapshot : null;
  });
  const visibleRows = createValueMemo(() => {
    const snapshot = stats();
    if (snapshot === undefined || !snapshot.ok) {
      return [];
    }
    return expanded() ? snapshot.value : snapshot.value.slice(0, COLLAPSED_STATS);
  });

  // 装备没有任何属性时整块不渲染，不留空标题。
  return (
    <Show when={!emptySuccess()}>
      <section class="public-catalog-detail__section">
        <div class="public-catalog-detail__section-head">
          <h3 class="public-catalog-detail__section-title">{t("equipment-stats")}</h3>
          <Show when={total() > COLLAPSED_STATS}>
            <button
              class="public-catalog-detail__more"
              type="button"
              aria-expanded={expanded() ? "true" : "false"}
              onClick={() => expanded.set(!expanded())}
            >
              {expanded()
                ? t("catalog-detail-show-less")
                : t("catalog-detail-show-all", { count: String(total()) })}
            </button>
          </Show>
        </div>
        <Switch>
          <Match when={stats() === undefined}>
            <div
              class="public-catalog-detail__stats public-catalog-detail__stats--pending"
              aria-busy="true"
            />
          </Match>
          <Match when={failed()}>
            <p class="public-catalog-detail__status" role="alert">
              <span>{t("data-error")}</span>
              <button
                class="public-catalog-detail__more"
                type="button"
                onClick={() => stats.restart()}
              >
                {t("data-retry")}
              </button>
            </p>
          </Match>
          <Match when={loaded() !== null}>
            <dl class="public-catalog-detail__stats">
              <For each={visibleRows()} keyed={(stat) => stat.name}>
                {(stat) => (
                  <div class="public-catalog-detail__stat">
                    <dt>{stat().name}</dt>
                    <dd>{stat().value}</dd>
                  </div>
                )}
              </For>
            </dl>
          </Match>
        </Switch>
      </section>
    </Show>
  );
}

/** 剩余时间只订阅在抽屉叶子上，抽屉事实行不随秒针重渲染。 */
function DetailCountdown(props: { readonly endsAt: string }): JSX.Element {
  const countdown = useCountdownDisplay(() => props.endsAt);
  return <span class={countdownClass(countdown())}>{countdown().label}</span>;
}

function DetailCloseIcon(): JSX.Element {
  return (
    <svg
      class="public-catalog-detail__close-icon"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="1.5"
      stroke-linecap="round"
      stroke-linejoin="round"
      aria-hidden="true"
    >
      <path d="M6 6l12 12M18 6 6 18" />
    </svg>
  );
}
