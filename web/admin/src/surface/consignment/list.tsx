// 认证运营列表：按有效状态等条件筛选并进入私有挂单详情。

import type { AdminListing, AdminListingPage } from "@auction/contracts/dto/admin/consignment";
import { ConsignmentUuidSchema } from "@auction/contracts/dto/public/consignment/scalar";
import {
  ListingModeSchema,
  ListingStateSchema,
  ListingWorldSchema,
} from "@auction/contracts/dto/public/consignment/states";
import { MAX_PUBLIC_PAGE_SIZE } from "@auction/contracts/dto/public/pagination";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { err } from "@auction/web-shared/result";
import { Link } from "@auction/web-shared/router";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { Time } from "@auction/web-shared/ui/components/time";
import { formatCreditAmountFull } from "@auction/web-shared/ui/format";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { createEffect, For, Match, Show, Switch } from "solid-js";
import { is } from "valibot";

import { fetchAdminListings, LISTING_MAX_OFFSET } from "../../api/consignment/list";
import { useAdminSessionHost } from "../auth/shell";
import { AdminPager, useAdminPage, useAdminPagination } from "../pagination";
import { advanceCommandReload, createCommandReload, usePendingCommands } from "../pending/sources";
import { adminWorldLabel } from "../presentation";
import { listingModeKey, listingStateKey, listingViewKey } from "./labels";
import { consignmentErrorMessage, ListingProgress, listingEquipment } from "./presentation";
import { type ListingQuery, useListingQuery } from "./query";

