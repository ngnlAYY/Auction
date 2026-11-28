// 拍品详情抽屉的价格历史弹窗：按装备名列出拍品所在世界往期拍卖的成交价。
// 只读取既有的公开价格历史接口，不新增筛选语义。名称查询是“各词都包含”的匹配，
// 结果可能含名称更长的同类装备，这类行会标出实际装备名。

import type { PublicPriceHistoryListResponse } from "@auction/contracts/dto/public/catalog/history";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import { err, ok } from "@auction/web-shared/result";
import { Dialog } from "@auction/web-shared/ui/components/dialog";
import { TableColumns } from "@auction/web-shared/ui/components/table-columns";
import { Time } from "@auction/web-shared/ui/components/time";
import { formatCreditAmount } from "@auction/web-shared/ui/format";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { createEffect, For, Match, Show, Switch, useContext } from "solid-js";
import { priceHistoryQueryDefault } from "../../../api/model";
import { fetchHistoricalAuction } from "../../../api/queries/current";
import { fetchPriceHistory, PRICE_HISTORY_FETCH_ERROR } from "../../../api/queries/price-history";
import { PublicPager } from "../../pagination";
import { historyEquipmentNameQuery } from "../../reads/history/filters/validation";
import { SelectedWorldContext } from "../../shell";
import type { DetailWorld } from ".";

/** 弹窗每页条数；页码只存在于弹窗内，不写地址栏。 */
const PRICE_HISTORY_PAGE_SIZE = 10;

/** 同名装备的往期成交价弹窗。与完整竞价历史弹窗一样作为抽屉的兄弟节点渲染，关闭后焦点
 * 回到抽屉里打开它的按钮。成交记录不随实时事件变化，这里不订阅出价刷新。 */
export function DetailPriceHistoryDialog(props: {
  readonly auctionId: string;
  readonly lotCode: string;
  readonly lotName: string;
  readonly world: DetailWorld;
  readonly onClose: () => void;
}): JSX.Element {
  const selectedWorld = useContext(SelectedWorldContext);
  const page = createCell(1);
  const knownWorld = createValueMemo(() =>
    props.world.kind === "selected"
      ? selectedWorld()
      : props.world.kind === "known"
        ? props.world.world
        : null,
  );
  // 世界切换后旧页码可能超出新世界末页；重置到第一页，空页才不会失去返回入口。
  createEffect(knownWorld, () => page.set(1));
  const historyDependencies = createValueMemo(() => ({
    auctionId: props.auctionId,
    lotName: props.lotName,
    world: props.world,
    knownWorld: knownWorld(),
    requested: page(),
  }));
  const history = createTaskResource(historyDependencies, async (dependencies, context) => {
    // 名称得不到可用查询词时不发请求，按没有成交记录展示。
    const name = historyEquipmentNameQuery(dependencies.lotName);
    if (name === null) {
      return ok(null);
    }
    let world = dependencies.knownWorld;
    if (world === null) {
      // 只有拍卖 UUID：按拍卖读取其世界；读取失败按价格历史不可用处理，不猜测世界。
      const auction = await fetchHistoricalAuction(dependencies.auctionId, context.signal);
      if (!context.alive()) {
        return ok(null);
      }
      if (!auction.ok) {
        return err(PRICE_HISTORY_FETCH_ERROR);
      }
      world = auction.value.world;
    }
    const query = {
      ...priceHistoryQueryDefault(),
      world,
      name,
      page: dependencies.requested,
    };
    return fetchPriceHistory(query, PRICE_HISTORY_PAGE_SIZE, context.signal);
  });
  const snapshot = () => history();
  const loadedPage = () => {
    const result = snapshot();
    if (result === undefined || !result.ok || result.value === null) {
      return null;
    }
    return result.value;
  };
  const nonEmptyPage = () => {
    const current = loadedPage();
    return current !== null && current.rows.length > 0 ? current : null;
  };
  const empty = () => {
    const result = snapshot();
    return result?.ok === true && (result.value === null || result.value.rows.length === 0);
  };
  const failed = () => {
    const result = snapshot();
    return result !== undefined && !result.ok;
  };

  return (
    <Dialog
      title={t("nav-price-history")}
      closeLabel={t("catalog-close")}
      onClose={() => props.onClose()}
    >
      <div class="public-bid-dialog__lot public-catalog-history__lot">
        <div class="public-bid-dialog__lot-name">
          <span class="public-bid-dialog__lot-code">{props.lotCode}</span>
          <span class="public-bid-dialog__lot-title">{props.lotName}</span>
        </div>
      </div>
      <div class="public-catalog-history">
        <p class="public-catalog-history__note">{t("catalog-price-history-note")}</p>
        {/* 翻页期间保留上一页的表格，新页到达后整体替换；只有首次读取时正文为空。 */}
        <Switch>
          <Match when={snapshot() === undefined}>{null}</Match>
          <Match when={failed()}>
            <p class="public-catalog-detail__status" role="alert">
              <span>{t("data-error")}</span>
              <button
                class="public-catalog-detail__more"
                type="button"
                onClick={() => history.restart()}
              >
                {t("data-retry")}
              </button>
            </p>
          </Match>
          <Match when={nonEmptyPage()}>
            {(current) => (
              <>
                <PriceHistoryRows page={current()} lotName={props.lotName} />
                <PublicPager
                  page={current().page ?? 1}
                  pageSize={PRICE_HISTORY_PAGE_SIZE}
                  total={current().total}
                  hasMore={current().has_more ?? false}
                  hideWhenSingle={true}
                  showSizePicker={false}
                  label={t("nav-price-history")}
                  onPageChange={(next: number) => page.set(next)}
                  onPageClamp={(next: number) => page.set(next)}
                  onPageSizeChange={() => {}}
                />
              </>
            )}
          </Match>
          <Match when={empty()}>
            <p class="public-catalog-detail__empty">{t("catalog-price-history-empty")}</p>
          </Match>
        </Switch>
      </div>
    </Dialog>
  );
}

