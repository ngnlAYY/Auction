// Admin 拍卖列表行与动作入口。

import { useSecondTick } from "@auction/web-shared/platform/clock";
import type { Cell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { Time } from "@auction/web-shared/ui/components/time";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";

import type { AdminAuction, AdminAuctionCapabilities } from "../../../api/auctions";
import { adminWorldLabel } from "../../presentation";
import { adminAuctionStatusLabel, currentTimeMillis, scheduledTimeMillis } from "../presentation";
import { AdminAuctionActions } from "./dialog";
import { type AdminAuctionNotice, pendingOpenMillis } from "./model";

/** 一行拍卖的展示输入与列表共享的成功提示槽。 */
export interface AdminAuctionRowProps {
  readonly auction: AdminAuction;
  readonly capabilities: AdminAuctionCapabilities;
  readonly editsReady: boolean;
  readonly notice: Cell<AdminAuctionNotice | null>;
}

/** 拍卖列表行：展示状态与计划时间，并在就绪单元格里按需挂载秒级时钟。 */
export function AdminAuctionRow(props: AdminAuctionRowProps): JSX.Element {
  const endsAt = createValueMemo(() => {
    const value = props.auction.scheduled_ends_at;
    return value === null ? null : { value };
  });
  return (
    <tr class="admin-auctions__row">
      <td data-label={t("admin-auctions-sequence")}>{`#${props.auction.sequence}`}</td>
      <td data-label={t("admin-auctions-world")}>{adminWorldLabel(props.auction.world_id)}</td>
      <td data-label={t("admin-auctions-status")}>
        <span class={["admin-auctions__status", `admin-auctions__status--${props.auction.status}`]}>
          {adminAuctionStatusLabel(props.auction.status)}
        </span>
      </td>
      <td data-label={t("admin-auctions-open-at")}>
        <Time value={props.auction.scheduled_open_at} />
      </td>
      <td data-label={t("admin-auctions-end-at")}>
        <Show when={endsAt()} fallback={t("value-empty")}>
          {(ends) => <Time value={ends().value} />}
        </Show>
      </td>
      <AdminAuctionReadinessCell
        status={props.auction.status}
        manualEarlyOpen={props.capabilities.manual_early_open}
        scheduledOpenAt={props.auction.scheduled_open_at}
      />
      <td data-label={t("admin-auctions-actions")}>
        <AdminAuctionActions
          auction={props.auction}
          capabilities={props.capabilities}
          editsReady={props.editsReady}
          notice={props.notice}
        />
      </td>
    </tr>
  );
}

/** 「可开拍」就绪单元格。只有渲染时仍在等待开拍时间的预览场次挂载秒级时钟叶子；其余状态
 * 按渲染时的时间一次算定，不订阅时钟。到点后叶子仍挂载并继续订阅，直到本行因列表刷新等
 * 原因重新渲染才卸载；页面上不再有挂载中的时钟叶子时，共享 interval 才停止。 */
function AdminAuctionReadinessCell(props: {
  readonly status: string;
  readonly manualEarlyOpen: boolean;
  readonly scheduledOpenAt: string;
}): JSX.Element {
  const state = createValueMemo(() => {
    const now = currentTimeMillis();
    const scheduled = scheduledTimeMillis(props.scheduledOpenAt);
    return {
      waitingUntil: pendingOpenMillis(
        props.status,
        props.manualEarlyOpen,
        props.scheduledOpenAt,
        now,
      ),
      openReady:
        props.status === "preview" &&
        (props.manualEarlyOpen || (scheduled !== null && scheduled <= now)),
    };
  });
  const waiting = createValueMemo(() => {
    const millis = state().waitingUntil;
    return millis === null ? null : { millis };
  });
  return (
    <td data-label={t("admin-auctions-readiness")}>
      <Show
        when={waiting()}
        fallback={
          <Show when={state().openReady} fallback={t("value-empty")}>
            <span class="admin-auctions__ready">{t("admin-auctions-ready")}</span>
          </Show>
        }
      >
        {(scheduled) => <AdminAuctionReadyClock scheduledOpenMillis={scheduled().millis} />}
      </Show>
    </td>
  );
}

/** 等待开拍时间的就绪标记：唯一订阅秒级节拍的就绪单元，到点后显示“可开拍”。 */
function AdminAuctionReadyClock(props: { readonly scheduledOpenMillis: number }): JSX.Element {
  const secondTick = useSecondTick();
  const ready = createValueMemo(() => {
    secondTick();
    return currentTimeMillis() >= props.scheduledOpenMillis;
  });
  return (
    <Show when={ready()} fallback={t("value-empty")}>
      <span class="admin-auctions__ready">{t("admin-auctions-ready")}</span>
    </Show>
  );
}
