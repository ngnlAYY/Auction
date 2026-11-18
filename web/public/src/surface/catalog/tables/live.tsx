// Public 拍卖目录的实时表格与实时行。

import { MAX_PUBLIC_BID_AMOUNT } from "@auction/contracts/dto/public/commands";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { TableColumns } from "@auction/web-shared/ui/components/table-columns";
import { formatCreditAmount, formatCreditAmountFull } from "@auction/web-shared/ui/format";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Show } from "solid-js";

import type { CatalogRow } from "../../../api/model";
import { LotWatchButton, useLotWatched } from "../../watches/control";
import { CatalogRecoveryEntry, LiveCatalogActions } from "../actions";
import type { CatalogLayout, CatalogVisibility } from "../columns/state";
import { catalogVisibilityContains, catalogVisibilityWidths } from "../columns/state";
import { recoveryDialogInput } from "../command-scope";
import { CatalogViewButton, useCatalogDetailEntry } from "../detail/entry";
import {
  type CountdownDisplay,
  countdownClass,
  lotStatusClass,
  lotStatusLabel,
  useCountdownDisplay,
} from "../presentation";
import { CatalogEquipmentCells, CatalogEquipmentName, CatalogParticipant } from "../rows";

/** 竞价期目录表格；列与 colgroup 宽度随可见列归一化，行以 lot_id 为稳定 key。 */
export function LiveCatalogTable(props: {
  readonly rows: readonly CatalogRow[];
  readonly indices: readonly number[];
  readonly visibility: CatalogVisibility;
  readonly showCommands: boolean;
  readonly canBid: boolean;
  readonly allowSlow: boolean;
  /** 该场冻结策略的代理默认模式；往期目录拼装的已结算场次为 null。 */
  readonly proxyDefaultMode: string | null;
  readonly sessionReady: boolean;
  readonly authenticated: boolean;
  readonly viewerPlayerId: string | null;
}): JSX.Element {
  // colgroup 分配完整百分比宽度；窄屏由 CSS 将同一行列 DOM 展示为字段卡片。
  const layout = createValueMemo(() =>
    props.showCommands ? "public-catalog-live" : "public-catalog-settled",
  );
  const columnLayout = createValueMemo(
    (): CatalogLayout => (props.showCommands ? "live" : "settled"),
  );
  const widths = createValueMemo(() => catalogVisibilityWidths(props.visibility, columnLayout()));
  const entries = createValueMemo(() =>
    props.indices.map((index) => {
      const row = props.rows[index];
      if (row === undefined) {
        throw new Error("目录行索引越界");
      }
      return { key: row.lotId, row };
    }),
  );
  return (
    <div class="public-auction-catalog">
      <table
        class="public-auction-catalog__table public-auction-catalog__table--live"
        aria-label={t("auction-lots")}
      >
        <TableColumns layout={layout()} widths={widths()} />
        <thead>
          <tr>
            <th hidden={!catalogVisibilityContains(props.visibility, "code")} scope="col">
              {t("catalog-code")}
            </th>
            <th hidden={!catalogVisibilityContains(props.visibility, "item")} scope="col">
              {t("catalog-item")}
            </th>
            <th hidden={!catalogVisibilityContains(props.visibility, "group")} scope="col">
              {t("catalog-group")}
            </th>
            <th
              hidden={!catalogVisibilityContains(props.visibility, "average_roll")}
              scope="col"
              class="public-auction-catalog__number"
            >
              {t("equipment-avg-roll")}
            </th>
            <th hidden={!catalogVisibilityContains(props.visibility, "level")} scope="col">
              {t("catalog-level")}
            </th>
            <th hidden={!catalogVisibilityContains(props.visibility, "attributes")} scope="col">
              {t("catalog-primary-attributes")}
            </th>
            <th hidden={!catalogVisibilityContains(props.visibility, "seller")} scope="col">
              {t("catalog-seller")}
            </th>
            <th
              hidden={!catalogVisibilityContains(props.visibility, "price")}
              scope="col"
              class="public-auction-catalog__number"
            >
              {t("catalog-current")}
            </th>
            <th
              hidden={!catalogVisibilityContains(props.visibility, "next_bid")}
              scope="col"
              class="public-auction-catalog__number"
            >
              {t("catalog-next-bid")}
            </th>
            <th hidden={!catalogVisibilityContains(props.visibility, "leader")} scope="col">
              {t("catalog-leader")}
            </th>
            <th hidden={!catalogVisibilityContains(props.visibility, "remaining")} scope="col">
              {t("catalog-remaining")}
            </th>
            <th hidden={!catalogVisibilityContains(props.visibility, "status")} scope="col">
              {t("catalog-status")}
            </th>
            <th hidden={!catalogVisibilityContains(props.visibility, "actions")} scope="col">
              {t("catalog-actions")}
            </th>
          </tr>
        </thead>
        <tbody>
          {/* 以 lot_id 作为稳定 key，避免刷新后行状态错位绑定到别的 lot。 */}
          <For each={entries()} keyed={(entry) => entry.key}>
            {(entry) => (
              <LiveCatalogRow
                row={entry().row}
                visibility={props.visibility}
                showCommands={props.showCommands}
                canBid={props.canBid}
                allowSlow={props.allowSlow}
                proxyDefaultMode={props.proxyDefaultMode}
                sessionReady={props.sessionReady}
                authenticated={props.authenticated}
                viewerPlayerId={props.viewerPlayerId}
              />
            )}
          </For>
        </tbody>
      </table>
    </div>
  );
}

