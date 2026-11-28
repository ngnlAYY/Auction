// 本人挂单列表，按有效态查询并呈现各类挂单的摘要。

import {
  ListingStateV2Schema,
  type OwnedListingV2,
} from "@auction/contracts/dto/public/consignment/listing-v2";
import {
  ListingModeSchema,
  ListingWorldSchema,
} from "@auction/contracts/dto/public/consignment/states";
import { MAX_PUBLIC_PAGE_SIZE } from "@auction/contracts/dto/public/pagination";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import { Link, useRouter } from "@auction/web-shared/router";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { Pager } from "@auction/web-shared/ui/components/pager";
import { Time } from "@auction/web-shared/ui/components/time";
import { formatCreditAmount } from "@auction/web-shared/ui/format";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Match, Show, Switch } from "solid-js";

import {
  fetchOwnedConsignmentListingsV2,
  type OwnedConsignmentQueryV2,
} from "../../api/consignment-owned-v2";
import type { ConsignmentPrivateError } from "../../api/consignment-private";
import { PUBLIC_PAGE_SIZES } from "../pagination";
import { publicOffsetLastPage } from "../pagination/parameters";
import { consignmentWorldLabel } from "./filters";
import {
  listingModeKey,
  listingReasonKey,
  listingStateKey,
  listingViewKey,
  relistConsignmentHref,
} from "./labels";
import { useOwnedConsignmentQuery } from "./owned-query";
import { useConsignmentPending } from "./pending";
import { PendingCommand } from "./pending-view";
import {
  consignmentErrorMessage,
  ownedConsignmentHref,
  ownedEquipment,
  PrivateConsignmentPage,
  PrivateReadError,
} from "./private-page";
import { useConsignmentRefresh } from "./refresh";

/** 认证的本人列表；服务开关不参与历史读取。 */
export function MyListings(): JSX.Element {
  return (
    <PrivateConsignmentPage title={t("consignment-mine")}>
      {(userId, canUse) => <OwnedListings userId={userId} canUse={canUse} />}
    </PrivateConsignmentPage>
  );
}

function OwnedListings(props: {
  readonly userId: string;
  readonly canUse: () => boolean;
}): JSX.Element {
  const location = useOwnedConsignmentQuery();
  const router = useRouter();
  const query = location.state;
  const request = createValueMemo(() => (props.canUse() ? query() : null));
  const read = createTaskResource(request, async (requested, context) =>
    requested === null
      ? null
      : {
          requested,
          result: await fetchOwnedConsignmentListingsV2(props.userId, requested, context.signal),
        },
  );
  const recoveryError = createCell<ConsignmentPrivateError | null>(null);
  const pending = useConsignmentPending(props.canUse, (result, record) => {
    if (record.operation !== "register") return;
    if (
      result.ok &&
      result.value.disposition === "registered" &&
      result.value.listing_id !== null
    ) {
      router.push(ownedConsignmentHref(result.value.listing_id));
    } else {
      if (!result.ok) recoveryError.set(result.error);
      read.restart();
    }
  });
  const current = () => {
    const loaded = read();
    return loaded != null && props.canUse() && loaded.requested === request()
      ? loaded.result
      : null;
  };
  useConsignmentRefresh(
    () => false,
    () => {
      if (props.canUse()) read.restart();
    },
  );

  const pageData = () => {
    const result = current();
    return result?.ok ? result.value : null;
  };
  const readError = () => {
    const result = current();
    return result?.ok === false ? result.error : null;
  };

  return (
    <>
      <PendingCommand pending={pending} canUse={props.canUse()} />
      <Show when={recoveryError()}>
        {(error) => <p role="alert">{consignmentErrorMessage(error())}</p>}
      </Show>
      <Switch>
        <Match when={query() === null}>
          <AsyncState
            kind="error"
            title={t("history-filter-settings")}
            detail=""
            retryLabel={t("history-filter-clear")}
            onRetry={location.clear}
          />
        </Match>
        <Match when={query()}>
          {(filter) => (
            <>
              <StateFilter query={filter()} onChange={location.push} />
              <Switch fallback={<AsyncState kind="loading" title={t("data-loading")} detail="" />}>
                <Match when={readError()}>
                  {(error) => <PrivateReadError error={error()} retry={read.restart} />}
                </Match>
                <Match when={pageData()}>
                  {(page) => (
                    <>
                      <Show
                        when={page().items.length > 0}
                        fallback={<p>{t("consignment-empty")}</p>}
                      >
                        <ul class="public-consignment-account__rows">
                          <For each={page().items} keyed={(item) => item.listing_id}>
                            {(listing) => <OwnedRow listing={listing()} />}
                          </For>
                        </ul>
                      </Show>
                      <OwnedPager
                        query={filter()}
                        total={page().total}
                        onChange={location.push}
                        onClamp={location.replace}
                      />
                    </>
                  )}
                </Match>
              </Switch>
            </>
          )}
        </Match>
      </Switch>
    </>
  );
}

