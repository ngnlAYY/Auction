// Public 补偿池余额、批次表格和展开行。

import { createValueMemo } from "@auction/web-shared/reactive/memo";
import type { Result } from "@auction/web-shared/result";
import { TableColumns } from "@auction/web-shared/ui/components/table-columns";
import { Time } from "@auction/web-shared/ui/components/time";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Show } from "solid-js";

import type {
  CompensationLotPage,
  CompensationPoolPage,
  CompensationRound,
} from "../../api/compensation";
import { worldLabel } from "../board/world";
import { formatAuctionTime } from "../catalog/presentation";
import { PublicPager } from "../pagination";
import { coverageLabel, credits, signedAmount } from ".";
import { CompensationLotDetails } from "./lots";

interface CompensationReversal {
  readonly text: string;
  readonly title: string | null;
}

/** 补偿池批次表；批次分页与展开明细的分页状态彼此独立。 */
export function CompensationPoolTable(props: {
  readonly page: CompensationPoolPage;
  readonly selectedAuctionId: string;
  readonly lots: Result<CompensationLotPage | null, string> | null;
  readonly onPageChange: (page: number) => void;
  readonly onPageClamp: (page: number) => void;
  readonly onPageSizeChange: (size: number) => void;
  readonly onSelectRound: (auctionId: string) => void;
  readonly onLotPageChange: (page: number) => void;
  readonly onLotPageClamp: (page: number) => void;
  readonly onLotPageSizeChange: (size: number) => void;
  readonly onRetryLots: () => void;
}): JSX.Element {
  const pageSize = createValueMemo(() => Math.max(props.page.page_size, 1));
  // 冲正列默认隐藏，只有本页存在冲正时才显示；否则整列都是“—”。
  const showReversal = createValueMemo(() =>
    props.page.rounds.some((round) => round.reversal_count > 0),
  );
  return (
    <section class="public-compensation-page" aria-label={t("page-compensation-pool-title")}>
      {/* 余额在前、计算方式在后：手机上先看到结果，桌面并排展示。 */}
      <div class="public-compensation-overview">
        {/* biome-ignore lint/a11y/useAriaPropsSupportedByRole: 源码在无 role 的普通容器上保留 aria-label，不能改 DOM 结构。 */}
        <div class="public-compensation-balance" aria-label={t("compensation-balance")}>
          <span class="public-compensation-balance__label">{t("compensation-balance")}</span>
          <strong class="public-compensation-balance__amount">{credits(props.page.balance)}</strong>
          <small>
            <time datetime={props.page.updated_at}>
              {t("compensation-updated", { date: formatAuctionTime(props.page.updated_at) })}
            </time>
          </small>
        </div>
        <section class="public-compensation-rules" aria-labelledby="compensation-rules-title">
          <h2 id="compensation-rules-title">{t("compensation-rules-title")}</h2>
          {/* 补偿池的一句话说明是公式之外唯一的白话解释，随计算方式卡片展示。 */}
          <p class="public-compensation-intro">{t("page-compensation-pool-note")}</p>
          <ol>
            <li>{t("compensation-rule-available")}</li>
            <li>{t("compensation-rule-share")}</li>
            <li>{t("compensation-rule-reversal")}</li>
          </ol>
        </section>
      </div>
      <Show
        when={props.page.rounds.length > 0}
        fallback={<p class="public-compensation-empty">{t("compensation-empty")}</p>}
      >
        <div class="public-compensation-table-wrap ui-table--fit">
          <table class="public-compensation-table">
            <TableColumns layout="public-compensation" widths={roundColumnWidths(showReversal())} />
            <thead>
              <tr>
                <th
                  scope="col"
                  class="public-compensation-toggle-cell"
                  aria-label={t("compensation-details")}
                />
                <th scope="col">{t("compensation-round-label")}</th>
                <th scope="col">{t("compensation-world")}</th>
                <th scope="col" class="public-compensation-number">
                  {t("compensation-opening")}
                </th>
                <th scope="col" class="public-compensation-number">
                  {t("compensation-tax")}
                </th>
                <th scope="col" class="public-compensation-number">
                  {t("compensation-discount")}
                </th>
                <th scope="col" class="public-compensation-number">
                  {t("compensation-out")}
                </th>
                <th scope="col" class="public-compensation-number">
                  {t("compensation-coverage")}
                </th>
                <th scope="col" class="public-compensation-number">
                  {t("compensation-closing")}
                </th>
                <Show when={showReversal()}>
                  <th scope="col" class="public-compensation-number">
                    {t("compensation-reversal")}
                  </th>
                </Show>
                <th scope="col">{t("compensation-settled")}</th>
              </tr>
            </thead>
            <tbody>
              <For each={props.page.rounds} keyed={false}>
                {(round) => {
                  const open = createValueMemo(
                    () => props.selectedAuctionId === round().auction_id,
                  );
                  // 只有展开行消费明细；不能给每个折叠批次复制完整明细页。
                  const visibleLots = createValueMemo(() => (open() ? props.lots : null));
                  return (
                    <CompensationRoundRow
                      round={round()}
                      open={open()}
                      showReversal={showReversal()}
                      lots={visibleLots()}
                      onSelectRound={props.onSelectRound}
                      onLotPageChange={props.onLotPageChange}
                      onLotPageClamp={props.onLotPageClamp}
                      onLotPageSizeChange={props.onLotPageSizeChange}
                      onRetryLots={props.onRetryLots}
                    />
                  );
                }}
              </For>
            </tbody>
          </table>
        </div>
      </Show>
      <PublicPager
        page={props.page.page ?? 1}
        pageSize={pageSize()}
        total={props.page.total ?? null}
        hasMore={props.page.has_more ?? false}
        label={t("page-compensation-pool-title")}
        onPageChange={props.onPageChange}
        onPageClamp={props.onPageClamp}
        onPageSizeChange={props.onPageSizeChange}
      />
    </section>
  );
}

