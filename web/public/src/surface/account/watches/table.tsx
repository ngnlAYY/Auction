//! 当前场次完整标记结果的本地分页和语义表格；不把目录总数冒充标记数。

import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { TableColumns } from "@auction/web-shared/ui/components/table-columns";
import { TruncatedText } from "@auction/web-shared/ui/components/truncated-text";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Show } from "solid-js";

import { worldLabel } from "../../board/world";
import { LotDetailLookupButton } from "../../catalog/detail/lookup";
import { auctionStatusLabel, lotStatusLabel } from "../../catalog/presentation";
import { PublicPager } from "../../pagination";
import type { WatchedAuction } from "./load";

const PAGE_SIZE = 10;

/** 按完整交集的真实总数分页，场次 keyed 宿主保证新场次从第一页开始。 */
export function WatchedTable(props: { readonly watched: WatchedAuction }): JSX.Element {
  const requestedPage = createCell(1);
  const total = () => props.watched.rows.length;
  const pages = () => Math.max(Math.trunc((total() + PAGE_SIZE - 1) / PAGE_SIZE), 1);
  const page = () => Math.min(Math.max(requestedPage(), 1), pages());
  const offset = () => (page() - 1) * PAGE_SIZE;
  const round = createValueMemo(() =>
    t("account-watches-round", {
      world: worldLabel(props.watched.auction.world),
      number: props.watched.auction.number,
    }),
  );
  const title = createValueMemo(() =>
    t("account-watches-heading", {
      round: round(),
      status: auctionStatusLabel(props.watched.auction.status),
    }),
  );
  const visibleRows = createValueMemo(() =>
    props.watched.rows.slice(offset(), offset() + PAGE_SIZE),
  );
  return (
    <>
      <h3>{title()}</h3>
      <Show
        when={total() > 0}
        fallback={<p class="public-account-empty">{t("account-watches-empty")}</p>}
      >
        <p class="public-account-hint">{t("account-watches-total", { count: total() })}</p>
        <div class="public-account-table-wrap">
          <table class="public-account-table" aria-label={title()}>
            <TableColumns layout="public-account-watches" widths={[13, 42, 22, 15, 8]} />
            <thead>
              <tr>
                <th scope="col">{t("account-lot")}</th>
                <th scope="col">{t("account-watches-equipment")}</th>
                <th scope="col">{t("account-watches-auction")}</th>
                <th scope="col">{t("account-status")}</th>
                <th scope="col" class="public-lot-view-cell">
                  {t("catalog-actions")}
                </th>
              </tr>
            </thead>
            <tbody>
              <For each={visibleRows()} keyed={(row) => row.lotId}>
                {(row) => (
                  <tr>
                    {/* 编号与名称只作标识；详情经操作栏的查看按钮在页内抽屉打开。 */}
                    <td class="public-account-code" data-label={t("account-lot")}>
                      {row().code}
                    </td>
                    <td data-label={t("account-watches-equipment")}>
                      <TruncatedText value={row().name} size="wide" portal={true} />
                    </td>
                    <td data-label={t("account-watches-auction")}>
                      <TruncatedText value={round()} size="compact" portal={true} />
                    </td>
                    <td data-label={t("account-status")}>
                      <TruncatedText
                        value={lotStatusLabel(row().status)}
                        size="compact"
                        portal={true}
                      />
                    </td>
                    <td class="public-lot-view-cell" data-label={t("catalog-actions")}>
                      <LotDetailLookupButton
                        auctionId={props.watched.auction.id}
                        lotId={row().lotId}
                        lotCode={row().code}
                        world={props.watched.auction.world}
                      />
                    </td>
                  </tr>
                )}
              </For>
            </tbody>
          </table>
        </div>
      </Show>
      {/* 始终把原页码交给纠偏宿主；空或单页只隐藏控件，不能留下会在总数恢复后跳回的旧页码。 */}
      <PublicPager
        label={title()}
        page={requestedPage()}
        total={total()}
        pageSize={PAGE_SIZE}
        hideWhenSingle={true}
        showSizePicker={false}
        numberedLayout
        maxOffset={null}
        jumpLabel={t("catalog-page-input")}
        onPageChange={(nextPage) => requestedPage.set(nextPage)}
        onPageClamp={(nextPage) => requestedPage.set(nextPage)}
        onPageSizeChange={() => {}}
      />
    </>
  );
}