/** 运营挂单列表的懒加载入口。 */
export function page(): JSX.Element {
  const host = useAdminSessionHost();
  const query = useListingQuery();
  const pagination = useAdminPagination(
    "consignment",
    ["page_size"],
    MAX_PUBLIC_PAGE_SIZE,
    LISTING_MAX_OFFSET,
  );
  const refresh = createCommandReload();
  usePendingCommands(refresh, (entity) => entity.startsWith("consignment:"));
  const listings = useAdminPage(
    pagination,
    () => {
      refresh();
      return query.state();
    },
    (request, size, signal) =>
      request.duplicate
        ? Promise.resolve(err("invalid_query"))
        : fetchAdminListings(host, request, request.page, size, signal),
  );
  const loadedPage = createValueMemo(() => {
    const result = listings();
    return result?.ok ? result.value : null;
  });
  const loadError = createValueMemo(() => {
    const result = listings();
    return result !== null && !result.ok ? result.error : null;
  });
  const sellerDraft = createCell(query.state.peek().sellerUserId);
  const invalidSeller = createCell(false);
  createEffect(
    () => query.state().sellerUserId,
    (seller) => {
      sellerDraft.set(seller);
      invalidSeller.set(false);
    },
  );

  const applyFilter = (filter: Partial<ListingQuery>): void => {
    query.push({ ...query.state.peek(), ...filter, page: 1n, duplicate: false });
  };
  const applySeller = (event: SubmitEvent): void => {
    event.preventDefault();
    const seller = sellerDraft.peek();
    if (seller !== "" && !is(ConsignmentUuidSchema, seller)) {
      invalidSeller.set(true);
      return;
    }
    invalidSeller.set(false);
    applyFilter({ sellerUserId: seller });
  };

  return (
    <section class="admin-consignment-list" aria-labelledby="consignment-list-title">
      <h1 id="consignment-list-title">{t("consignment-admin-title")}</h1>
      <div class="admin-consignment-filters">
        <ListingFilter
          label={t("consignment-world")}
          value={query.state().world}
          options={ListingWorldSchema.options}
          optionLabel={adminWorldLabel}
          onChange={(world) => applyFilter({ world })}
        />
        <ListingFilter
          label={t("consignment-mode")}
          value={query.state().mode}
          options={ListingModeSchema.options}
          optionLabel={(mode) => t(listingModeKey(mode))}
          onChange={(mode) => applyFilter({ mode })}
        />
        <ListingFilter
          label={t("consignment-effective-state")}
          value={query.state().state}
          options={ListingStateSchema.options}
          optionLabel={(state) => t(listingStateKey(state))}
          onChange={(state) => applyFilter({ state })}
        />
        <form class="admin-consignment-filters__seller" onSubmit={applySeller}>
          <label for="consignment-seller-id">{t("consignment-seller-id")}</label>
          <input
            id="consignment-seller-id"
            type="text"
            value={sellerDraft()}
            aria-invalid={invalidSeller() ? "true" : "false"}
            aria-describedby={invalidSeller() ? "consignment-seller-error" : undefined}
            onInput={(event) => {
              sellerDraft.set(event.currentTarget.value);
              invalidSeller.set(false);
            }}
          />
          <button type="submit">{t("admin-pagination-apply")}</button>
          <Show when={invalidSeller()}>
            <p
              id="consignment-seller-error"
              class="action-feedback action-feedback--error"
              role="alert"
            >
              {consignmentErrorMessage("invalid_query")}
            </p>
          </Show>
        </form>
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
              <ListingTable listingPage={ready()} />
              <AdminPager
                pagination={pagination}
                page={ready().page}
                pageSize={ready().page_size}
                total={ready().total}
                label={t("consignment-admin-title")}
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

function ListingFilter<Choice extends string>(props: {
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

function ListingTable(props: { readonly listingPage: AdminListingPage }): JSX.Element {
  return (
    <Show
      when={props.listingPage.items.length > 0}
      fallback={<AsyncState kind="empty" title={t("admin-auctions-empty")} detail="" />}
    >
      <div class="admin-shell__table-wrap">
        <table
          class="admin-shell__table admin-consignment-list__table"
          aria-label={t("consignment-admin-title")}
        >
          <thead>
            <tr>
              <th scope="col">{t("consignment-seller")}</th>
              <th scope="col">{t("consignment-eid")}</th>
              <th scope="col">{t("consignment-world")}</th>
              <th scope="col">{t("consignment-mode")}</th>
              <th scope="col">{t("consignment-price")}</th>
              <th scope="col">{t("consignment-effective-state")}</th>
              <th scope="col">{t("delivery-detail-updated")}</th>
            </tr>
          </thead>
          <tbody>
            <For each={props.listingPage.items} keyed={(row) => row.listing.listing_id}>
              {(row) => <ListingRow row={row()} />}
            </For>
          </tbody>
        </table>
      </div>
    </Show>
  );
}

function ListingRow(props: { readonly row: AdminListing }): JSX.Element {
  const listing = () => props.row.listing;
  const equipment = createValueMemo(() => listingEquipment(listing().view));
  const progress = createValueMemo(() => {
    const view = listing().view;
    return view.kind === "resolving" ? view.progress : null;
  });
  return (
    <tr>
      <td data-label={t("consignment-seller")}>{props.row.seller_name}</td>
      <td data-label={t("consignment-eid")}>
        <Link href={`/consignment/listings/${listing().listing_id}`}>
          {listing().external_equipment_id}
        </Link>
        <Show when={equipment()}>{(snapshot) => <div>{snapshot().name}</div>}</Show>
      </td>
      <td data-label={t("consignment-world")}>{adminWorldLabel(listing().world_id)}</td>
      <td data-label={t("consignment-mode")}>{t(listingModeKey(listing().mode))}</td>
      <td data-label={t("consignment-price")}>{formatCreditAmountFull(listing().price)}</td>
      <td data-label={t("consignment-effective-state")}>
        <span class="admin-dashboard-badge admin-dashboard-badge--neutral">
          {t(listingStateKey(listing().effective_state))}
        </span>
        <Show when={listing().effective_state === "rejected"}>
          <div>{t(listingViewKey(listing().view.kind))}</div>
        </Show>
        <Show when={progress()}>{(ready) => <ListingProgress progress={ready()} />}</Show>
      </td>
      <td data-label={t("delivery-detail-updated")}>
        <Time value={listing().updated_at} />
      </td>
    </tr>
  );
}
