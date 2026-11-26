// 寄售公开目录、摘要卡片与独立分页。

import type { ListingSummaryV2 } from "@auction/contracts/dto/public/consignment/listing-v2";
import { MAX_PUBLIC_PAGE_SIZE } from "@auction/contracts/dto/public/pagination";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import { Link } from "@auction/web-shared/router";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { Pager } from "@auction/web-shared/ui/components/pager";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Match, Show, Switch } from "solid-js";

import { fetchConsignmentCatalogV2 } from "../../api/consignment-orders";
import { PUBLIC_PAGE_SIZES } from "../pagination";
import { publicOffsetLastPage } from "../pagination/parameters";
import { normalizedWorldQuery, usePublicQueryState } from "../query";
import { consignmentEnabled, useConsignmentCapability } from "./capability";
import { DEFAULT_CONSIGNMENT_QUERY, Filters, useConsignmentCatalogQuery } from "./filters";
import { purchaseHref } from "./order-labels";
import {
  ConsignmentCapabilityGate,
  ConsignmentEmpty,
  ConsignmentReadNotice,
  consignmentLevelLabel,
  ListingFacts,
  ModeNotice,
  publicListingStateLabel,
} from "./read-view";
import { useConsignmentRefresh } from "./refresh";

function ListingCard(props: { readonly listing: ListingSummaryV2 }): JSX.Element {
  return (
    <article class="public-consignment-listing">
      <header class="public-consignment-listing__header">
        <h2>
          <Link href={`/consignment/listings/${props.listing.listing_id}`}>
            {props.listing.name}
          </Link>
        </h2>
        <span>{publicListingStateLabel(props.listing.state)}</span>
      </header>
      <p class="public-consignment-listing__equipment">
        {props.listing.category} · {t("equipment-level")}{" "}
        {consignmentLevelLabel(props.listing.level)}
      </p>
      <ListingFacts listing={props.listing} />
      <ModeNotice listing={props.listing} />
      <Show when={props.listing.purchasable}>
        <Link href={purchaseHref(props.listing.listing_id)}>
          {t("consignment-purchase-action")}
        </Link>
      </Show>
    </article>
  );
}

/** 寄售公开目录；资源结果只供原请求的筛选快照使用。 */
export function ConsignmentCatalog(): JSX.Element {
  const capability = useConsignmentCapability();
  const enabled = createValueMemo(() => consignmentEnabled(capability()));
  const location = useConsignmentCatalogQuery();
  // 世界归外壳的 world 键管理：缺省与非法值都是全部世界，与拍卖列表一致。
  const shellQuery = usePublicQueryState();
  const world = createValueMemo(() => normalizedWorldQuery(shellQuery.state().world));
  const query = createValueMemo(() => {
    const selected = location.state();
    return typeof selected === "string" ? null : selected;
  });
  const rejected = () => {
    const selected = location.state();
    return typeof selected === "string" ? selected : null;
  };
  const request = createValueMemo(() => {
    const selected = query();
    return enabled() && selected !== null ? { query: { ...selected, world: world() } } : null;
  });
  const listings = createTaskResource(request, async (requested, context) =>
    requested === null
      ? null
      : { requested, result: await fetchConsignmentCatalogV2(requested.query, context.signal) },
  );
  const refresh = () => {
    capability.refresh();
    if (enabled() && query() !== null) listings.restart();
  };
  useConsignmentRefresh(enabled, refresh);

  const clearFilters = () => location.replace(DEFAULT_CONSIGNMENT_QUERY);
  const currentResult = () => {
    const loaded = listings();
    return loaded?.requested === request() ? loaded.result : null;
  };

  return (
    <section class="public-consignment-catalog" aria-label={t("consignment-catalog")}>
      <ConsignmentCapabilityGate capability={capability()} onRetry={() => capability.refresh()}>
        <Filters
          query={query()}
          rejected={rejected()}
          onApply={(next) => location.push(next)}
          onReject={(field) => location.state.set(field)}
        />
        <Switch fallback={<AsyncState kind="loading" title={t("data-loading")} detail="" />}>
          <Match when={query() === null}>
            <ConsignmentReadNotice error={{ kind: "invalid-query" }} onRetry={clearFilters} />
          </Match>
          <Match when={currentResult()}>
            {(loaded) => {
              const page = () => {
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
                      <ConsignmentReadNotice
                        error={error()}
                        onRetry={() => {
                          if (error().kind === "invalid-query") {
                            clearFilters();
                            listings.restart();
                          } else refresh();
                        }}
                      />
                    )}
                  </Show>
                  <Show when={page()}>
                    {(value) => (
                      <>
                        <Show
                          when={value().items.length > 0}
                          fallback={<ConsignmentEmpty title={t("consignment-empty")} />}
                        >
                          <div class="public-consignment-catalog__grid">
                            <For each={value().items} keyed={(item) => item.listing_id}>
                              {(item) => <ListingCard listing={item()} />}
                            </For>
                          </div>
                        </Show>
                        <CatalogPager
                          page={value().page}
                          pageSize={value().page_size}
                          total={value().total}
                          onPage={(page) => {
                            const current = query();
                            if (current !== null) location.push({ ...current, page });
                          }}
                          onSize={(pageSize) => {
                            const current = query();
                            if (current !== null) location.push({ ...current, page: 1, pageSize });
                          }}
                        />
                      </>
                    )}
                  </Show>
                </>
              );
            }}
          </Match>
        </Switch>
      </ConsignmentCapabilityGate>
    </section>
  );
}

function CatalogPager(props: {
  readonly page: number;
  readonly pageSize: number;
  readonly total: number;
  readonly onPage: (page: number) => void;
  readonly onSize: (size: number) => void;
}): JSX.Element {
  // PublicPager 的页长回调写入拍卖 URL 和偏好。直接复用同一底层控件，保持寄售查询独立。
  return (
    <Pager
      page={props.page}
      pageSize={props.pageSize}
      total={props.total}
      maxPage={publicOffsetLastPage(props.pageSize)}
      label={t("consignment-catalog")}
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
      allowCustomPageSize={true}
      pageSizeCustomLabel={t("pagination-custom")}
      pageSizeApplyLabel={t("pagination-apply")}
      pageSizeInvalidLabel={t("pagination-size-limit", { maximum: String(MAX_PUBLIC_PAGE_SIZE) })}
      onPageChange={props.onPage}
      // 合法空页保留原 URL 和空态，分页仍可手动跳回有条目的页。
      onPageClamp={() => true}
      onPageSizeChange={props.onSize}
    />
  );
}
