// 运营订单详情：买卖双方与完整金额、各阶段，以及分页的订单证据。

import type {
  AdminOrderV2,
  OrderEvidenceItemV2,
} from "@auction/contracts/dto/admin/consignment/order-v2";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import { unreachable } from "@auction/web-shared/result";
import { Link, type RoutePageProps } from "@auction/web-shared/router";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { Time } from "@auction/web-shared/ui/components/time";
import { formatCreditAmountFull } from "@auction/web-shared/ui/format";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Match, Show, Switch, untrack } from "solid-js";

import { fetchAdminOrder, fetchAdminOrderEvidence } from "../../api/consignment/orders";
import { useAdminSessionHost } from "../auth/shell";
import { advanceCommandReload, createCommandReload } from "../pending/sources";
import { adminWorldLabel } from "../presentation";
import { isRecoveryEvidence, RecoveryEvidenceFields } from "./evidence-recovery";
import {
  evidenceKeys,
  evidenceKindKey,
  orderKindKey,
  orderStateKey,
  stageKey,
} from "./order-labels";
import { consignmentErrorMessage } from "./presentation";
import { OrderResolvePanel } from "./resolve";

const EVIDENCE_PAGE_SIZE = 20;

/** 路由页内切换订单时卸载旧私有视图与证据页码。 */
export function detailPage(props: RoutePageProps): JSX.Element {
  return (
    <For each={[props.params.contract_id ?? ""]} keyed={(contractId) => contractId}>
      {(contractId) => <Detail contractId={untrack(contractId)} />}
    </For>
  );
}

function Detail(props: { readonly contractId: string }): JSX.Element {
  const host = useAdminSessionHost();
  const contractId = untrack(() => props.contractId);
  const refresh = createCommandReload();
  const read = createTaskResource(
    () => refresh(),
    async (generation, context) => ({
      generation,
      result: await fetchAdminOrder(host, contractId, context.signal),
    }),
  );
  const loaded = createValueMemo(() => {
    const current = read();
    return current?.generation === refresh() ? current.result : null;
  });
  const order = createValueMemo(() => {
    const result = loaded();
    return result?.ok ? result.value : null;
  });
  const loadError = createValueMemo(() => {
    const result = loaded();
    return result !== null && !result.ok ? result.error : null;
  });

  return (
    <section class="admin-consignment-detail" aria-labelledby="consignment-order-title">
      <div class="admin-consignment-detail__header">
        <h1 id="consignment-order-title" tabindex="-1">
          {t("consignment-order-detail-title")}
        </h1>
        <Link href="/consignment/orders">{t("consignment-orders-title")}</Link>
        <Show when={order()}>
          <button type="button" onClick={() => advanceCommandReload(refresh)}>
            {t("admin-record-refresh")}
          </button>
        </Show>
      </div>
      <Switch
        fallback={<AsyncState kind="loading" title={t("dashboard-legacy-loading")} detail="" />}
      >
        <Match when={loadError()}>
          {(error) => (
            <AsyncState
              kind="error"
              title={consignmentErrorMessage(error())}
              detail=""
              retryLabel={t("admin-retry")}
              onRetry={() => advanceCommandReload(refresh)}
            />
          )}
        </Match>
        <Match when={order()}>
          {(current) => (
            <>
              <OrderFacts order={current()} />
              <OrderResolvePanel
                contractId={contractId}
                version={current().version}
                sale={current().kind === "sale"}
                refresh={refresh}
              />
              <Evidence contractId={contractId} generation={refresh} />
            </>
          )}
        </Match>
      </Switch>
    </section>
  );
}

