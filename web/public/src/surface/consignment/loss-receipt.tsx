// 损失案件回执页：卖家只读查看代管件灭失的案件状态，页面没有任何付款、领取或发行入口。
//
// 数据只来自服务端的案件回执；严格闭集解码保证服务端若附带金额等额外字段即整体失败，
// 不会被照单展示。

import { createTaskResource } from "@auction/web-shared/reactive/resource";
import type { RoutePageProps } from "@auction/web-shared/router";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { Match, Show, Switch } from "solid-js";
import type { LossCase } from "../../api/consignment-loss";
import { fetchLossCase } from "../../api/consignment-loss";
import { validConsignmentUuid } from "../../api/consignment-private";

import { PrivateConsignmentPage, PrivateReadError } from "./private-page";
import { ConsignmentEmpty } from "./read-view";
import { useConsignmentRefresh } from "./refresh";

function stateKey(state: LossCase["state"]): string {
  switch (state) {
    case "opened":
      return "consignment-loss-recorded";
    case "reviewing":
      return "consignment-loss-reviewing";
    case "closed":
      return "consignment-loss-closed";
  }
}

/** 地址中的代管件 ID 先经规范 UUID 校验；非法值不发起请求。 */
export function LossReceiptPage(props: RoutePageProps): JSX.Element {
  const positionId = () => {
    const value = props.params.position_id ?? "";
    return validConsignmentUuid(value) ? value : null;
  };
  return (
    <PrivateConsignmentPage title={t("consignment-loss-title")}>
      {(userId, canUse) => (
        <Show
          when={positionId()}
          keyed
          fallback={<ConsignmentEmpty title={t("consignment-not-found")} />}
        >
          {(id) => <LossReceipt userId={userId} positionId={id} canUse={canUse} />}
        </Show>
      )}
    </PrivateConsignmentPage>
  );
}

function LossReceipt(props: {
  readonly userId: string;
  readonly positionId: string;
  readonly canUse: () => boolean;
}): JSX.Element {
  const read = createTaskResource(props.canUse, async (authorized, context) =>
    authorized ? fetchLossCase(props.userId, props.positionId, context.signal) : null,
  );
  // 案件状态由后台审理推进且没有公开订阅；可见与恢复焦点时重读权威回执，避免长期显示过期阶段。
  useConsignmentRefresh(props.canUse, () => {
    if (props.canUse()) read.restart();
  });
  const loaded = () => {
    const result = read();
    return result?.ok ? result.value : null;
  };
  const failure = () => {
    const result = read();
    return result?.ok === false ? result.error : null;
  };
  return (
    <Switch fallback={<AsyncState kind="loading" title={t("data-loading")} detail="" />}>
      <Match when={failure()}>
        {(error) => <PrivateReadError error={error()} retry={read.restart} />}
      </Match>
      <Match when={loaded()}>
        {(current) => (
          <section class="public-consignment-account__detail">
            <h2>{t(stateKey(current().state))}</h2>
            <p>{t("consignment-loss-explain")}</p>
          </section>
        )}
      </Match>
    </Switch>
  );
}
