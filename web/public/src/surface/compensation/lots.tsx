// Public 单场逐件结算明细：每件的计算依据、金额与成交撤销冲正。

import { createValueMemo } from "@auction/web-shared/reactive/memo";
import type { Result } from "@auction/web-shared/result";
import { TableColumns } from "@auction/web-shared/ui/components/table-columns";
import { TruncatedText } from "@auction/web-shared/ui/components/truncated-text";
import { formatHeldDuration, formatPpm } from "@auction/web-shared/ui/format";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Match, Show, Switch } from "solid-js";

import type {
  CompensationLot,
  CompensationLotPage,
  CompensationRound,
} from "../../api/compensation";
import { lotCodeFor } from "../../api/model";
import { worldLabel } from "../board/world";
import { LotDetailLookupButton } from "../catalog/detail/lookup";
import { formatAuctionTime } from "../catalog/presentation";
import { useContentLoading } from "../loading";
import { PublicPager } from "../pagination";
import { credits, signedAmount } from ".";

interface CompensationLotKind {
  readonly kind: string | null;
  readonly kindClass: string;
  readonly amount: string;
  readonly amountClass: string;
  readonly amountTitle: string | null;
}

interface CompensationLotReversal {
  readonly text: string;
  readonly title: string | null;
}

/** 单场逐件结算明细；加载状态沿用页面共享的内容加载指示。 */
export function CompensationLotDetails(props: {
  readonly round: CompensationRound;
  readonly lots: Result<CompensationLotPage | null, string> | null;
  readonly onLotPageChange: (page: number) => boolean;
  readonly onLotPageClamp: (page: number) => boolean;
  readonly onLotPageSizeChange: (size: number) => boolean;
  readonly onRetryLots: () => void;
}): JSX.Element {
  useContentLoading(() => props.lots === null);
  const failed = () => props.lots !== null && !props.lots.ok;
  const empty = () => props.lots?.ok === true && props.lots.value === null;
  const currentPage = createValueMemo((): CompensationLotPage | null => {
    const current = props.lots;
    return current?.ok === true && current.value !== null ? current.value : null;
  });
  return (
    <section class="public-compensation-movements" aria-label={t("compensation-lots")}>
      <Switch fallback={null}>
        <Match when={props.lots === null}>{null}</Match>
        <Match when={failed()}>
          <div role="alert">
            <p class="public-auction-page__status">{t("data-error")}</p>
            <button type="button" onClick={props.onRetryLots}>
              {t("data-retry")}
            </button>
          </div>
        </Match>
        <Match when={empty()}>
          <p class="public-auction-page__status">{t("data-empty")}</p>
        </Match>
        <Match when={currentPage()}>
          {(page) => {
            // 冲正列默认隐藏，只有本页存在已撤销成交（含未冲正）时才显示。
            const showReversal = createValueMemo(() =>
              page().lots.some((lot) => lot.voided || lot.reversal != null),
            );
            return (
              <>
                <Show
                  when={page().lots.length > 0}
                  fallback={<p class="public-auction-page__status">{t("data-empty")}</p>}
                >
                  <div class="public-compensation-movement-wrap ui-table--fit">
                    <table class="public-compensation-movement-table">
                      <TableColumns
                        layout="public-compensation-lots"
                        widths={lotColumnWidths(showReversal())}
                      />
                      <thead>
                        <tr>
                          <th>{t("compensation-lot")}</th>
                          <th>{t("compensation-equipment")}</th>
                          <th>{t("compensation-world")}</th>
                          <th class="public-compensation-number">{t("compensation-hold")}</th>
                          <th class="public-compensation-number">{t("compensation-adjustment")}</th>
                          <th class="public-compensation-number">{t("compensation-held-time")}</th>
                          <th class="public-compensation-kind-cell">{t("compensation-kind")}</th>
                          <th class="public-compensation-number">{t("compensation-amount")}</th>
                          <th class="public-compensation-number">{t("compensation-end-price")}</th>
                          <th class="public-compensation-number">
                            {t("compensation-buyer-amount")}
                          </th>
                          <th class="public-compensation-number">
                            {t("compensation-seller-amount")}
                          </th>
                          <Show when={showReversal()}>
                            <th class="public-compensation-number">{t("compensation-reversal")}</th>
                          </Show>
                          <th class="public-lot-view-cell">{t("catalog-actions")}</th>
                        </tr>
                      </thead>
                      <tbody>
                        <For each={page().lots} keyed={false}>
                          {(lot) => (
                            <CompensationLotRow
                              lot={lot()}
                              auctionId={props.round.auction_id}
                              worldId={props.round.world_id}
                              showReversal={showReversal()}
                            />
                          )}
                        </For>
                      </tbody>
                    </table>
                  </div>
                </Show>
                <PublicPager
                  page={page().page}
                  pageSize={page().page_size}
                  total={page().total}
                  hasMore={false}
                  label={t("compensation-lots")}
                  movement={true}
                  onPageChange={props.onLotPageChange}
                  onPageClamp={props.onLotPageClamp}
                  onPageSizeChange={props.onLotPageSizeChange}
                />
              </>
            );
          }}
        </Match>
      </Switch>
    </section>
  );
}

