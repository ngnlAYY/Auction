// 认证运营订单列表：按世界与订单态筛选，进入订单详情与证据。

import type { AdminOrderV2 } from "@auction/contracts/dto/admin/consignment/order-v2";
import { OrderStateV2Schema } from "@auction/contracts/dto/public/consignment/order-v2";
import { ListingWorldSchema } from "@auction/contracts/dto/public/consignment/states";
import { MAX_PUBLIC_PAGE_SIZE } from "@auction/contracts/dto/public/pagination";
import { type QueryPair, type QueryState, useQueryState } from "@auction/web-shared/net/query";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { err } from "@auction/web-shared/result";
import { Link } from "@auction/web-shared/router";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { Time } from "@auction/web-shared/ui/components/time";
import { formatCreditAmountFull } from "@auction/web-shared/ui/format";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Match, Show, Switch } from "solid-js";

import { LISTING_MAX_OFFSET } from "../../api/consignment/list";
import {
  type AdminOrderPageV2,
  fetchAdminOrders,
  type OrderFilters,
} from "../../api/consignment/orders";
import { useAdminSessionHost } from "../auth/shell";
import { AdminPager, useAdminPage, useAdminPagination } from "../pagination";
import { advanceCommandReload, createCommandReload } from "../pending/sources";
import { adminWorldLabel } from "../presentation";
import { pairValue, parsePage, pushPage, pushText } from "../query/primitives";
import { orderKindKey, orderStateKey } from "./order-labels";
import { consignmentErrorMessage } from "./presentation";

const FILTER_KEYS = ["order_world", "order_state", "order_page"] as const;

interface OrderQuery extends OrderFilters {
  readonly page: bigint;
  readonly duplicate: boolean;
}

function useOrderQuery(): QueryState<OrderQuery> {
  return useQueryState(
    FILTER_KEYS,
    (pairs) => ({
      world: pairValue(pairs, "order_world") ?? "",
      state: pairValue(pairs, "order_state") ?? "",
      page: parsePage(pairs, "order_page"),
      duplicate: FILTER_KEYS.some((key) => pairs.filter(([name]) => name === key).length > 1),
    }),
    (query) => {
      const pairs: QueryPair[] = [];
      pushText(pairs, "order_world", query.world);
      pushText(pairs, "order_state", query.state);
      pushPage(pairs, "order_page", query.page);
      return pairs;
    },
    { sameAddress: "write" },
  );
}

/** 运营订单列表的懒加载入口，路由 `/consignment/orders`。 */
export function page(): JSX.Element {
  const host = useAdminSessionHost();
  const query = useOrderQuery();
  const pagination = useAdminPagination(
    "consignment-orders",
    ["page_size"],
    MAX_PUBLIC_PAGE_SIZE,
    LISTING_MAX_OFFSET,
  );
  const refresh = createCommandReload();
  const orders = useAdminPage(
    pagination,
    () => {
      refresh();
      return query.state();
    },
    (request, size, signal) =>
      request.duplicate
        ? Promise.resolve(err("invalid_query"))
        : fetchAdminOrders(host, request, request.page, size, signal),
  );
  const loadedPage = createValueMemo(() => {
    const result = orders();
    return result?.ok ? result.value : null;
  });
  const loadError = createValueMemo(() => {
    const result = orders();
    return result !== null && !result.ok ? result.error : null;
  });
  const applyFilter = (filter: Partial<OrderFilters>): void => {
    query.push({ ...query.state.peek(), ...filter, page: 1n, duplicate: false });
  };

  return (
    <section class="admin-consignment-list" aria-labelledby="consignment-orders-title">
      <h1 id="consignment-orders-title">{t("consignment-orders-title")}</h1>
      <Link href="/consignment">{t("consignment-admin-title")}</Link>
      <div class="admin-consignment-filters">
        <OrderFilter
          label={t("consignment-world")}
          value={query.state().world}
          options={ListingWorldSchema.options}
          optionLabel={adminWorldLabel}
          onChange={(world) => applyFilter({ world })}
        />
        <OrderFilter
          label={t("consignment-order-state")}
          value={query.state().state}
          options={OrderStateV2Schema.options}
          optionLabel={(state) => t(orderStateKey(state))}
          onChange={(state) => applyFilter({ state })}
        />
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
        <Match when={loadedPage()}>
          {(ready) => (
            <>
              <OrderTable orderPage={ready()} />
              <AdminPager
                pagination={pagination}
                page={ready().page}
                pageSize={ready().page_size}
                total={ready().total}
                label={t("consignment-orders-title")}
                onPageChange={(nextPage) =>
                  query.push({ ...query.state.peek(), page: BigInt(nextPage) })
                }
              />
            </>
          )}
        </Match>
      </Switch>
    </section>
  );
}

function OrderFilter<Choice extends string>(props: {
  readonly label: string;
  readonly value: string;
  readonly options: readonly Choice[];
  readonly optionLabel: (choice: Choice) => string;
  readonly onChange: (value: string) => void;
}): JSX.Element {
  return (
    <label>
      {props.label}
      <select value={props.value} onChange={(event) => props.onChange(event.currentTarget.value)}>
        <option value="">{t("admin-audit-all")}</option>
        <For each={props.options} keyed={false}>
          {(choice) => <option value={choice()}>{props.optionLabel(choice())}</option>}
        </For>
      </select>
    </label>
  );
}

function OrderTable(props: { readonly orderPage: AdminOrderPageV2 }): JSX.Element {
  return (
    <Show
      when={props.orderPage.items.length > 0}
      fallback={<AsyncState kind="empty" title={t("admin-auctions-empty")} detail="" />}
    >
      <div class="admin-shell__table-wrap">
        <table
          class="admin-shell__table admin-consignment-list__table"
          aria-label={t("consignment-orders-title")}
        >
          <thead>
            <tr>
              <th scope="col">{t("consignment-order-kind")}</th>
              <th scope="col">{t("consignment-seller")}</th>
              <th scope="col">{t("consignment-buyer")}</th>
              <th scope="col">{t("consignment-world")}</th>
              <th scope="col">{t("consignment-price")}</th>
              <th scope="col">{t("consignment-order-state")}</th>
              <th scope="col">{t("delivery-detail-updated")}</th>
            </tr>
          </thead>
          <tbody>
            <For each={props.orderPage.items} keyed={(row) => row.contract_id}>
              {(row) => <OrderRow order={row()} />}
            </For>
          </tbody>
        </table>
      </div>
    </Show>
  );
}

function OrderRow(props: { readonly order: AdminOrderV2 }): JSX.Element {
  return (
    <tr>
      <td data-label={t("consignment-order-kind")}>
        <Link href={`/consignment/orders/${props.order.contract_id}`}>
          {t(orderKindKey(props.order.kind))}
        </Link>
      </td>
      <td data-label={t("consignment-seller")}>{props.order.seller_name}</td>
      <td data-label={t("consignment-buyer")}>{props.order.buyer_name ?? "—"}</td>
      <td data-label={t("consignment-world")}>{adminWorldLabel(props.order.world)}</td>
      <td data-label={t("consignment-price")}>
        {props.order.price === null ? "—" : formatCreditAmountFull(props.order.price)}
      </td>
      <td data-label={t("consignment-order-state")}>
        <span class="admin-dashboard-badge admin-dashboard-badge--neutral">
          {t(orderStateKey(props.order.order_state))}
        </span>
      </td>
      <td data-label={t("delivery-detail-updated")}>
        <Time value={props.order.updated_at} />
      </td>
    </tr>
  );
}
