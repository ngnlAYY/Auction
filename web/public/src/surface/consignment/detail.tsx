// 独立的公开挂单详情与安全装备快照。

import type {
  EquipmentStat,
  EquipmentView,
} from "@auction/contracts/dto/public/consignment/catalog";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import { Link, type RoutePageProps } from "@auction/web-shared/router";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Match, Show, Switch } from "solid-js";

import { fetchConsignmentDetailV2 } from "../../api/consignment-orders";
import { statValueLabel } from "../catalog/detail/stat-format";
import { consignmentEnabled, useConsignmentCapability } from "./capability";
import { purchaseHref } from "./order-labels";
import {
  ConsignmentCapabilityGate,
  ConsignmentReadNotice,
  consignmentLevelLabel,
  ListingFacts,
  ModeNotice,
} from "./read-view";
import { useConsignmentRefresh } from "./refresh";

/** 寄售属性只有来源值和可空 base；不合成拍卖 roll。 */
export function consignmentStatLabel(stat: EquipmentStat): string {
  return statValueLabel(String(stat.value), stat.base, null);
}

/** 安全装备投影，供公开详情使用，也可供本人详情复用。 */
export function EquipmentPanel(props: { readonly equipment: EquipmentView }): JSX.Element {
  return (
    <section class="public-consignment-listing__equipment">
      <h2>{props.equipment.name}</h2>
      <Show when={props.equipment.alt_name}>{(name) => <p>{name()}</p>}</Show>
      <dl class="public-consignment-listing__facts">
        <div>
          <dt>{t("consignment-category")}</dt>
          <dd>{props.equipment.category}</dd>
        </div>
        <div>
          <dt>{t("equipment-level")}</dt>
          <dd>{consignmentLevelLabel(props.equipment.level)}</dd>
        </div>
        <div>
          <dt>{t("consignment-source-time")}</dt>
          <dd>
            <time datetime={props.equipment.observed_at}>{props.equipment.observed_at}</time>
          </dd>
        </div>
      </dl>
      <Show when={props.equipment.stat_groups.length > 0}>
        <h3>{t("equipment-stats")}</h3>
        <For each={props.equipment.stat_groups} keyed={false}>
          {(group) => (
            <section class="public-consignment-listing__stat-group">
              <h4>{group().name}</h4>
              <dl class="public-consignment-listing__facts">
                <For each={group().stats} keyed={false}>
                  {(stat) => (
                    <div>
                      <dt>{stat().name}</dt>
                      <dd>{consignmentStatLabel(stat())}</dd>
                    </div>
                  )}
                </For>
              </dl>
            </section>
          )}
        </For>
      </Show>
    </section>
  );
}

/** 公开详情；路由目标改变后旧请求与旧挂单内容立即失去展示资格。 */
export function PublicDetail(props: RoutePageProps): JSX.Element {
  const capability = useConsignmentCapability();
  const enabled = createValueMemo(() => consignmentEnabled(capability()));
  const request = createValueMemo(() =>
    enabled() ? { listingId: props.params.listing_id ?? "" } : null,
  );
  const read = createTaskResource(request, async (requested, context) =>
    requested === null
      ? null
      : { requested, result: await fetchConsignmentDetailV2(requested.listingId, context.signal) },
  );
  const refresh = () => {
    capability.refresh();
    if (enabled()) read.restart();
  };
  useConsignmentRefresh(enabled, refresh);

  const currentResult = () => {
    const loaded = read();
    return loaded?.requested === request() ? loaded.result : null;
  };

  return (
    <section class="public-consignment-detail">
      <header class="public-consignment-detail__header">
        <h1>{t("consignment-detail")}</h1>
        <Link href="/consignment">{t("consignment-catalog")}</Link>
      </header>
      <ConsignmentCapabilityGate capability={capability()} onRetry={() => capability.refresh()}>
        <Switch fallback={<AsyncState kind="loading" title={t("data-loading")} detail="" />}>
          <Match when={currentResult()}>
            {(loaded) => {
              const detail = () => {
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
                    {(error) => <ConsignmentReadNotice error={error()} onRetry={refresh} />}
                  </Show>
                  <Show when={detail()}>
                    {(value) => (
                      <article class="public-consignment-listing">
                        <EquipmentPanel equipment={value().equipment} />
                        <dl class="public-consignment-listing__facts">
                          <div>
                            <dt>{t("consignment-eid")}</dt>
                            <dd>{value().summary.external_equipment_id}</dd>
                          </div>
                          <div>
                            <dt>{t("consignment-expires")}</dt>
                            <dd>
                              <time datetime={value().summary.expires_at}>
                                {value().summary.expires_at}
                              </time>
                            </dd>
                          </div>
                        </dl>
                        <ListingFacts listing={value().summary} />
                        <ModeNotice listing={value().summary} />
                        <Show when={value().summary.purchasable}>
                          <Link href={purchaseHref(value().summary.listing_id)}>
                            {t("consignment-purchase-action")}
                          </Link>
                        </Show>
                      </article>
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
