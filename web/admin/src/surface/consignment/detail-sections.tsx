// 挂单详情的安全读模型：有效状态、解析快照、运营说明和拍卖碰撞。

import type { AdminListingDetail } from "@auction/contracts/dto/admin/consignment";
import type { EquipmentLevel, EquipmentView } from "@auction/contracts/dto/public/consignment";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { Time } from "@auction/web-shared/ui/components/time";
import { formatCreditAmountFull } from "@auction/web-shared/ui/format";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Show } from "solid-js";

import { adminWorldLabel } from "../presentation";
import {
  listingLookupReasonKey,
  listingModeKey,
  listingReasonKey,
  listingStateKey,
  listingViewKey,
} from "./labels";
import { ListingProgress, listingEquipment } from "./presentation";

/** 安全详情按服务端的已记录状态与当前有效状态分别展示。 */
export function ListingDetailSections(props: { readonly detail: AdminListingDetail }): JSX.Element {
  const record = () => props.detail.record;
  const listing = () => record().listing;
  const view = () => listing().view;
  const equipment = createValueMemo(() => listingEquipment(view()));
  const progress = createValueMemo(() => {
    const current = view();
    return current.kind === "resolving" ? current.progress : null;
  });
  const lookupFailure = createValueMemo(() => {
    const current = view();
    return current.kind === "lookup_failed" ? current.last_lookup_reason : null;
  });

  return (
    <>
      <div class="admin-panel admin-consignment-detail__record">
        <dl class="admin-delivery-detail__grid">
          <dt>{t("consignment-seller")}</dt>
          <dd>{record().seller_name}</dd>
          <dt>{t("consignment-seller-id")}</dt>
          <dd>{record().seller_user_id}</dd>
          <dt>{t("consignment-world")}</dt>
          <dd>{adminWorldLabel(listing().world_id)}</dd>
          <dt>{t("consignment-eid")}</dt>
          <dd>{listing().external_equipment_id}</dd>
          <dt>{t("consignment-mode")}</dt>
          <dd>{t(listingModeKey(listing().mode))}</dd>
          <dt>{t("consignment-price")}</dt>
          <dd>{formatCreditAmountFull(listing().price)}</dd>
          <dt>{t("consignment-stored-state")}</dt>
          <dd>{t(listingStateKey(listing().stored_state))}</dd>
          <dt>{t("consignment-effective-state")}</dt>
          <dd>
            {t(listingStateKey(listing().effective_state))}
            <Show when={listing().effective_state === "rejected"}>
              <div>{t(listingViewKey(view().kind))}</div>
            </Show>
            <Show when={listing().effective_reason}>
              {(reason) => <div>{t(listingReasonKey(reason()))}</div>}
            </Show>
            <Show when={lookupFailure()}>
              {(reason) => <div>{t(listingLookupReasonKey(reason()))}</div>}
            </Show>
            <Show when={progress()}>{(ready) => <ListingProgress progress={ready()} />}</Show>
          </dd>
          <dt>{t("delivery-detail-updated")}</dt>
          <dd>
            <Time value={listing().updated_at} />
          </dd>
          <dt>{t("consignment-expires")}</dt>
          <dd>
            <Time value={listing().expires_at} />
          </dd>
          <Show when={record().admin_delist_reason}>
            {(reason) => (
              <>
                <dt>{t("consignment-delist-reason")}</dt>
                <dd>{reason()}</dd>
              </>
            )}
          </Show>
        </dl>
      </div>
      <Show when={equipment()}>{(snapshot) => <Equipment equipment={snapshot()} />}</Show>
      <section
        class="admin-panel admin-consignment-collisions"
        aria-labelledby="consignment-collisions-title"
      >
        <h2 id="consignment-collisions-title">{t("consignment-collisions")}</h2>
        <Show when={props.detail.conflicts_truncated}>
          <p class="admin-panel__note">{t("consignment-truncated")}</p>
        </Show>
        <Show when={props.detail.conflicts.length > 0} fallback={<p>{t("value-empty")}</p>}>
          <ul class="admin-consignment-collisions__list">
            <For each={props.detail.conflicts} keyed={false}>
              {(conflict) => (
                <li class="admin-consignment-collisions__item">
                  <code>{conflict().lot_id}</code>
                  <Time value={conflict().detected_at} />
                </li>
              )}
            </For>
          </ul>
        </Show>
      </section>
    </>
  );
}

function levelText(level: EquipmentLevel): string {
  if (level.kind === "assigned") return String(level.value);
  // Admin 暂无未分配与魂绑等级键；属性原文及上游未确认值不作为等级标签。
  return level.kind === "soulbound" ? t("consignment-not-tradeable") : t("value-empty");
}

function Equipment(props: { readonly equipment: EquipmentView }): JSX.Element {
  return (
    <section
      class="admin-panel admin-consignment-equipment"
      aria-labelledby="consignment-equipment-title"
    >
      <h2 id="consignment-equipment-title">{props.equipment.name}</h2>
      <Show when={props.equipment.alt_name}>{(name) => <p>{name()}</p>}</Show>
      <p>
        {props.equipment.category} · {levelText(props.equipment.level)}
      </p>
      <Show when={!props.equipment.tradeable && props.equipment.level.kind !== "soulbound"}>
        <p>{t("consignment-not-tradeable")}</p>
      </Show>
      <p>
        <Time value={props.equipment.observed_at} />
      </p>
      <For each={props.equipment.stat_groups} keyed={false}>
        {(group) => (
          <section class="admin-consignment-equipment__group">
            <h3>{group().name}</h3>
            <dl class="admin-delivery-detail__grid">
              <For each={group().stats} keyed={false}>
                {(stat) => (
                  <>
                    <dt>{stat().name}</dt>
                    <dd>
                      {stat().value}
                      <Show when={stat().base !== null}> ({stat().base})</Show>
                    </dd>
                  </>
                )}
              </For>
            </dl>
          </section>
        )}
      </For>
    </section>
  );
}