function CompensationLotRow(props: {
  readonly lot: CompensationLot;
  readonly auctionId: string;
  readonly worldId: string;
  readonly showReversal: boolean;
}): JSX.Element {
  const lotCode = createValueMemo(() =>
    lotCodeFor(props.lot.equipment_category, props.lot.lot_number),
  );
  const equipment = createValueMemo(
    () => `${props.lot.equipment_name}(${props.lot.equipment_category})`,
  );
  // 一件成交要么缴纳税收入池，要么从池中获得补偿，用“类型”一列区分；金额按池的视角带符号，
  // 税收流入为 `+`、补偿流出为 `−`。有折扣需求时即使补偿为零也显示，让池余额不足可见，
  // 名义折扣需求放进提示供复算。
  const presentation = createValueMemo((): CompensationLotKind => {
    if (props.lot.tax_amount > 0) {
      return {
        kind: t("compensation-kind-tax"),
        kindClass: "public-compensation-kind public-compensation-kind--tax",
        amount: signedAmount(props.lot.tax_amount),
        amountClass: "public-compensation-income",
        amountTitle: null,
      };
    }
    if (props.lot.compensation_amount > 0 || props.lot.discount_amount > 0) {
      return {
        kind: t("compensation-kind-compensation"),
        kindClass: "public-compensation-kind public-compensation-kind--compensation",
        amount: signedAmount(-props.lot.compensation_amount),
        amountClass: "public-compensation-outflow",
        amountTitle: t("compensation-discount-title", {
          amount: credits(props.lot.discount_amount),
        }),
      };
    }
    return {
      kind: null,
      kindClass: "",
      amount: "—",
      amountClass: "",
      amountTitle: null,
    };
  });
  const kind = createValueMemo(() => {
    const current = presentation();
    return current.kind === null ? null : { text: current.kind, className: current.kindClass };
  });
  const reversal = createValueMemo((): CompensationLotReversal => {
    const current = props.lot.reversal;
    let text: string;
    if (current != null) {
      text =
        current.tax_shortfall > 0
          ? t("compensation-reversal-shortfall", {
              amount: signedAmount(current.amount),
              shortfall: credits(current.tax_shortfall),
            })
          : signedAmount(current.amount);
    } else if (props.lot.voided) {
      text = t("compensation-voided-unreversed");
    } else {
      text = "—";
    }
    return {
      text,
      title:
        current == null
          ? null
          : t("compensation-reversal-detail", {
              date: formatAuctionTime(current.reversed_at),
              balance: credits(current.balance_after),
            }),
    };
  });
  return (
    <tr class={{ "public-compensation-voided": props.lot.voided }}>
      {/* 编号只作标识；详情经操作栏的查看按钮在页内抽屉打开。 */}
      <td data-label={t("compensation-lot")}>{lotCode()}</td>
      <td data-label={t("compensation-equipment")}>
        <a
          class="public-compensation-equipment-link"
          href={props.lot.game_equipment_url}
          target="_blank"
          rel="noopener noreferrer"
          aria-label={equipment()}
          title={equipment()}
        >
          <TruncatedText value={equipment()} size="wide" focusable={false} portal={true} />
        </a>
      </td>
      <td data-label={t("compensation-world")}>{worldLabel(props.worldId)}</td>
      <td class="public-compensation-number" data-label={t("compensation-hold")}>
        {formatPpm(props.lot.hold_ppm, false)}
      </td>
      <td class="public-compensation-number" data-label={t("compensation-adjustment")}>
        {formatPpm(props.lot.adjustment_ppm, true)}
      </td>
      <td class="public-compensation-number" data-label={t("compensation-held-time")}>
        {formatHeldDuration(props.lot.leader_held_nanos)}
      </td>
      <td class="public-compensation-kind-cell" data-label={t("compensation-kind")}>
        <Show when={kind()} fallback="—">
          {(current) => (
            <span class={current().className} title={current().text}>
              {current().text}
            </span>
          )}
        </Show>
      </td>
      <td class="public-compensation-number" data-label={t("compensation-amount")}>
        <span class={presentation().amountClass} title={presentation().amountTitle ?? undefined}>
          {presentation().amount}
        </span>
      </td>
      <td class="public-compensation-number" data-label={t("compensation-end-price")}>
        {credits(props.lot.end_price)}
      </td>
      <td class="public-compensation-number" data-label={t("compensation-buyer-amount")}>
        <CompensationParty
          amount={props.lot.buyer_amount}
          name={props.lot.buyer_name}
          strong={true}
        />
      </td>
      <td class="public-compensation-number" data-label={t("compensation-seller-amount")}>
        <CompensationParty
          amount={props.lot.seller_amount}
          name={props.lot.seller_name}
          strong={false}
        />
      </td>
      <Show when={props.showReversal}>
        <td class="public-compensation-number" data-label={t("compensation-reversal")}>
          <span title={reversal().title ?? undefined}>{reversal().text}</span>
        </td>
      </Show>
      <td class="public-lot-view-cell" data-label={t("catalog-actions")}>
        <LotDetailLookupButton
          auctionId={props.auctionId}
          lotId={props.lot.lot_id}
          lotCode={lotCode()}
          world={props.worldId}
        />
      </td>
    </tr>
  );
}

