// 竞价命令组件共用的短生命周期状态和安全错误映射。

import { MAX_PUBLIC_BID_AMOUNT } from "@auction/contracts/dto/public/commands";
import type { ConfirmCommand } from "@auction/web-shared/net/commands";
import { type BaseErrorKind, baseApiErrorMessage } from "@auction/web-shared/net/errors";
import { unreachable } from "@auction/web-shared/result";
import { formatCreditAmount } from "@auction/web-shared/ui/format";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { createContext } from "solid-js";

import type { CommandError } from "../../api/commands";
import { parseCreditAmount } from "../catalog/presentation";
import { rustTrim } from "../query/text";

/** 已冻结的普通竞价命令负载；同一确认输入复用该命令的 `commandId`。 */
export interface PendingBidCommand {
  readonly userId: string;
  readonly amount: number;
  readonly commandId: string;
}

/** 已冻结的代理命令负载；创建与降低上限共享同一确认与重试边界。 */
export type PendingProxyCommand =
  | {
      readonly kind: "create";
      readonly userId: string;
      readonly amount: number;
      readonly mode: string;
      readonly commandId: string;
    }
  | {
      readonly kind: "lower";
      readonly userId: string;
      readonly amount: number;
      readonly authorizationId: string;
      readonly commandId: string;
    };

/** 确认宿主的固定身份；不能在发送或重试时替换成新会话的用户。 */
export interface PublicCommandIdentity {
  readonly userId: string;
}

/** 当前身份宿主的固定用户；发送与重试不得替换成新会话的用户。 */
export const PublicCommandIdentityContext = createContext<PublicCommandIdentity>();

/** 普通竞价的两阶段确认命令 hook。由弹窗宿主持有，表单经 prop 共享。 */
export type BidConfirmCommand = ConfirmCommand<PendingBidCommand, CommandError>;

/** 代理授权（创建/降低上限各持有一份实例）的两阶段确认命令 hook。 */
export type ProxyConfirmCommand = ConfirmCommand<PendingProxyCommand, CommandError>;

/** 二次确认面板的公共骨架：类型与金额两行，加约束性命令说明。
 * 面板只负责展示；命令冻结、校验与提交仍由各表单持有，组件替换不能取消已提交命令。
 * 文案通过普通值 props 传入，由本组件的 JSX 读取，语言或确认数据变化时同步更新。 */
export function ConfirmPanel(props: {
  readonly id: string;
  readonly typeLabel: string;
  readonly amount: string;
  readonly note: string;
}): JSX.Element {
  return (
    <>
      <dl class="public-bid-confirm__grid">
        <div>
          <dt>{t("bid-type-label")}</dt>
          <dd>{props.typeLabel}</dd>
        </div>
        <div>
          <dt>{t("bid-amount-confirm-label")}</dt>
          <dd>{props.amount}</dd>
        </div>
      </dl>
      <p
        id={props.id}
        class="public-bid-confirm__note"
        role="status"
        aria-live="polite"
        aria-atomic="true"
      >
        {props.note}
      </p>
    </>
  );
}

/** 只为代理输入框生成可解析的默认值：向上取到 1K 网格，不修改服务端下限或用户输入。 */
export function initialProxyAmount(minimum: number): string {
  if (minimum < 1 || minimum > MAX_PUBLIC_BID_AMOUNT) {
    return "";
  }
  const amount = Math.trunc((minimum + 999) / 1_000) * 1_000;
  return formatCreditAmount(amount);
}

/** 显示金额输入的换算提示；空值不渲染，非法值给出该字段的专用错误文案。 */
export function amountHint(value: string, id: string, invalidKey: string): JSX.Element {
  if (rustTrim(value) === "") {
    // biome-ignore lint/complexity/noUselessFragments: 源码在空值时不渲染任何节点。
    return <></>;
  }
  const amount = parseCreditAmount(value);
  if (amount !== null) {
    return (
      <p id={id} class="public-auction-bid-form__amount-hint" role="status">
        {`= ${formatCreditAmount(amount)}`}
      </p>
    );
  }
  return (
    <p
      id={id}
      class="public-auction-bid-form__amount-hint public-auction-bid-form__amount-hint--invalid"
      role="alert"
    >
      {t(invalidKey)}
    </p>
  );
}

