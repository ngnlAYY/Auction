// 代理命令成功后的安全反馈面板。

import { formatCreditAmount } from "@auction/web-shared/ui/format";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";

import type { ProxyCommandAuthorization } from "../../../api/commands";
import { proxyStatusLabel, proxyTransitionLabel } from "../../proxy";

/** 代理命令成功后的安全反馈；只回显服务端返回的授权投影。 */
export function ProxySuccessPanel(props: {
  readonly authorization: ProxyCommandAuthorization;
  readonly onClose: () => void;
}): JSX.Element {
  const successMessage = () => {
    const bid = props.authorization.bid;
    return bid?.leading === true
      ? t("proxy-leading-now", { price: formatCreditAmount(bid.current_price) })
      : t("proxy-saved");
  };
  const quota = () => {
    const value = props.authorization.quota;
    return value.limit === 0
      ? t("account-quota-unlimited", { used: value.used })
      : t("account-quota-bounded", {
          used: value.used,
          limit: value.limit,
          remaining: Math.max(value.limit - value.used, 0),
        });
  };
  const transitions = () => {
    const value = props.authorization.transitions;
    if (value.length === 0) {
      return t("value-empty");
    }
    return value
      .map(
        (transition) =>
          `${transition.transition_number}: ${proxyTransitionLabel(transition.kind)} · ${formatCreditAmount(transition.new_maximum_amount)}`,
      )
      .join(", ");
  };
  const status = () => proxyStatusLabel(props.authorization.status);

  return (
    <section class="public-auction-bid-form">
      <p class="action-feedback action-feedback--success" role="status">
        {successMessage()}
      </p>
      <p class="public-auction-bid-form__meta">
        {t("proxy-status", { status: status() })}
        {" · "}
        {quota()}
      </p>
      <p class="public-auction-bid-form__meta public-auction-bid-form__history">
        {t("account-transition-history")}
        {": "}
        {transitions()}
      </p>
      <div class="public-auction-bid-form__actions">
        <button type="button" onClick={() => props.onClose()}>
          {t("catalog-close")}
        </button>
      </div>
    </section>
  );
}
