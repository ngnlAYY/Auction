// Public 拍卖的预览与历史视图切换。

import type {
  AuctionSummaryResponse,
  CurrentAuctionItemResponse,
} from "@auction/contracts/dto/public/catalog/auction";
import { Link } from "@auction/web-shared/router";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { Match, Switch } from "solid-js";
import type { PreviewHistoryLoad, PublicBoardQuery } from "../query";
import { normalizeWorldQuery, publicHref } from "../query";

interface ViewNavigation {
  readonly currentHref: string;
  readonly previousHref: string | null;
}

/** 预览与历史场次的视图切换；读取期间不占位，加载提示由调用方登记到顶栏。 */
export function previewHistorySwitch(
  state: PreviewHistoryLoad | null | undefined,
  auction: CurrentAuctionItemResponse,
  historical: boolean,
  query: PublicBoardQuery,
): JSX.Element {
  // 当前竞价场次不展示切换；预览与往期场次才等待辅助读取。
  if (!historical && auction.presentation_mode === "live") {
    return null;
  }
  if (state === null || state === undefined) {
    return null;
  }
  const failed = !state.ok || state.value === null;
  const current = state.ok && state.value !== null ? state.value[0] : null;
  const rounds = state.ok && state.value !== null ? state.value[1] : [];
  const latestSettled =
    current === null ? null : maxRound(rounds, (round) => round.world_id === current.world);
  const hiddenHistorical =
    historical && (latestSettled === null || latestSettled.id !== auction.id);
  const navigation = viewNavigation(state, current, rounds, failed, hiddenHistorical, query);

  return (
    <Switch fallback={null}>
      <Match when={failed}>
        <nav class="public-auction-view-switch" aria-label={t("auction-history-view-label")}>
          <span class="public-auction-view-switch__options">
            <span class="public-auction-view-switch__status">{t("data-error")}</span>
          </span>
        </nav>
      </Match>
      <Match when={navigation}>
        {(loaded) => (
          <nav class="public-auction-view-switch" aria-label={t("auction-history-view-label")}>
            <span class="public-auction-view-switch__options">
              <Switch>
                <Match when={historical}>
                  <Link href={loaded().currentHref}>{t("auction-history-current")}</Link>
                </Match>
                <Match when={loaded().previousHref !== null}>
                  <Link href={loaded().previousHref ?? ""}>
                    {t("auction-history-previous-view")}
                  </Link>
                </Match>
                <Match when={true}>
                  <span>{t("auction-history-no-previous")}</span>
                </Match>
              </Switch>
            </span>
          </nav>
        )}
      </Match>
    </Switch>
  );
}

function viewNavigation(
  state: PreviewHistoryLoad,
  current: CurrentAuctionItemResponse | null,
  rounds: readonly AuctionSummaryResponse[],
  failed: boolean,
  hiddenHistorical: boolean,
  query: PublicBoardQuery,
): ViewNavigation | null {
  if (!state.ok || failed || hiddenHistorical || current === null) {
    return null;
  }
  if (current.presentation_mode === "live") {
    return null;
  }
  // 链接随筛选与分页的查询状态重算；两份链接各自复制，避免改一处时污染另一份。
  const currentQuery = { ...query };
  currentQuery.auctionId = null;
  currentQuery.world = normalizeWorldQuery(current.world);
  const currentHref = publicHref("/auction", currentQuery);
  const previous = maxRound(
    rounds,
    (round) => round.world_id === current.world && round.sequence < current.number,
  );
  let previousHref: string | null = null;
  if (previous !== null) {
    const previousQuery = { ...query };
    previousQuery.auctionId = previous.id;
    previousQuery.world = normalizeWorldQuery(previous.world_id);
    previousHref = publicHref("/auction", previousQuery);
  }
  return { currentHref, previousHref };
}

/** Rust `max_by_key` 在同值最大值时保留后出现的元素；顺序不能换成先到者。 */
function maxRound(
  rounds: readonly AuctionSummaryResponse[],
  predicate: (round: AuctionSummaryResponse) => boolean,
): AuctionSummaryResponse | null {
  let selected: AuctionSummaryResponse | null = null;
  for (const round of rounds) {
    if (!predicate(round)) {
      continue;
    }
    if (selected === null || round.sequence >= selected.sequence) {
      selected = round;
    }
  }
  return selected;
}
