// 本人订单列表：按角色、世界与订单态筛选，只展示本人作为买家或卖家的订单。

import type { OrderStateV2, OrderV2 } from "@auction/contracts/dto/public/consignment/order-v2";
import {
  OrderRoleV2Schema,
  OrderStateV2Schema,
} from "@auction/contracts/dto/public/consignment/order-v2";
import { ListingWorldSchema } from "@auction/contracts/dto/public/consignment/states";
import { MAX_PUBLIC_PAGE_SIZE } from "@auction/contracts/dto/public/pagination";
import { type QueryPair, useQueryState } from "@auction/web-shared/net/query";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import { Link } from "@auction/web-shared/router";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { Pager } from "@auction/web-shared/ui/components/pager";
import { Time } from "@auction/web-shared/ui/components/time";
import { formatCreditAmount } from "@auction/web-shared/ui/format";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Match, Show, Switch } from "solid-js";

import {
  fetchOwnedOrders,
  type OwnedOrderQuery,
  validOwnedOrderQuery,
} from "../../api/consignment-orders";
import { DEFAULT_PAGE_SIZE, PUBLIC_PAGE_SIZES } from "../pagination";
import { publicOffsetLastPage } from "../pagination/parameters";
import { consignmentWorldLabel } from "./filters";
import { orderHref, orderKindKey, orderRoleKey, orderStateKey } from "./order-labels";
import { PrivateConsignmentPage, PrivateReadError } from "./private-page";
import { useConsignmentRefresh } from "./refresh";

const KEYS = ["order_role", "order_world", "order_state", "order_page", "order_page_size"];
const DEFAULT_QUERY: OwnedOrderQuery = {
  role: null,
  world: null,
  state: null,
  page: 1,
  pageSize: DEFAULT_PAGE_SIZE,
};

function queryNumber(text: string): number {
  return text !== "" && !/[^0-9]/u.test(text) ? Number(text) : 0;
}

function parse(pairs: readonly QueryPair[]): OwnedOrderQuery | null {
  const relevant = pairs.filter(([key]) => KEYS.includes(key));
  const values = new Map(relevant);
  if (values.size !== relevant.length) return null;
  const role = values.get("order_role");
  const world = values.get("order_world");
  const state = values.get("order_state");
  const query = {
    role: OrderRoleV2Schema.options.find((value) => value === role) ?? null,
    world: ListingWorldSchema.options.find((value) => value === world) ?? null,
    state: OrderStateV2Schema.options.find((value) => value === state) ?? null,
    page: queryNumber(values.get("order_page") ?? "1"),
    pageSize: queryNumber(values.get("order_page_size") ?? String(DEFAULT_PAGE_SIZE)),
  };
  if (
    (role !== undefined && query.role === null) ||
    (world !== undefined && query.world === null) ||
    (state !== undefined && query.state === null) ||
    !validOwnedOrderQuery(query)
  ) {
    return null;
  }
  return query;
}

function serialize(query: OwnedOrderQuery | null): readonly QueryPair[] {
  if (query === null) return [];
  const pairs: QueryPair[] = [];
  if (query.role !== null) pairs.push(["order_role", query.role]);
  if (query.world !== null) pairs.push(["order_world", query.world]);
  if (query.state !== null) pairs.push(["order_state", query.state]);
  if (query.page !== 1) pairs.push(["order_page", String(query.page)]);
  if (query.pageSize !== DEFAULT_PAGE_SIZE) {
    pairs.push(["order_page_size", String(query.pageSize)]);
  }
  return pairs;
}

/** 路由 `/consignment/orders`：认证的本人订单列表；寄售开关不参与历史读取。 */
export function MyOrders(): JSX.Element {
  return (
    <PrivateConsignmentPage title={t("consignment-orders")}>
      {(userId, canUse) => <OwnedOrders userId={userId} canUse={canUse} />}
    </PrivateConsignmentPage>
  );
}

function OwnedOrders(props: {
  readonly userId: string;
  readonly canUse: () => boolean;
}): JSX.Element {
  const location = useQueryState(KEYS, parse, serialize);
  const query = location.state;
  const request = createValueMemo(() => (props.canUse() ? query() : null));
  const read = createTaskResource(request, async (requested, context) =>
    requested === null
      ? null
      : { requested, result: await fetchOwnedOrders(props.userId, requested, context.signal) },
  );
  const current = () => {
    const loaded = read();
    return loaded != null && props.canUse() && loaded.requested === request()
      ? loaded.result
      : null;
  };
  // 订单阶段事件没有公开订阅，页面可见与恢复焦点时按固定间隔重读权威接口。
  useConsignmentRefresh(props.canUse, () => {
    if (props.canUse()) read.restart();
  });
  const pageData = () => {
    const result = current();
    return result?.ok ? result.value : null;
  };
  const readError = () => {
    const result = current();
    return result?.ok === false ? result.error : null;
  };

  return (
    <Switch>
      <Match when={query() === null}>
        <AsyncState
          kind="error"
          title={t("history-filter-settings")}
          detail=""
          retryLabel={t("history-filter-clear")}
          onRetry={() => location.replace(DEFAULT_QUERY)}
        />
      </Match>
      <Match when={query()}>
        {(filter) => (
          <>
            <OrderFilter query={filter()} onChange={location.push} />
            <Switch fallback={<AsyncState kind="loading" title={t("data-loading")} detail="" />}>
              <Match when={readError()}>
                {(error) => <PrivateReadError error={error()} retry={read.restart} />}
              </Match>
              <Match when={pageData()}>
                {(page) => (
                  <>
                    <Show
                      when={page().items.length > 0}
                      fallback={<p>{t("consignment-order-empty")}</p>}
                    >
                      <ul class="public-consignment-account__rows">
                        <For each={page().items} keyed={(item) => item.contract_id}>
                          {(order) => <OrderRow order={order()} />}
                        </For>
                      </ul>
                    </Show>
                    <OrderPager query={filter()} total={page().total} onChange={location.push} />
                  </>
                )}
              </Match>
            </Switch>
          </>
        )}
      </Match>
    </Switch>
  );
}

