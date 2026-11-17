// Public 拍卖目录的实时、预览和参与者行，以及竞价历史的展示行。

import type { PublicBidLogEntryResponse } from "@auction/contracts/dto/public/catalog/history";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { TruncatedText } from "@auction/web-shared/ui/components/truncated-text";
import { formatCreditAmount } from "@auction/web-shared/ui/format";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { Match, Show, Switch } from "solid-js";

import type { CatalogRow } from "../../api/model";
import type { CatalogVisibility } from "./columns/state";
import { catalogVisibilityContains } from "./columns/state";
import { CatalogViewButton, useCatalogDetailEntry } from "./detail/entry";
import { formatOneDecimal } from "./detail/stat-format";
import { equipmentGroupLabel, lotStatusClass, lotStatusLabel } from "./presentation";

/** 把一页竞价记录展开成展示行：代理对代理竞争中落败的授权按一次代理出价（金额为其上限）
 * 紧跟在胜者出价之后——降序列表里即在其下方、时间上先于它——说明胜者为何以该价出价。
 * 落败行不领先，也不计入分页总数；键在出价 ID 后加后缀，与真实出价互不冲突。 */
export function bidHistoryRows(
  bids: readonly PublicBidLogEntryResponse[],
): PublicBidLogEntryResponse[] {
  const rows: PublicBidLogEntryResponse[] = [];
  for (const bid of bids) {
    // 输入数组可能由调用方持有，必须克隆行而不是改写原条目。
    const winner: PublicBidLogEntryResponse = { ...bid, defeated_proxy: null };
    rows.push(winner);
    const defeated = bid.defeated_proxy;
    if (defeated != null) {
      rows.push({
        ...winner,
        id: `${bid.id}-defeated`,
        bidder_name: defeated.bidder_name,
        amount: defeated.maximum_amount,
        source: "proxy",
        source_post_number: null,
        source_post_url: null,
        valid: false,
        mechanism: "proxy",
      });
    }
  }
  return rows;
}

/** 公开拍卖参与者标识；长名称保持可读的省略样式，并保留旧版的代理/当前用户标记。 */
export function CatalogParticipant(props: {
  readonly name: string;
  readonly marker?: string | null;
  readonly trailing?: string | null;
}): JSX.Element {
  return (
    <span class="public-auction-participant-identity">
      <Show when={props.marker != null}>
        <span class="public-auction-participant-identity__marker public-auction-participant-identity__marker--leading">
          {props.marker}
        </span>
      </Show>
      <TruncatedText
        value={props.name}
        size="compact"
        className="public-auction-participant-identity__name"
        portal={true}
        legacyDom={true}
      />
      <Show when={props.trailing != null}>
        <span class="public-auction-participant-identity__marker public-auction-participant-identity__marker--trailing">
          {props.trailing}
        </span>
      </Show>
    </span>
  );
}

/** 竞价历史明确标记普通/代理来源；论坛楼层紧贴玩家名称并链接真实帖子。 */
export function BidHistoryParticipant(props: {
  readonly name: string;
  readonly source: string | null;
  readonly sourcePostNumber: number | null;
  readonly sourcePostUrl: string | null;
}): JSX.Element {
  const forumSource = createValueMemo(
    (): { readonly postNumber: number; readonly url: string | null } | null => {
      if (props.source !== "forum") {
        return null;
      }
      const postNumber = props.sourcePostNumber ?? null;
      if (postNumber === null) {
        return null;
      }
      return { postNumber, url: props.sourcePostUrl ?? null };
    },
  );
  const forumUnknown = createValueMemo(() => props.source === "forum" && forumSource() === null);
  const proxy = createValueMemo(() => props.source === "proxy");
  const ordinary = createValueMemo(() => props.source == null);
  return (
    <span class="public-auction-participant-identity public-auction-participant-identity--source">
      <TruncatedText
        value={props.name}
        size="compact"
        className="public-auction-participant-identity__name"
        portal={true}
        legacyDom={true}
      />
      <Switch>
        <Match when={forumSource()}>
          {(source) => (
            <Show
              when={source().url !== null}
              fallback={
                <span class="public-auction-participant-identity__marker public-auction-participant-identity__marker--trailing">
                  {t("bidlog-source-forum", { post_number: source().postNumber })}
                </span>
              }
            >
              <a
                class="public-auction-participant-identity__marker public-auction-participant-identity__marker--trailing"
                href={source().url ?? ""}
                target="_blank"
                rel="noopener noreferrer"
                aria-label={t("bidlog-source-forum-link-label", {
                  post_number: source().postNumber,
                })}
              >
                {t("bidlog-source-forum", { post_number: source().postNumber })}
              </a>
            </Show>
          )}
        </Match>
        <Match when={forumUnknown()}>
          <span class="public-auction-participant-identity__marker public-auction-participant-identity__marker--trailing">
            {t("bidlog-source-forum-unknown")}
          </span>
        </Match>
        <Match when={proxy()}>
          <span class="public-auction-participant-identity__marker public-auction-participant-identity__marker--trailing">
            {t("bidlog-source-proxy")}
          </span>
        </Match>
        <Match when={ordinary()}>
          <span class="public-auction-participant-identity__marker public-auction-participant-identity__marker--trailing">
            {t("bidlog-source-ordinary")}
          </span>
        </Match>
      </Switch>
    </span>
  );
}