/** 买家实付/卖家所得格：金额始终完整显示，后接括号内的玩家名；名字过长时截断并提供完整值提示。
 * 没有投稿记录的历史 lot 卖家为 `—`，此时不显示括号。 */
function CompensationParty(props: {
  readonly amount: number;
  readonly name: string;
  readonly strong: boolean;
}): JSX.Element {
  return (
    <span class="public-compensation-party">
      <Show when={props.strong} fallback={<span>{credits(props.amount)}</span>}>
        <strong>{credits(props.amount)}</strong>
      </Show>
      <Show when={props.name !== "—"}>
        <span class="public-compensation-party__name">
          <TruncatedText value={props.name} size="compact" focusable={false} portal={true} />
        </span>
      </Show>
    </span>
  );
}

/** 逐件明细列宽（千分比）。数字列按窄桌面（表宽约 1000px、小号字）下最长完整值实测预留，
 * 类型列容纳英文 “Compensation” 标签；买家实付/卖家所得在金额之外留出玩家名空间。
 * 装备名与玩家名取剩余宽度，是仅有的允许截断的内容。 */
function lotColumnWidths(showReversal: boolean): readonly number[] {
  // 末列是查看按钮；权重按总和归一化，其余列等比例让出宽度。
  if (showReversal) {
    return [51, 89, 50, 75, 77, 87, 109, 80, 90, 118, 110, 64, 40];
  }
  return [51, 113, 50, 75, 77, 87, 109, 80, 90, 138, 130, 40];
}
