// 本人订单详情：按角色显示金额与各阶段，只读；非本人与不存在同为未找到。

import type { OrderV2 } from "@auction/contracts/dto/public/consignment/order-v2";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import { Link, type RoutePageProps } from "@auction/web-shared/router";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { Time } from "@auction/web-shared/ui/components/time";
import { formatCreditAmount } from "@auction/web-shared/ui/format";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { Match, Show, Switch } from "solid-js";

import { fetchOwnedOrder } from "../../api/consignment-orders";
import { consignmentWorldLabel } from "./filters";
import { orderAmountRows } from "./order-amounts";
import { orderKindKey, orderRoleKey, orderStateKey, stageKey } from "./order-labels";
import { PrivateConsignmentPage, PrivateReadError } from "./private-page";
import { ConsignmentEmpty } from "./read-view";
import { useConsignmentRefresh } from "./refresh";

/** 路由 `/consignment/orders/:contract_id`：认证的本人订单详情。 */
export function OrderDetail(props: RoutePageProps): JSX.Element {
  return (
    <PrivateConsignmentPage title={t("consignment-order-detail")}>
      {(userId, canUse) => (
        <OrderView userId={userId} contractId={props.params.contract_id ?? ""} canUse={canUse} />
      )}
    </PrivateConsignmentPage>
  );
}

function OrderView(props: {
  readonly userId: string;
  readonly contractId: string;
  readonly canUse: () => boolean;
}): JSX.Element {
  const request = createValueMemo(() => (props.canUse() ? { contractId: props.contractId } : null));
  const read = createTaskResource(request, async (requested, context) =>
    requested === null
      ? null
      : {
          requested,
          result: await fetchOwnedOrder(props.userId, requested.contractId, context.signal),
        },
  );
  // 订单阶段事件（consignment.order_stage_changed）没有公开订阅；页面只读权威接口，
  // 可见与恢复焦点时按固定间隔重读，不在浏览器重建订单状态机。
  useConsignmentRefresh(props.canUse, () => {
    if (props.canUse()) read.restart();
  });
  const current = () => {
    const loaded = read();
    return loaded != null && props.canUse() && loaded.requested === request()
      ? loaded.result
      : null;
  };

  return (
    <>
      <nav class="public-consignment-catalog__actions">
        <Link href="/consignment/orders">{t("consignment-orders")}</Link>
        <button type="button" onClick={() => read.restart()}>
          {t("consignment-order-refresh")}
        </button>
      </nav>
      <Switch fallback={<AsyncState kind="loading" title={t("data-loading")} detail="" />}>
        <Match when={current()}>
          {(loaded) => {
            const order = () => {
              const result = loaded();
              return result.ok ? result.value : null;
            };
            const failure = () => {
              const result = loaded();
              return result.ok ? null : result.error;
            };
            return (
              <>
                <Show when={failure()}>
                  {(error) => (
                    <Show
                      when={error().status === 404}
                      fallback={<PrivateReadError error={error()} retry={read.restart} />}
                    >
                      <ConsignmentEmpty title={t("consignment-order-not-found")} />
                    </Show>
                  )}
                </Show>
                <Show when={order()}>{(value) => <OrderBody order={value()} />}</Show>
              </>
            );
          }}
        </Match>
      </Switch>
    </>
  );
}

function OrderBody(props: { readonly order: OrderV2 }): JSX.Element {
  const amounts = () => orderAmountRows(props.order);
  return (
    <section class="public-consignment-account__detail">
      <h2>{t(orderStateKey(props.order.order_state))}</h2>
      <Show when={props.order.order_state === "reconcile"}>
        <p role="status">{t("consignment-order-reconcile-note")}</p>
      </Show>
      <dl class="public-consignment-account__facts">
        <div>
          <dt>{t("consignment-order-kind")}</dt>
          <dd>{t(orderKindKey(props.order.kind))}</dd>
        </div>
        <div>
          <dt>{t("consignment-order-role")}</dt>
          <dd>{t(orderRoleKey(props.order.role))}</dd>
        </div>
        <div>
          <dt>{t("consignment-world")}</dt>
          <dd>{consignmentWorldLabel(props.order.world)}</dd>
        </div>
        <Show when={props.order.deadline_at}>
          {(deadline) => (
            <div>
              <dt>{t("consignment-order-deadline")}</dt>
              <dd>
                <Time value={deadline()} />
              </dd>
            </div>
          )}
        </Show>
      </dl>
      <Show when={amounts()}>
        {(rows) => (
          <dl class="public-consignment-account__facts">
            <div>
              <dt>{t("consignment-price")}</dt>
              <dd>{formatCreditAmount(rows().price)}</dd>
            </div>
            <Show when={rows().codGross}>
              {(cod) => (
                <div>
                  <dt>{t("consignment-purchase-cod")}</dt>
                  <dd>{formatCreditAmount(cod())}</dd>
                </div>
              )}
            </Show>
            {/* 手续费可为 0，只以 null 表示省略。 */}
            <Show when={rows().fee !== null}>
              <div>
                <dt>{t("consignment-order-fee")}</dt>
                <dd>{formatCreditAmount(rows().fee ?? 0)}</dd>
              </div>
            </Show>
            <Show when={rows().sellerAmount !== null}>
              <div>
                <dt>{t("consignment-order-seller-amount")}</dt>
                <dd>{formatCreditAmount(rows().sellerAmount ?? 0)}</dd>
              </div>
            </Show>
          </dl>
        )}
      </Show>
      <dl class="public-consignment-account__facts">
        <div>
          <dt>{t("consignment-order-stage-delivery")}</dt>
          <dd>{t(stageKey(props.order.delivery_stage))}</dd>
        </div>
        <Show when={props.order.payment_stage !== "not_applicable"}>
          <div>
            <dt>{t("consignment-order-stage-payment")}</dt>
            <dd>{t(stageKey(props.order.payment_stage))}</dd>
          </div>
        </Show>
        <Show when={props.order.payout_stage !== "not_applicable"}>
          <div>
            <dt>{t("consignment-order-stage-payout")}</dt>
            <dd>{t(stageKey(props.order.payout_stage))}</dd>
          </div>
        </Show>
        <Show when={props.order.refund_stage !== "not_applicable"}>
          <div>
            <dt>{t("consignment-order-stage-refund")}</dt>
            <dd>{t(stageKey(props.order.refund_stage))}</dd>
          </div>
        </Show>
      </dl>
    </section>
  );
}