/** 竞价命令失败的本地化文案：先匹配竞价/代理业务码与 401 会话过期规则，未命中走共享
 * 基础层（`baseApiErrorMessage` 的传输/通用码），最后落到竞价通用兜底。 */
export function bidErrorMessage(error: CommandError, minimum: number): string {
  switch (error.code) {
    case "invalid_amount":
      return t("bid-invalid-amount");
    case "bid_too_low":
      return t("bid-below-minimum", { price: formatCreditAmount(minimum) });
    case "unauthenticated":
      return t("command-login-required");
    case "not_found":
      return t("command-not-found");
    case "bidder_ineligible":
      return t("bid-ineligible");
    case "already_leading":
      return t("bid-already-leading");
    case "active_proxy_exists":
      return t("proxy-active-exists");
    case "invalid_policy":
      return t("proxy-invalid-policy");
    case "auction_not_open":
      return t("bid-unavailable");
    case "command_conflict":
      return t("command-conflict");
    default:
      if (error.status === 401) {
        return t("command-session-expired");
      }
  }
  return (
    baseApiErrorMessage(error.code, (kind) => baseErrorMessage(kind, "bid-error")) ?? t("bid-error")
  );
}

/** 代理授权命令失败的本地化文案；映射顺序与 `bidErrorMessage` 相同。 */
export function proxyErrorMessage(error: CommandError, minimum: number): string {
  switch (error.code) {
    case "invalid_amount":
    case "invalid_maximum_amount":
      return t("proxy-invalid-amount");
    case "bid_too_low":
      return t("proxy-below-minimum", { price: formatCreditAmount(minimum) });
    case "unauthenticated":
      return t("command-login-required");
    case "not_found":
      return t("command-not-found");
    case "bidder_ineligible":
      return t("bid-ineligible");
    case "already_leading":
      return t("bid-already-leading");
    case "active_proxy_exists":
      return t("proxy-active-exists");
    case "proxy_authorization_not_active":
      return t("proxy-authorization-not-active");
    case "proxy_maximum_not_lower":
      return t("proxy-maximum-not-lower");
    case "proxy_maximum_below_binding":
      return t("proxy-maximum-below-binding");
    case "proxy_reentry_maximum_not_higher":
      return t("proxy-reentry-maximum-not-higher");
    case "proxy_outbid_limit":
      return t("proxy-outbid-limit");
    case "proxy_disabled":
      return t("proxy-disabled-error");
    case "proxy_slow_final":
      return t("proxy-slow-final-error");
    case "invalid_policy":
      return t("proxy-invalid-policy");
    case "auction_not_open":
      return t("bid-unavailable");
    case "command_conflict":
      return t("command-conflict");
    default:
      if (error.status === 401) {
        return t("command-session-expired");
      }
  }
  return (
    baseApiErrorMessage(error.code, (kind) => baseErrorMessage(kind, "proxy-command-error")) ??
    t("proxy-command-error")
  );
}

function baseErrorMessage(kind: BaseErrorKind, fallbackKey: string): string {
  switch (kind) {
    case "network":
      return t("error-network");
    case "timeout":
      return t("error-request-timeout");
    case "invalid_response":
      return t("error-invalid-response");
    case "unauthenticated":
      return t("command-login-required");
    case "not_found":
      return t("command-not-found");
    case "conflict":
      return t("command-conflict");
    case "forbidden":
    case "rate_limited":
    case "unavailable":
    case "feature_not_supported":
    case "invalid_request":
      return t(fallbackKey);
    default:
      return unreachable(kind);
  }
}
