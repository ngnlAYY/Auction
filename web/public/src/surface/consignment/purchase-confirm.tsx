// 购买确认页：核对标价与 COD 口径，提交冻结的购买命令；结果未知时只允许原样重放。

import type { ListingDetailV2 } from "@auction/contracts/dto/public/consignment/listing-v2";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import { Link, type RoutePageProps, useRouter } from "@auction/web-shared/router";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { formatCreditAmount } from "@auction/web-shared/ui/format";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { Match, Show, Switch } from "solid-js";

import { commandErrorOutcomeUncertain } from "../../api/commands";
import { fetchConsignmentDetailV2 } from "../../api/consignment-orders";
import type { ConsignmentPrivateError } from "../../api/consignment-private";
import { consignmentEnabled, useConsignmentCapability } from "./capability";
import { EquipmentPanel } from "./detail";
import { consignmentWorldLabel } from "./filters";
import { purchaseQuote } from "./order-amounts";
import { orderHref, purchaseErrorKey, purchaseHref } from "./order-labels";
import { consignmentErrorMessage, PrivateConsignmentPage } from "./private-page";
import { type PurchasePending, usePurchasePending } from "./purchase-pending";
import { ConsignmentCapabilityGate, ConsignmentReadNotice, SellerName } from "./read-view";
import { useConsignmentRefresh } from "./refresh";

/** 路由 `/consignment/listings/:listing_id/purchase`：认证买家的购买确认页。 */
export function PurchaseConfirm(props: RoutePageProps): JSX.Element {
  return (
    <PrivateConsignmentPage title={t("consignment-purchase-title")}>
      {(_userId, canUse) => (
        <PurchasePanel listingId={props.params.listing_id ?? ""} canUse={canUse} />
      )}
    </PrivateConsignmentPage>
  );
}

function purchaseErrorMessage(error: ConsignmentPrivateError): string {
  // 不变量 1：结果未知不得读成失败，页面只提示原样重放。
  if (commandErrorOutcomeUncertain(error)) return t("consignment-purchase-unknown");
  const key = purchaseErrorKey(error.code);
  return key === null ? consignmentErrorMessage(error) : t(key);
}

function PurchasePanel(props: {
  readonly listingId: string;
  readonly canUse: () => boolean;
}): JSX.Element {
  const router = useRouter();
  const capability = useConsignmentCapability();
  const enabled = createValueMemo(() => consignmentEnabled(capability()));
  const request = createValueMemo(() => (enabled() ? { listingId: props.listingId } : null));
  const read = createTaskResource(request, async (requested, context) =>
    requested === null
      ? null
      : { requested, result: await fetchConsignmentDetailV2(requested.listingId, context.signal) },
  );
  const pending = usePurchasePending(props.canUse, (receipt) =>
    router.push(orderHref(receipt.contract_id)),
  );
  const refresh = () => {
    capability.refresh();
    if (enabled() && !pending.busy()) read.restart();
  };
  useConsignmentRefresh(enabled, refresh);
  const current = () => {
    const loaded = read();
    return loaded?.requested === request() ? loaded.result : null;
  };

  return (
    <>
      <PurchaseRecovery pending={pending} canUse={props.canUse()} listingId={props.listingId} />
      <ConsignmentCapabilityGate capability={capability()} onRetry={() => capability.refresh()}>
        <Switch fallback={<AsyncState kind="loading" title={t("data-loading")} detail="" />}>
          <Match when={current()}>
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
                      <PurchaseSummary
                        detail={value()}
                        disabled={
                          !value().summary.purchasable || !props.canUse() || pending.blocked()
                        }
                        onConfirm={() => {
                          const { listing_id, version, price } = value().summary;
                          pending.submit({
                            listing_id,
                            expected_version: version,
                            expected_price: price,
                          });
                        }}
                      />
                    )}
                  </Show>
                </>
              );
            }}
          </Match>
        </Switch>
      </ConsignmentCapabilityGate>
    </>
  );
}

/** 未决购买的恢复入口：只读取冻结记录原样重放，从不分配新命令 ID。 */
function PurchaseRecovery(props: {
  readonly pending: PurchasePending;
  readonly canUse: boolean;
  readonly listingId: string;
}): JSX.Element {
  return (
    <>
      <Show when={props.pending.record()}>
        {(record) => (
          <section
            class="public-consignment-account__pending"
            aria-label={t("consignment-purchase-unknown")}
          >
            <p role="status">
              {props.pending.busy() ? t("data-loading") : t("consignment-purchase-unknown")}
            </p>
            <button
              type="button"
              disabled={props.pending.busy() || !props.canUse}
              onClick={() => props.pending.retry()}
            >
              {t("consignment-purchase-retry")}
            </button>
            <Show when={record().listing_id !== props.listingId}>
              <Link href={purchaseHref(record().listing_id)}>{t("consignment-detail")}</Link>
            </Show>
          </section>
        )}
      </Show>
      <Show when={props.pending.error()}>
        {(error) => <p role="alert">{purchaseErrorMessage(error())}</p>}
      </Show>
    </>
  );
}

function PurchaseSummary(props: {
  readonly detail: ListingDetailV2;
  readonly disabled: boolean;
  readonly onConfirm: () => void;
}): JSX.Element {
  const summary = () => props.detail.summary;
  const quote = () => purchaseQuote(summary().world_id, summary().price);
  return (
    <article class="public-consignment-listing">
      <EquipmentPanel equipment={props.detail.equipment} />
      <dl class="public-consignment-account__facts">
        <div>
          <dt>{t("consignment-seller")}</dt>
          <SellerName listing={summary()} />
        </div>
        <div>
          <dt>{t("consignment-world")}</dt>
          <dd>{consignmentWorldLabel(summary().world_id)}</dd>
        </div>
        <div>
          <dt>{t("consignment-price")}</dt>
          <dd>{formatCreditAmount(quote().price)}</dd>
        </div>
        <Show when={quote().carrierCredit > 0}>
          <div>
            <dt>{t("consignment-purchase-carrier")}</dt>
            <dd>{formatCreditAmount(quote().carrierCredit)}</dd>
          </div>
        </Show>
        <div>
          <dt>{t("consignment-purchase-cod")}</dt>
          <dd>{formatCreditAmount(quote().codGross)}</dd>
        </div>
      </dl>
      <p class="public-consignment-listing__notice">
        {summary().world_id === "isekai"
          ? t("consignment-purchase-explain-isekai")
          : t("consignment-purchase-explain-main")}
      </p>
      <Show when={!summary().purchasable}>
        <p role="status">{t("consignment-purchase-unavailable")}</p>
      </Show>
      <button type="button" disabled={props.disabled} onClick={() => props.onConfirm()}>
        {t("consignment-purchase-action")}
      </button>
    </article>
  );
}