function StateFilter(props: {
  readonly query: OwnedConsignmentQueryV2;
  readonly onChange: (query: OwnedConsignmentQueryV2) => void;
}): JSX.Element {
  return (
    <div class="public-consignment-account__filters">
      <label>
        {t("consignment-world")}
        <select
          name="world"
          value={props.query.world ?? ""}
          onChange={(event) => {
            const world =
              ListingWorldSchema.options.find((world) => world === event.currentTarget.value) ??
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
        {t("consignment-mode")}
        <select
          name="mode"
          value={props.query.mode ?? ""}
          onChange={(event) => {
            const mode =
              ListingModeSchema.options.find((mode) => mode === event.currentTarget.value) ?? null;
            props.onChange({ ...props.query, mode, page: 1 });
          }}
        >
          <option value="">{t("filter-all")}</option>
          <For each={ListingModeSchema.options}>
            {(mode) => <option value={mode}>{t(listingModeKey(mode))}</option>}
          </For>
        </select>
      </label>
      <label>
        {t("consignment-state-filter")}
        <select
          name="state"
          value={props.query.state ?? ""}
          onChange={(event) => {
            const state =
              ListingStateV2Schema.options.find((state) => state === event.currentTarget.value) ??
              null;
            props.onChange({ ...props.query, state, page: 1 });
          }}
        >
          <option value="">{t("filter-all")}</option>
          <For each={ListingStateV2Schema.options}>
            {(state) => {
              const key = listingStateKey(state);
              // 聚合拒绝态包含两个服务端视图，合同未定义独立的 Public 文案。
              return (
                <option value={state}>
                  {key === null
                    ? `${t("consignment-lookup-failed")} / ${t("consignment-ineligible")}`
                    : t(key)}
                </option>
              );
            }}
          </For>
        </select>
      </label>
    </div>
  );
}

function OwnedRow(props: { readonly listing: OwnedListingV2 }): JSX.Element {
  const view = () => props.listing.view;
  const reason = () => {
    const current = view();
    return "reason" in current ? current.reason : null;
  };
  return (
    <li class="public-consignment-account__row">
      <Link href={ownedConsignmentHref(props.listing.listing_id)}>
        {ownedEquipment(view())?.name ?? `#${props.listing.external_equipment_id}`}
      </Link>
      <span>{consignmentWorldLabel(props.listing.world_id)}</span>
      <span>{t(listingModeKey(props.listing.mode))}</span>
      <strong>{t(listingViewKey(view().kind))}</strong>
      <span>{formatCreditAmount(props.listing.price)}</span>
      <Time value={props.listing.updated_at} />
      <Show when={reason()}>{(reason) => <span>{t(listingReasonKey(reason()))}</span>}</Show>
      <Show when={props.listing.allowed_actions.includes("relist")}>
        <Link href={relistConsignmentHref(props.listing.listing_id)}>
          {t("consignment-relist")}
        </Link>
      </Show>
    </li>
  );
}

function OwnedPager(props: {
  readonly query: OwnedConsignmentQueryV2;
  readonly total: number;
  readonly onChange: (query: OwnedConsignmentQueryV2) => void;
  readonly onClamp: (query: OwnedConsignmentQueryV2) => void;
}): JSX.Element {
  // 与 PublicPager 及 3C 复用底层控件；PublicPager 自身会写拍卖 URL 与偏好。
  return (
    <Pager
      pageLevel
      page={props.query.page}
      pageSize={props.query.pageSize}
      total={props.total}
      maxPage={publicOffsetLastPage(props.query.pageSize)}
      label={t("consignment-mine")}
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
      onPageClamp={(page) => props.onClamp({ ...props.query, page })}
      onPageSizeChange={(pageSize) => props.onChange({ ...props.query, page: 1, pageSize })}
    />
  );
}