/** 行内剩余时间倒计时。秒级节拍只在本叶子组件内订阅：整行的装备、价格与操作单元格
 * 不随秒针重渲染，只有真正显示倒计时的单元格每秒重算。 */
function LiveRowCountdown(props: { readonly remaining: string }): JSX.Element {
  const countdown = useCountdownDisplay(() => props.remaining);
  return <span class={countdownClass(countdown())}>{countdown().label}</span>;
}

/** 竞价期目录行：展示实时价格、领先与倒计时，并在竞价中的拍品上提供出价与标记命令。 */
export function LiveCatalogRow(props: {
  readonly row: CatalogRow;
  readonly visibility: CatalogVisibility;
  readonly showCommands: boolean;
  readonly canBid: boolean;
  readonly allowSlow: boolean;
  /** 该场冻结策略的代理默认模式；往期目录拼装的已结算场次为 null。 */
  readonly proxyDefaultMode: string | null;
  readonly sessionReady: boolean;
  readonly authenticated: boolean;
  readonly viewerPlayerId: string | null;
}): JSX.Element {
  // 出价与标记只属于竞价期内仍在竞价的拍品；查看入口对每一行都保留。
  const commands = createValueMemo(() => props.showCommands && props.row.status === "live");
  const detail = useCatalogDetailEntry(() => props.row, commands);
  const statusLabel = createValueMemo(() => lotStatusLabel(props.row.status));
  const statusClass = createValueMemo(() => lotStatusClass(props.row.status));
  const isLeading = createValueMemo(() => {
    const viewer = props.viewerPlayerId;
    const leader = props.row.leaderName;
    return viewer !== null && leader !== null && viewer === leader;
  });
  // “已标记”与“领先”是两种独立的行状态：标记是行首色条，领先是整行底色，可同时出现。
  const watched = useLotWatched(() => props.row.lotId);
  const rowClass = createValueMemo(() => {
    if (isLeading() && watched()) {
      return "public-auction-catalog__leading public-auction-catalog__watched";
    }
    if (isLeading()) {
      return "public-auction-catalog__leading";
    }
    if (watched()) {
      return "public-auction-catalog__watched";
    }
    return "";
  });
  const showRemaining = createValueMemo(() =>
    catalogVisibilityContains(props.visibility, "remaining"),
  );
  // 非 live 行保持原占位倒计时文案与样式；live 行的倒计时下沉到 LiveRowCountdown 叶子组件。
  const inactiveCountdown = createValueMemo((): CountdownDisplay | null => {
    if (!showRemaining() || props.row.status === "live") {
      return null;
    }
    return {
      label: t("value-empty"),
      urgent: false,
      expired: false,
      empty: true,
    };
  });
  const leader = createValueMemo(() => props.row.leaderName ?? t("value-empty"));
  const currentPrice = createValueMemo(() => ({
    short: formatCreditAmount(props.row.currentPrice),
    full: t("price-exact", {
      amount: formatCreditAmountFull(props.row.currentPrice),
    }),
  }));
  const nextBid = createValueMemo(() => {
    const value = props.row.nextBid;
    return value > MAX_PUBLIC_BID_AMOUNT
      ? { short: t("bid-limit-reached"), full: t("bid-limit-note") }
      : {
          short: formatCreditAmount(value),
          full: t("price-exact", { amount: formatCreditAmountFull(value) }),
        };
  });
  return (
    <tr
      class={rowClass()}
      // 领先由整行底色表达；同一语义以行描述提供给非视觉用户。
      aria-description={isLeading() ? t("bid-leading") : undefined}
      // 整行是详情入口：点击，或聚焦后按 Enter/Space 打开抽屉；行内控件保持各自行为。
      tabindex="0"
      onClick={(event) => detail.click(event)}
      onKeyDown={(event) => detail.keydown(event)}
    >
      <td
        hidden={!catalogVisibilityContains(props.visibility, "code")}
        data-label={t("catalog-code")}
      >
        {/* 编号只作标识；进入详情走整行点击或操作栏的查看入口。 */}
        <span class="public-auction-catalog__code">{props.row.code}</span>
      </td>
      <td
        class="public-auction-catalog__name"
        hidden={!catalogVisibilityContains(props.visibility, "item")}
        data-label={t("catalog-item")}
      >
        <CatalogEquipmentName name={props.row.name} gameEquipmentUrl={props.row.gameEquipmentUrl} />
      </td>
      <CatalogEquipmentCells row={props.row} visibility={props.visibility} />
      <td
        class="public-auction-catalog__amount public-auction-catalog__number"
        hidden={!catalogVisibilityContains(props.visibility, "price")}
        data-label={t("catalog-current")}
        title={currentPrice().full}
        aria-label={currentPrice().full}
      >
        {currentPrice().short}
      </td>
      <td
        class="public-auction-catalog__amount public-auction-catalog__number"
        hidden={!catalogVisibilityContains(props.visibility, "next_bid")}
        data-label={t("catalog-next-bid")}
        title={nextBid().full}
        aria-label={nextBid().full}
      >
        {nextBid().short}
      </td>
      <td
        class="public-auction-catalog__participant-cell"
        hidden={!catalogVisibilityContains(props.visibility, "leader")}
        data-label={t("catalog-leader")}
      >
        <Show
          when={props.row.leaderName !== null}
          fallback={<span class="public-auction-catalog__none">{leader()}</span>}
        >
          <CatalogParticipant name={leader()} marker={null} trailing={null} />
        </Show>
      </td>
      <td
        class="public-auction-catalog__remaining"
        hidden={!showRemaining()}
        data-label={t("catalog-remaining")}
      >
        {/* 隐藏时卸载倒计时叶子组件，其订阅守卫随之释放；整行和命令宿主不受影响。 */}
        <Show
          when={showRemaining() && props.row.status === "live"}
          fallback={
            <Show when={inactiveCountdown()}>
              {(countdown) => <span class={countdownClass(countdown())}>{countdown().label}</span>}
            </Show>
          }
        >
          <LiveRowCountdown remaining={props.row.remaining} />
        </Show>
      </td>
      <td
        class="public-auction-catalog__status"
        hidden={!catalogVisibilityContains(props.visibility, "status")}
        data-label={t("catalog-status")}
      >
        <span class={["public-auction-status", `public-auction-status--${statusClass()}`]}>
          {statusLabel()}
        </span>
      </td>
      <td
        class="public-auction-catalog__actions"
        hidden={!catalogVisibilityContains(props.visibility, "actions")}
        data-label={t("catalog-actions")}
      >
        {/* 列偏好只隐藏单元格，不卸载命令宿主，保留冻结的 command_id 与 unknown 状态。
            固定顺序：查看、出价、标记。查看入口每行都有；竞价表里已结束的拍品只保留查看，
            仍与同表可竞价拍品的第一个图标对齐。整表都没有命令（已结算）时单个图标居中。 */}
        <div
          class={
            props.showCommands
              ? "public-auction-catalog__action-controls"
              : "public-auction-catalog__action-controls public-auction-catalog__action-controls--view-only"
          }
        >
          <CatalogViewButton lotCode={props.row.code} onOpen={() => detail.open()} />
          <Show
            when={commands()}
            fallback={<CatalogRecoveryEntry input={recoveryDialogInput(props.row)} />}
          >
            <LiveCatalogActions
              lotId={props.row.lotId}
              lotCode={props.row.code}
              lotName={props.row.name}
              canBid={props.canBid}
              currentPrice={props.row.currentPrice}
              nextBid={props.row.nextBid}
              lotStatus={props.row.status}
              allowSlow={props.allowSlow}
              proxyDefaultMode={props.proxyDefaultMode}
              sessionReady={props.sessionReady}
              authenticated={props.authenticated}
              leading={isLeading()}
              detailBid={detail.bid}
            />
            <LotWatchButton lotId={props.row.lotId} lotCode={props.row.code} iconOnly={true} />
          </Show>
        </div>
      </td>
    </tr>
  );
}
