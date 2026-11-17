// Public 代理授权的展示文案。
// 本模块只格式化服务端已经过滤的代理投影，不在浏览器端推导授权状态机或竞价结果。

import { formatCreditAmount } from "@auction/web-shared/ui/format";
import { t } from "@auction/web-shared/ui/i18n";

import type { OwnedProxy } from "../api/account";

/** 代理模式徽标的本地化文案；未知值按空值处理。 */
export function proxyModeLabel(mode: string): string {
  switch (mode) {
    case "fast":
      return t("proxy-mode-fast");
    case "slow":
      return t("proxy-mode-slow");
    default:
      return t("value-empty");
  }
}

/** 代理状态迁移的本地化文案；未知迁移按空值处理。 */
export function proxyTransitionLabel(kind: string): string {
  switch (kind) {
    case "created":
      return t("account-transition-created");
    case "lowered":
      return t("account-transition-lowered");
    case "final_fast":
      return t("account-transition-final-fast");
    case "exhausted":
      return t("account-transition-exhausted");
    case "frozen":
      return t("account-transition-frozen");
    case "closed":
      return t("account-transition-closed");
    default:
      return t("value-empty");
  }
}

/** 代理授权状态的本地化文案；未知状态按空值处理。 */
export function proxyStatusLabel(status: string): string {
  switch (status) {
    case "active":
      return t("account-proxy-status-active");
    case "final_fast":
      return t("account-proxy-status-final-fast");
    case "exhausted":
      return t("account-proxy-status-exhausted");
    case "frozen":
      return t("account-proxy-status-frozen");
    case "closed":
      return t("account-proxy-status-closed");
    default:
      return t("value-empty");
  }
}

/** 代理上限配额的展示文案；无限额度使用专用格式。 */
export function proxyQuotaLabel(proxy: OwnedProxy): string {
  if (proxy.quota.limit === 0) {
    return t("account-quota-unlimited", { used: proxy.quota.used });
  }
  return t("account-quota-bounded", {
    used: proxy.quota.used,
    limit: proxy.quota.limit,
    remaining: Math.max(proxy.quota.limit - proxy.quota.used, 0),
  });
}

/** 按服务端顺序拼接代理迁移历史；空历史显示空值文案。 */
export function proxyTransitionSummary(proxy: OwnedProxy): string {
  if (proxy.transitions.length === 0) {
    return t("value-empty");
  }
  return proxy.transitions
    .map(
      (transition) =>
        `${transition.transition_number}: ${proxyTransitionLabel(transition.kind)} · ${formatCreditAmount(transition.new_maximum_amount)}`,
    )
    .join(", ");
}