function CompensationRoundRow(props: {
  readonly round: CompensationRound;
  readonly open: boolean;
  readonly showReversal: boolean;
  readonly lots: Result<CompensationLotPage | null, string> | null;
  readonly onSelectRound: (auctionId: string) => void;
  readonly onLotPageChange: (page: number) => void;
  readonly onLotPageClamp: (page: number) => void;
  readonly onLotPageSizeChange: (size: number) => void;
  readonly onRetryLots: () => void;
}): JSX.Element {
  const detailsId = createValueMemo(() => `compensation-details-${props.round.auction_id}`);
  // 单元格只放净额，笔数放进提示，避免窄列截断金额。
  const reversal = createValueMemo((): CompensationReversal => {
    if (props.round.reversal_count > 0) {
      const amount = signedAmount(props.round.reversal_amount);
      return {
        text: amount,
        title: t("compensation-reversal-summary", {
          amount,
          count: props.round.reversal_count,
        }),
      };
    }
    return { text: "—", title: null };
  });
  return (
    <>
      <tr class={["public-compensation-round", { "public-compensation-round--open": props.open }]}>
        <td class="public-compensation-toggle-cell" data-label={t("compensation-details")}>
          <button
            class="public-history-group-toggle public-compensation-expand"
            type="button"
            aria-controls={detailsId()}
            aria-expanded={props.open ? "true" : "false"}
            aria-label={
              props.open
                ? t("compensation-collapse", { sequence: props.round.auction_sequence })
                : t("compensation-expand", { sequence: props.round.auction_sequence })
            }
            onClick={() => props.onSelectRound(props.round.auction_id)}
          >
            <span class="public-chevron" aria-hidden="true" />
          </button>
        </td>
        <td data-label={t("compensation-round-label")}>#{props.round.auction_sequence}</td>
        <td data-label={t("compensation-world")}>{worldLabel(props.round.world_id)}</td>
        <td class="public-compensation-number" data-label={t("compensation-opening")}>
          {credits(props.round.opening_balance)}
        </td>
        <td class="public-compensation-number" data-label={t("compensation-tax")}>
          <span class="public-compensation-income">{signedAmount(props.round.tax_income)}</span>
        </td>
        <td class="public-compensation-number" data-label={t("compensation-discount")}>
          {credits(props.round.discount_demand)}
        </td>
        <td class="public-compensation-number" data-label={t("compensation-out")}>
          <span class="public-compensation-outflow">
            {signedAmount(-props.round.compensation_out)}
          </span>
        </td>
        <td class="public-compensation-number" data-label={t("compensation-coverage")}>
          {coverageLabel(props.round.compensation_out, props.round.discount_demand)}
        </td>
        <td
          class="public-compensation-number public-compensation-closing"
          data-label={t("compensation-closing")}
        >
          <strong>{credits(props.round.closing_balance)}</strong>
        </td>
        <Show when={props.showReversal}>
          <td class="public-compensation-number" data-label={t("compensation-reversal")}>
            <span title={reversal().title ?? undefined}>{reversal().text}</span>
          </td>
        </Show>
        <td data-label={t("compensation-settled")}>
          <Time value={props.round.settled_at} />
        </td>
      </tr>
      <Show when={props.open}>
        <tr id={detailsId()} class="public-compensation-details-row">
          <td colspan={props.showReversal ? 11 : 10}>
            <CompensationLotDetails
              round={props.round}
              lots={props.lots}
              onLotPageChange={props.onLotPageChange}
              onLotPageClamp={props.onLotPageClamp}
              onLotPageSizeChange={props.onLotPageSizeChange}
              onRetryLots={props.onRetryLots}
            />
          </td>
        </tr>
      </Show>
    </>
  );
}

/** 批次表列宽；隐藏冲正列时其余列按原权重归一化。 */
function roundColumnWidths(showReversal: boolean): readonly number[] {
  if (showReversal) {
    return [3, 6, 7, 10, 10, 9, 10, 7, 10, 10, 18];
  }
  return [3, 6, 8, 10, 10, 9, 10, 7, 10, 19];
}