/** 成交价表格。名称与当前拍品不同（忽略 ASCII 大小写）的行在期数下方标出实际装备名。 */
function PriceHistoryRows(props: {
  readonly page: PublicPriceHistoryListResponse;
  readonly lotName: string;
}): JSX.Element {
  const currentName = () => rustTrim(props.lotName);
  return (
    <div class="public-auction-lot__table">
      <table aria-label={t("nav-price-history")}>
        <TableColumns layout="public-lot-price-history" widths={[38, 14, 20, 28]} />
        <thead>
          <tr>
            <th>{t("history-round")}</th>
            <th>{t("history-level")}</th>
            <th class="public-auction-lot__number">{t("history-price")}</th>
            <th>{t("history-closed")}</th>
          </tr>
        </thead>
        <tbody>
          <For each={props.page.rows} keyed={(row) => row.lot_id}>
            {(row) => (
              <tr>
                <td data-label={t("history-round")}>
                  <span>{`#${row().auction_sequence}`}</span>
                  <Show when={!eqIgnoreAsciiCase(rustTrim(row().equipment_name), currentName())}>
                    <span class="public-catalog-history__variant">{row().equipment_name}</span>
                  </Show>
                </td>
                <td data-label={t("history-level")}>
                  {row().equipment_level == null ? t("value-empty") : String(row().equipment_level)}
                </td>
                <td
                  class="public-auction-lot__amount public-auction-lot__number"
                  data-label={t("history-price")}
                >
                  {formatCreditAmount(row().price)}
                </td>
                <td data-label={t("history-closed")}>
                  <Time value={row().closed_at} />
                </td>
              </tr>
            )}
          </For>
        </tbody>
      </table>
    </div>
  );
}

/** Rust `str::trim` 的 Unicode 空白集合；JS `trim()` 的集合不同，名称比较必须按源语义。 */
function isRustWhitespace(code: number): boolean {
  return (
    code === 0x09 ||
    code === 0x0a ||
    code === 0x0b ||
    code === 0x0c ||
    code === 0x0d ||
    code === 0x20 ||
    code === 0x85 ||
    code === 0xa0 ||
    code === 0x1680 ||
    (code >= 0x2000 && code <= 0x200a) ||
    code === 0x2028 ||
    code === 0x2029 ||
    code === 0x202f ||
    code === 0x205f ||
    code === 0x3000
  );
}

function rustTrim(value: string): string {
  let start = 0;
  let end = value.length;
  while (start < end && isRustWhitespace(value.charCodeAt(start))) {
    start += 1;
  }
  while (end > start && isRustWhitespace(value.charCodeAt(end - 1))) {
    end -= 1;
  }
  return value.slice(start, end);
}

/** Rust `str::eq_ignore_ascii_case`：只折叠 A-Z，不做 Unicode 大小写转换。 */
function eqIgnoreAsciiCase(left: string, right: string): boolean {
  if (left.length !== right.length) {
    return false;
  }
  for (let index = 0; index < left.length; index += 1) {
    const leftCode = left.charCodeAt(index);
    const rightCode = right.charCodeAt(index);
    if (leftCode === rightCode) {
      continue;
    }
    const leftLower = leftCode >= 0x41 && leftCode <= 0x5a ? leftCode + 0x20 : leftCode;
    const rightLower = rightCode >= 0x41 && rightCode <= 0x5a ? rightCode + 0x20 : rightCode;
    if (leftLower !== rightLower) {
      return false;
    }
  }
  return true;
}