function OrderFacts(props: { readonly order: AdminOrderV2 }): JSX.Element {
  return (
    <dl class="admin-consignment-detail__facts">
      <div>
        <dt>{t("consignment-order-state")}</dt>
        <dd>{t(orderStateKey(props.order.order_state))}</dd>
      </div>
      <div>
        <dt>{t("consignment-order-kind")}</dt>
        <dd>{t(orderKindKey(props.order.kind))}</dd>
      </div>
      <div>
        <dt>{t("consignment-world")}</dt>
        <dd>{adminWorldLabel(props.order.world)}</dd>
      </div>
      <div>
        <dt>{t("consignment-seller")}</dt>
        <dd>{props.order.seller_name}</dd>
      </div>
      <Show when={props.order.buyer_name}>
        {(name) => (
          <div>
            <dt>{t("consignment-buyer")}</dt>
            <dd>{name()}</dd>
          </div>
        )}
      </Show>
      <Show when={props.order.price}>
        {(price) => (
          <div>
            <dt>{t("consignment-price")}</dt>
            <dd>{formatCreditAmountFull(price())}</dd>
          </div>
        )}
      </Show>
      <Show when={props.order.fee !== null}>
        <div>
          <dt>{t("consignment-order-fee")}</dt>
          <dd>{formatCreditAmountFull(props.order.fee ?? 0)}</dd>
        </div>
      </Show>
      <Show when={props.order.seller_amount}>
        {(amount) => (
          <div>
            <dt>{t("consignment-order-seller-amount")}</dt>
            <dd>{formatCreditAmountFull(amount())}</dd>
          </div>
        )}
      </Show>
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
      <div>
        <dt>{t("consignment-order-stage-delivery")}</dt>
        <dd>{t(stageKey(props.order.delivery_stage))}</dd>
      </div>
      <div>
        <dt>{t("consignment-order-stage-payment")}</dt>
        <dd>{t(stageKey(props.order.payment_stage))}</dd>
      </div>
      <div>
        <dt>{t("consignment-order-stage-payout")}</dt>
        <dd>{t(stageKey(props.order.payout_stage))}</dd>
      </div>
      <div>
        <dt>{t("consignment-order-stage-refund")}</dt>
        <dd>{t(stageKey(props.order.refund_stage))}</dd>
      </div>
      <div>
        <dt>{t("delivery-detail-updated")}</dt>
        <dd>
          <Time value={props.order.updated_at} />
        </dd>
      </div>
    </dl>
  );
}

function Evidence(props: {
  readonly contractId: string;
  readonly generation: () => bigint;
}): JSX.Element {
  const host = useAdminSessionHost();
  const page = createCell(1);
  const read = createTaskResource(
    () => ({ page: page(), generation: props.generation() }),
    async (request, context) => ({
      request,
      result: await fetchAdminOrderEvidence(
        host,
        props.contractId,
        request.page,
        EVIDENCE_PAGE_SIZE,
        context.signal,
      ),
    }),
  );
  const current = createValueMemo(() => {
    const loaded = read();
    return loaded?.request.page === page() && loaded.request.generation === props.generation()
      ? loaded.result
      : null;
  });
  const evidence = createValueMemo(() => {
    const result = current();
    return result?.ok ? result.value : null;
  });
  const failure = createValueMemo(() => {
    const result = current();
    return result !== null && !result.ok ? result.error : null;
  });

  return (
    <section class="admin-consignment-detail__evidence">
      <h2>{t("consignment-order-evidence")}</h2>
      <Switch
        fallback={<AsyncState kind="loading" title={t("dashboard-legacy-loading")} detail="" />}
      >
        <Match when={failure()}>
          {(error) => (
            <AsyncState kind="error" title={consignmentErrorMessage(error())} detail="" />
          )}
        </Match>
        <Match when={evidence()}>
          {(ready) => (
            <>
              <Show
                when={ready().items.length > 0}
                fallback={<AsyncState kind="empty" title={t("admin-auctions-empty")} detail="" />}
              >
                <ul class="admin-consignment-detail__evidence-list">
                  <For each={ready().items} keyed={(entry) => evidenceItemKey(entry)}>
                    {(item) => <EvidenceItem item={item()} />}
                  </For>
                </ul>
              </Show>
              <div class="admin-consignment-detail__evidence-pager">
                <button
                  type="button"
                  disabled={page() <= 1}
                  onClick={() => page.set(page.peek() - 1)}
                >
                  {t("admin-pagination-previous")}
                </button>
                <span>{t("admin-pagination-total", { count: String(ready().total) })}</span>
                <button
                  type="button"
                  disabled={ready().page * ready().page_size >= ready().total}
                  onClick={() => page.set(page.peek() + 1)}
                >
                  {t("admin-pagination-next")}
                </button>
              </div>
            </>
          )}
        </Match>
      </Switch>
    </section>
  );
}