/** 装备列仅展示名称，保持单行截断并直接导航真实装备 URL。 */
export function CatalogEquipmentName(props: {
  readonly name: string;
  readonly gameEquipmentUrl: string;
}): JSX.Element {
  return (
    <span class="public-auction-catalog__equipment">
      <Show
        when={props.gameEquipmentUrl !== ""}
        fallback={
          <TruncatedText
            value={props.name}
            className="ui-equipment-name"
            portal={true}
            legacyDom={true}
          />
        }
      >
        <a
          class="public-auction-catalog__equipment-link"
          href={props.gameEquipmentUrl}
          target="_blank"
          rel="noopener noreferrer"
          aria-label={props.name}
        >
          {/* 截断文本已有自己的浮层；不再叠加浏览器原生 title 提示框。 */}
          <TruncatedText
            value={props.name}
            className="ui-equipment-name"
            focusable={false}
            portal={true}
            legacyDom={true}
          />
        </a>
      </Show>
    </span>
  );
}

/** 预览行与正式行共用的装备属性单元格（分组、平均 roll、等级、主属性、卖家）；列偏好只隐藏单元格。 */
export function CatalogEquipmentCells(props: {
  readonly row: CatalogRow;
  readonly visibility: CatalogVisibility;
}): JSX.Element {
  const level = createValueMemo(() =>
    props.row.level === null ? t("value-empty") : String(props.row.level),
  );
  const primary = createValueMemo(() => props.row.primaryAttributeInitials ?? t("value-empty"));
  const averageRoll = createValueMemo(() =>
    props.row.avgRoll === null ? null : `${formatOneDecimal(props.row.avgRoll)}%`,
  );
  return (
    <>
      <td
        hidden={!catalogVisibilityContains(props.visibility, "group")}
        data-label={t("catalog-group")}
      >
        <TruncatedText
          value={equipmentGroupLabel(props.row.equipmentGroup)}
          portal={true}
          legacyDom={true}
        />
      </td>
      <td
        class="public-auction-catalog__avg-roll public-auction-catalog__number"
        hidden={!catalogVisibilityContains(props.visibility, "average_roll")}
        data-label={t("equipment-avg-roll")}
      >
        <Show
          when={averageRoll() !== null}
          fallback={<span class="public-auction-catalog__none">{t("value-empty")}</span>}
        >
          {averageRoll()}
        </Show>
      </td>
      <td
        class="public-auction-catalog__level"
        hidden={!catalogVisibilityContains(props.visibility, "level")}
        data-label={t("catalog-level")}
      >
        {level()}
      </td>
      <td
        class="public-auction-catalog__primary-attributes"
        hidden={!catalogVisibilityContains(props.visibility, "attributes")}
        data-label={t("catalog-primary-attributes")}
      >
        {primary()}
      </td>
      <td
        class="public-auction-catalog__seller"
        hidden={!catalogVisibilityContains(props.visibility, "seller")}
        data-label={t("catalog-seller")}
      >
        <TruncatedText value={props.row.sellerName} portal={true} legacyDom={true} />
      </td>
    </>
  );
}

/** 预览期没有出价与标记，抽屉只展示拍品事实。 */
export function PreviewCatalogRow(props: {
  readonly row: CatalogRow;
  readonly visibility: CatalogVisibility;
}): JSX.Element {
  const detail = useCatalogDetailEntry(
    () => props.row,
    () => false,
  );
  const statusLabel = createValueMemo(() => lotStatusLabel(props.row.status));
  const statusClass = createValueMemo(() => lotStatusClass(props.row.status));
  return (
    <tr
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
        data-label={t("catalog-start-price")}
      >
        {formatCreditAmount(props.row.startPrice)}
      </td>
      <td
        class="public-auction-catalog__participant-cell"
        hidden={!catalogVisibilityContains(props.visibility, "leader")}
        data-label={t("catalog-leader")}
      >
        <span class="public-auction-catalog__none">{t("value-empty")}</span>
      </td>
      <td
        class="public-auction-catalog__remaining"
        hidden={!catalogVisibilityContains(props.visibility, "remaining")}
        data-label={t("catalog-remaining")}
      >
        <span class="public-auction-countdown--none">{t("value-empty")}</span>
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
        {/* 预览表没有命令，操作列只有居中的查看入口。 */}
        <div class="public-auction-catalog__action-controls public-auction-catalog__action-controls--view-only">
          <CatalogViewButton lotCode={props.row.code} onOpen={() => detail.open()} />
        </div>
      </td>
    </tr>
  );
}