function OrderFilter(props: {
  readonly query: OwnedOrderQuery;
  readonly onChange: (query: OwnedOrderQuery) => void;
}): JSX.Element {
  return (
    <div class="public-consignment-account__filters">
      <label>
        {t("consignment-order-role")}
        <select
          name="role"
          value={props.query.role ?? ""}
          onChange={(event) => {
            const role =
              OrderRoleV2Schema.options.find((value) => value === event.currentTarget.value) ??
              null;
            props.onChange({ ...props.query, role, page: 1 });
          }}
        >
          <option value="">{t("filter-all")}</option>
          <For each={OrderRoleV2Schema.options}>
            {(role) => <option value={role}>{t(orderRoleKey(role))}</option>}
          </For>
        </select>
      </label>
      <label>
        {t("consignment-world")}
        <select
          name="world"
          value={props.query.world ?? ""}
          onChange={(event) => {
            const world =
              ListingWorldSchema.options.find((value) => value === event.currentTarget.value) ??
              null;
            props.onChange({ ...props.query, world, page: 1 });
          }}
        >
          <option value="">{t("filter-all")}</option>
          <For each={ListingWorldSchema.options}>
            {(world) => <option value={world}>{consignmentWorldLabel(world)}</option>}
          </For>
        </select>
      </label>
      <label>
        {t("consignment-order-state-filter")}
        <select
          name="state"
          value={props.query.state ?? ""}
          onChange={(event) => {
            const state: OrderStateV2 | null =
              OrderStateV2Schema.options.find((value) => value === event.currentTarget.value) ??
              null;
            props.onChange({ ...props.query, state, page: 1 });
          }}
        >
          <option value="">{t("filter-all")}</option>
          <For each={OrderStateV2Schema.options}>
            {(state) => <option value={state}>{t(orderStateKey(state))}</option>}
          </For>
        </select>
      </label>
    </div>
  );
}

function OrderRow(props: { readonly order: OrderV2 }): JSX.Element {
  return (
    <li class="public-consignment-account__row">
      <Link href={orderHref(props.order.contract_id)}>{t(orderKindKey(props.order.kind))}</Link>
      <span>{t(orderRoleKey(props.order.role))}</span>
      <span>{consignmentWorldLabel(props.order.world)}</span>
      <strong>{t(orderStateKey(props.order.order_state))}</strong>
      <Show when={props.order.price}>{(price) => <span>{formatCreditAmount(price())}</span>}</Show>
      <Show when={props.order.deadline_at}>{(deadline) => <Time value={deadline()} />}</Show>
    </li>
  );
}

function OrderPager(props: {
  readonly query: OwnedOrderQuery;
  readonly total: number;
  readonly onChange: (query: OwnedOrderQuery) => void;
}): JSX.Element {
  // 与挂单列表同样复用底层控件，不写拍卖 URL 与页长偏好。
  return (
    <Pager
      pageLevel
      page={props.query.page}
      pageSize={props.query.pageSize}
      total={props.total}
      maxPage={publicOffsetLastPage(props.query.pageSize)}
      label={t("consignment-orders")}
      previousLabel={t("catalog-previous")}
      status={(value) => t("catalog-page", { page: value[0], pages: value[1] })}
      nextLabel={t("catalog-next")}
      jumpLabel={t("pagination-jump")}
      jumpButtonLabel={t("catalog-page-go")}
      rangeSummary={(value) =>
        t("pagination-range", {
          start: String(value[0]),
          end: String(value[1]),
          total: String(value[2]),
        })
      }
      pageSizeOptions={PUBLIC_PAGE_SIZES}
      pageSizeLabel={t("pagination-size")}
      pageSizeOptionLabel={(count) => t("pagination-size-option", { count: String(count) })}
      maxPageSize={MAX_PUBLIC_PAGE_SIZE}
      allowCustomPageSize
      pageSizeCustomLabel={t("pagination-custom")}
      pageSizeApplyLabel={t("pagination-apply")}
      pageSizeInvalidLabel={t("pagination-size-limit", { maximum: String(MAX_PUBLIC_PAGE_SIZE) })}
      onPageChange={(page) => props.onChange({ ...props.query, page })}
      onPageClamp={() => true}
      onPageSizeChange={(pageSize) => props.onChange({ ...props.query, page: 1, pageSize })}
    />
  );
}