function Field(props: { readonly label: string; readonly children: JSX.Element }): JSX.Element {
  return (
    <span>
      {props.label}: {props.children}
    </span>
  );
}

/** 证据条目只展示 ID、阶段、金额、摘要与时刻；摘要截到前 12 位便于核对。 */
function EvidenceItem(props: { readonly item: OrderEvidenceItemV2 }): JSX.Element {
  const item = () => props.item;
  return (
    <li class="admin-consignment-detail__evidence-item">
      <strong>{t(evidenceKindKey(item().kind))}</strong>
      <EvidenceFields item={item()} />
    </li>
  );
}

/** 证据行的稳定键；同类重复条目没有独立 ID，按种类与判别字段区分。 */
function evidenceItemKey(item: OrderEvidenceItemV2): string {
  switch (item.kind) {
    case "round":
      return `round:${item.round_id}`;
    case "attempt":
      return `attempt:${item.attempt_id}`;
    case "return_take":
      return `return_take:${item.intake_id}:${item.work_id}`;
    case "observation":
      return `observation:${item.observation_id}`;
    case "cash_use":
      return `cash_use:${item.purpose}`;
    case "blocker":
      return `blocker:${item.blocker}`;
    case "closure":
      return `closure:${item.closure_kind}`;
    case "resolution":
      return `resolution:${item.resolution_id}`;
    case "recheck":
      return `recheck:${item.work_id}`;
    default:
      return unreachable(item);
  }
}

function EvidenceFields(props: { readonly item: OrderEvidenceItemV2 }): JSX.Element {
  const item = props.item;
  if (isRecoveryEvidence(item)) return <RecoveryEvidenceFields item={item} />;
  switch (item.kind) {
    case "round":
      return (
        <>
          <Field label={t("consignment-evidence-field-round")}>{item.round_number}</Field>
          <Field label={t("consignment-evidence-field-leg")}>{t(evidenceKeys.leg(item.leg))}</Field>
          <Field label={t("consignment-order-stage-delivery")}>
            {t(stageKey(item.delivery_stage))}
          </Field>
          <Time value={item.created_at} />
        </>
      );
    case "attempt":
      return (
        <>
          <Field label={t("consignment-evidence-field-fence")}>{item.fence}</Field>
          <Field label={t("consignment-evidence-field-write-started")}>
            {item.write_started ? t("consignment-evidence-yes") : t("consignment-evidence-no")}
          </Field>
          <Field label={t("consignment-order-stage-delivery")}>
            {t(stageKey(item.delivery_stage))}
          </Field>
        </>
      );
    case "return_take":
      return (
        <>
          <Field label={t("consignment-evidence-field-take-state")}>
            {t(evidenceKeys.take(item.state))}
          </Field>
          <Show when={item.fence}>
            {(fence) => <Field label={t("consignment-evidence-field-fence")}>{fence()}</Field>}
          </Show>
        </>
      );
    case "observation":
      return (
        <>
          <Field label={t("consignment-evidence-field-category")}>
            {t(evidenceKeys.category(item.category))}
          </Field>
          <Field label={t("consignment-evidence-field-conclusion")}>
            {t(evidenceKeys.conclusion(item.conclusion))}
          </Field>
          <Field label={t("consignment-evidence-field-digest")}>{item.digest.slice(0, 12)}</Field>
          <Time value={item.observed_at} />
        </>
      );
    case "cash_use":
      return (
        <>
          <Field label={t("consignment-evidence-field-purpose")}>
            {t(evidenceKeys.purpose(item.purpose))}
          </Field>
          <Field label={t("consignment-order-stage-payout")}>{t(stageKey(item.state))}</Field>
          <Field label={t("consignment-evidence-field-amount")}>
            {formatCreditAmountFull(item.amount)}
          </Field>
        </>
      );
    case "blocker":
      return (
        <>
          <Field label={t("consignment-evidence-field-blocker")}>
            {t(evidenceKeys.blocker(item.blocker))}
          </Field>
          <Field label={t("consignment-evidence-field-resolved")}>
            {item.resolved ? t("consignment-evidence-yes") : t("consignment-evidence-no")}
          </Field>
          <Time value={item.since} />
        </>
      );
  }
}
