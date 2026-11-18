// 详情抽屉里的本人代理授权摘要：上限、已用次数、模式与状态。
// 只展示服务端按会话返回的本人授权；上限不公开，抽屉与页面级信号都由身份宿主持有，
// 身份切换时一并清空。读取由行内操作栏在抽屉打开时发起，这里不发请求、不推导状态。

import { formatCreditAmount } from "@auction/web-shared/ui/format";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";

import type { OwnedProxy } from "../../../api/account";
import { proxyModeLabel, proxyQuotaLabel, proxyStatusLabel } from "../../proxy";
import type { CatalogDetailBid } from ".";

/** 从出价入口投影里取出本人对该拍品的代理授权。
 *
 * 只有已登录的命令入口带上次确认的授权快照。抽屉沿用它展示状态，读取错误与重试留给代理弹窗。
 */
export function ownProxy(bid: CatalogDetailBid): OwnedProxy | null {
  if (bid.kind !== "command") {
    return null;
  }
  const result = bid.input.proxy;
  if (result == null || !result.ok || result.value === null) {
    return null;
  }
  return result.value;
}

/** 代理状态对应公开样式的修饰名；未知状态保留可见状态文本并落到 `unknown`。 */
function proxyStatusClass(status: string): string {
  switch (status) {
    case "active":
      return "active";
    case "final_fast":
      return "final-fast";
    case "exhausted":
      return "exhausted";
    case "frozen":
      return "frozen";
    case "closed":
      return "closed";
    default:
      return "unknown";
  }
}

/** “我的代理出价”分区；样式沿用抽屉的事实网格与分区标题。 */
export function DetailOwnProxy(props: { readonly proxy: OwnedProxy }): JSX.Element {
  const title = () => t("proxy-summary-title");
  const statusClass = () => proxyStatusClass(props.proxy.status);
  return (
    <section class="public-catalog-detail__section" aria-label={title()}>
      <div class="public-catalog-detail__section-head">
        <h3 class="public-catalog-detail__section-title">{title()}</h3>
      </div>
      <dl class="public-catalog-detail__facts">
        <div>
          <dt>{t("account-maximum")}</dt>
          <dd>{formatCreditAmount(props.proxy.maximum_amount)}</dd>
        </div>
        <div>
          <dt>{t("account-proxy-uses")}</dt>
          <dd>{proxyQuotaLabel(props.proxy)}</dd>
        </div>
        <div>
          <dt>{t("account-mode")}</dt>
          <dd>{proxyModeLabel(props.proxy.mode)}</dd>
        </div>
        <div>
          <dt>{t("account-status")}</dt>
          <dd>
            <span
              class={[
                "public-auction-proxy-status",
                `public-auction-proxy-status--${statusClass()}`,
              ]}
            >
              {proxyStatusLabel(props.proxy.status)}
            </span>
          </dd>
        </div>
      </dl>
    </section>
  );
}
