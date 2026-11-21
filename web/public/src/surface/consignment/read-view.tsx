// 公开寄售页面共用的读取反馈、装备等级和两种挂单说明。

import "../../../../../crates/auction-web-public/assets/consignment.css";

import type {
  EquipmentLevel,
  ListingSummary,
} from "@auction/contracts/dto/public/consignment/catalog";
import { unreachable } from "@auction/web-shared/result";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { formatCreditAmount } from "@auction/web-shared/ui/format";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { Match, Switch } from "solid-js";

import type { ConsignmentReadError } from "../../api/consignment-status";
import { type ConsignmentCapability, consignmentEnabled } from "./capability";
import { consignmentWorldLabel } from "./filters";
import { listingModeKey, listingStateKey } from "./labels";

/** 只使用本地译文，未知响应永远不把正文或错误码带进页面。 */
export function ConsignmentReadNotice(props: {
  readonly error: ConsignmentReadError;
  readonly onRetry: () => void;
}): JSX.Element {
  const title = () => {
    const error = props.error;
    switch (error.kind) {
      case "disabled":
        return t("consignment-disabled");
      case "not-found":
        return t("consignment-not-found");
      case "invalid-query":
        // 合同未给通用筛选错误分配专用键，从已有文案提供可执行的清除入口。
        return t("history-filter-settings");
      case "unavailable":
        return t("consignment-unavailable");
      case "invalid-response":
        return t("error-invalid-response");
      case "transport":
        return error.error.kind === "timeout" ? t("error-request-timeout") : t("error-network");
      default:
        return unreachable(error);
    }
  };
  const canRetry = () => props.error.kind !== "disabled" && props.error.kind !== "not-found";
  return (
    <Switch fallback={<AsyncState kind="empty" title={title()} detail="" />}>
      <Match when={canRetry()}>
        <AsyncState
          kind="error"
          title={title()}
          detail={props.error.kind === "invalid-query" ? t("history-empty-hint") : ""}
          retryLabel={
            props.error.kind === "invalid-query" ? t("history-filter-clear") : t("data-retry")
          }
          onRetry={props.onRetry}
        />
      </Match>
    </Switch>
  );
}

/** 未确认开启前只显示能力状态，不渲染公开读取的内容。 */
export function ConsignmentCapabilityGate(props: {
  readonly capability: ConsignmentCapability | undefined;
  readonly onRetry: () => void;
  readonly children: JSX.Element;
}): JSX.Element {
  return (
    <Switch fallback={<AsyncState kind="loading" title={t("data-loading")} detail="" />}>
      <Match when={consignmentEnabled(props.capability)}>{props.children}</Match>
      <Match when={props.capability?.kind === "disabled"}>
        <AsyncState kind="empty" title={t("consignment-disabled")} detail="" />
      </Match>
      <Match when={props.capability?.kind === "failed"}>
        <AsyncState
          kind="error"
          title={t("consignment-unavailable")}
          detail=""
          retryLabel={t("data-retry")}
          onRetry={props.onRetry}
        />
      </Match>
    </Switch>
  );
}

/** 等级使用已解码的判别联合。未分配沿用目录占位，魂绑在公开契约中会被拒绝。 */
export function consignmentLevelLabel(level: EquipmentLevel): string {
  switch (level.kind) {
    case "assigned":
      return String(level.value);
    case "unassigned":
      return "—";
    case "soulbound":
      // 现有五语译文没有魂绑等级标签，以已有的不可交易文案安全展示。
      return t("consignment-not-tradeable");
    default:
      return unreachable(level);
  }
}

/** 卡片与公开详情共用的挂单事实，仅接受公开投影。 */
export function ListingFacts(props: { readonly listing: ListingSummary }): JSX.Element {
  return (
    <dl class="public-consignment-listing__facts">
      <div>
        <dt>{t("consignment-price")}</dt>
        <dd>{formatCreditAmount(props.listing.price)}</dd>
      </div>
      <div>
        <dt>{t("consignment-seller")}</dt>
        <dd>{props.listing.seller_name}</dd>
      </div>
      <div>
        <dt>{t("consignment-world")}</dt>
        <dd>{consignmentWorldLabel(props.listing.world_id)}</dd>
      </div>
      <div>
        <dt>{t("consignment-mode")}</dt>
        <dd>{t(listingModeKey(props.listing.mode))}</dd>
      </div>
    </dl>
  );
}

/** 待寄只展示尚未代管的提示，游戏内联系说明只用于非寄售。 */
export function ModeNotice(props: { readonly listing: ListingSummary }): JSX.Element {
  return (
    <p class="public-consignment-listing__notice">
      {props.listing.mode === "direct"
        ? t("consignment-direct-notice")
        : t("consignment-deposit-notice")}
    </p>
  );
}

/** 公开状态显示使用寄售闭集映射。 */
export function publicListingStateLabel(state: ListingSummary["state"]): string {
  const key = listingStateKey(state);
  return key === null ? "" : t(key);
}
