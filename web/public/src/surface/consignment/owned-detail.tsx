// 本人详情的解析进度、七种安全投影和卖家动作。

import type { LookupProgress } from "@auction/contracts/dto/public/consignment";
import type {
  ListingActionV2,
  OwnedListingV2,
} from "@auction/contracts/dto/public/consignment/listing-v2";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import { Link, type RoutePageProps, useRouter } from "@auction/web-shared/router";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { Dialog } from "@auction/web-shared/ui/components/dialog";
import { Time } from "@auction/web-shared/ui/components/time";
import { formatCreditAmount } from "@auction/web-shared/ui/format";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { Match, Show, Switch } from "solid-js";

import { fetchOwnedConsignmentListingV2 } from "../../api/consignment-owned-v2";
import { type ConsignmentPrivateError, validConsignmentUuid } from "../../api/consignment-private";
import { EquipmentPanel } from "./detail";
import { consignmentWorldLabel } from "./filters";
import {
  listingLookupReasonKey,
  listingModeKey,
  listingReasonKey,
  listingViewKey,
  lossReceiptHref,
  relistConsignmentHref,
} from "./labels";
import { type ConsignmentIntent, useConsignmentPending } from "./pending";
import { PendingCommand } from "./pending-view";
import {
  consignmentErrorMessage,
  FeeQuote,
  ownedConsignmentHref,
  ownedEquipment,
  PrivateConsignmentPage,
  PrivateReadError,
} from "./private-page";
import { ConsignmentEmpty } from "./read-view";
import { useConsignmentRefresh } from "./refresh";
import { listingPrice } from "./registration-input";

/** 本人详情根据规范挂单 ID 重建读取及确认宿主，路由参数变化立即移除旧投影。 */
export function OwnedDetail(props: RoutePageProps): JSX.Element {
  const listingId = () => {
    const value = props.params.listing_id ?? "";
    return validConsignmentUuid(value) ? value : null;
  };
  return (
    <PrivateConsignmentPage title={t("consignment-detail")}>
      {(userId, canUse) => (
        <Show
          when={listingId()}
          keyed
          fallback={<ConsignmentEmpty title={t("consignment-not-found")} />}
        >
          {(id) => <Detail userId={userId} listingId={id} canUse={canUse} />}
        </Show>
      )}
    </PrivateConsignmentPage>
  );
}

function Detail(props: {
  readonly userId: string;
  readonly listingId: string;
  readonly canUse: () => boolean;
}): JSX.Element {
  const router = useRouter();
  const read = createTaskResource(props.canUse, async (authorized, context) =>
    authorized
      ? fetchOwnedConsignmentListingV2(props.userId, props.listingId, context.signal)
      : null,
  );
  const feedback = createCell<ConsignmentPrivateError | null>(null);
  // 已在库件的撤回由服务端直接建撤回代（202 withdrawal_accepted），挂单重读后只显示撤回处理中，
  // 这里另行说明装备将寄回；任何后续结果都会撤下该提示。
  const accepted = createCell(false);
  const pending = useConsignmentPending(props.canUse, (result, record) => {
    if (record.operation !== "register" && record.listing_id !== props.listingId) return;
    accepted.set(result.ok && result.value.disposition === "withdrawal_accepted");
    if (
      result.ok &&
      result.value.disposition === "registered" &&
      result.value.listing_id !== null
    ) {
      router.push(ownedConsignmentHref(result.value.listing_id));
      return;
    }
    feedback.set(result.ok ? null : result.error);
    // 不变量 6：版本或状态冲突后立即撤下旧动作，等新快照后重新确认。
    if (
      result.ok ||
      result.error.code === "stale_version" ||
      result.error.code === "listing_not_active"
    ) {
      read.clear();
      read.restart();
    }
  });
  const current = () => (props.canUse() ? read() : null);
  const projection = () => {
    const result = current();
    return result?.ok ? result.value : null;
  };
  const relevantPending = () => {
    const record = pending.record();
    return (
      record !== null && (record.operation === "register" || record.listing_id === props.listingId)
    );
  };
  useConsignmentRefresh(
    () => props.canUse() && (projection()?.view.kind === "resolving" || relevantPending()),
    () => {
      if (!props.canUse()) return;
      read.restart();
      if (relevantPending() && !pending.missing()) pending.check();
    },
  );

  return (
    <>
      <Link href="/consignment/me">{t("consignment-mine")}</Link>
      <PendingCommand pending={pending} canUse={props.canUse()} listingId={props.listingId} />
      <Show when={feedback()}>
        {(error) => <p role="alert">{consignmentErrorMessage(error())}</p>}
      </Show>
      <Show when={accepted()}>
        <p role="status">{t("consignment-withdrawal-accepted")}</p>
      </Show>
      <Switch fallback={<AsyncState kind="loading" title={t("data-loading")} detail="" />}>
        <Match when={projection()}>
          {(listing) => (
            <>
              <ListingProjection listing={listing()} />
              <SellerActions
                listing={listing()}
                disabled={!props.canUse() || pending.blocked() || read.state() !== "ready"}
                submit={(input) => {
                  const submitted = pending.submit(input);
                  if (submitted) {
                    feedback.set(null);
                    accepted.set(false);
                  }
                  return submitted;
                }}
                refresh={() => {
                  feedback.set({ code: "stale_version", status: 409 });
                  read.clear();
                  read.restart();
                }}
              />
            </>
          )}
        </Match>
        <Match when={current()}>
          {(result) => {
            const failure = () => {
              const value = result();
              return value.ok ? null : value.error;
            };
            return (
              <Show when={failure()}>
                {(error) => <PrivateReadError error={error()} retry={read.restart} />}
              </Show>
            );
          }}
        </Match>
      </Switch>
    </>
  );
}

function ListingProjection(props: { readonly listing: OwnedListingV2 }): JSX.Element {
  const equipment = () => ownedEquipment(props.listing.view);
  const progress = () => {
    const view = props.listing.view;
    return view.kind === "resolving" ? view.progress : null;
  };
  const lastFailure = () => {
    const view = props.listing.view;
    return view.kind === "lookup_failed" ? view.last_lookup_reason : null;
  };
  // 损失回执按 position 查询；只有服务端给出损失案件时才链接（D9 §6.1）。
  const lossPosition = () =>
    props.listing.loss_case_id === null ? null : props.listing.position_id;
  return (
    <section class="public-consignment-account__detail">
      <h2>{t(listingViewKey(props.listing.view.kind))}</h2>
      <dl class="public-consignment-account__facts">
        <div>
          <dt>{t("consignment-world")}</dt>
          <dd>{consignmentWorldLabel(props.listing.world_id)}</dd>
        </div>
        <div>
          <dt>{t("consignment-eid")}</dt>
          <dd>{props.listing.external_equipment_id}</dd>
        </div>
        <div>
          <dt>{t("consignment-mode")}</dt>
          <dd>{t(listingModeKey(props.listing.mode))}</dd>
        </div>
        <div>
          <dt>{t("consignment-price")}</dt>
          <dd>{formatCreditAmount(props.listing.price)}</dd>
        </div>
        <div>
          <dt>{t("consignment-expires")}</dt>
          <dd>
            <Time value={props.listing.expires_at} />
          </dd>
        </div>
      </dl>
      <FeeQuote fee={props.listing.estimated_fee} net={props.listing.estimated_seller_amount} />
      <Show when={progress()}>{(value) => <Progress progress={value()} />}</Show>
      <Show when={equipment()}>{(value) => <EquipmentPanel equipment={value()} />}</Show>
      <Show when={props.listing.effective_reason}>
        {(reason) => <p>{t(listingReasonKey(reason()))}</p>}
      </Show>
      <Show when={lastFailure()}>{(reason) => <p>{t(listingLookupReasonKey(reason()))}</p>}</Show>
      <Show
        when={props.listing.mode === "consigned" && props.listing.view.kind === "awaiting_deposit"}
      >
        <Link href={`/consignment/me/${props.listing.listing_id}/deposit-instructions`}>
          {t("consignment-deposit-notice")}
        </Link>
      </Show>
      <Show when={props.listing.view.kind === "awaiting_deposit"}>
        <p>{t("consignment-mail-unavailable")}</p>
      </Show>
      <Show when={props.listing.view.kind === "closed" || props.listing.view.kind === "expired"}>
        <Link href="/consignment/new">{t("consignment-register-again")}</Link>
      </Show>
      <Show when={lossPosition()}>
        {(position) => (
          <Link href={lossReceiptHref(position())}>{t("consignment-loss-title")}</Link>
        )}
      </Show>
    </section>
  );
}

function Progress(props: { readonly progress: LookupProgress }): JSX.Element {
  return (
    <>
      <dl class="public-consignment-account__facts">
        <div>
          <dt>{t("consignment-attempts")}</dt>
          <dd>{`${props.progress.attempts} / ${props.progress.max_attempts}`}</dd>
        </div>
        <Show when={props.progress.next_attempt_at}>
          {(time) => (
            <div>
              <dt>{t("consignment-next-attempt")}</dt>
              <dd>
                <Time value={time()} />
              </dd>
            </div>
          )}
        </Show>
      </dl>
      <Show when={props.progress.paused}>
        <p>{t("consignment-paused")}</p>
      </Show>
      <Show when={props.progress.last_reason}>
        {(reason) => <p>{t(listingLookupReasonKey(reason()))}</p>}
      </Show>
    </>
  );
}

interface ActionDraft {
  readonly action: ListingActionV2;
  readonly version: number;
}

function SellerActions(props: {
  readonly listing: OwnedListingV2;
  readonly disabled: boolean;
  readonly submit: (input: ConsignmentIntent) => boolean;
  readonly refresh: () => void;
}): JSX.Element {
  const draft = createCell<ActionDraft | null>(null);
  const price = createCell("");
  const allowed = (action: ListingActionV2) => props.listing.allowed_actions.includes(action);
  const begin = (action: ListingActionV2) => {
    if (props.disabled || !allowed(action)) return;
    price.set(String(props.listing.price));
    draft.set({ action, version: props.listing.version });
  };
  const submit = () => {
    const current = draft.peek();
    if (current === null || props.disabled) return;
    if (props.listing.version !== current.version || !allowed(current.action)) {
      draft.set(null);
      props.refresh();
      return;
    }
    const target = { listing_id: props.listing.listing_id, expected_version: current.version };
    if (current.action === "withdraw") {
      // 新撤回一律走 v2 路由：未代管件回 closed，代管件登记意愿回 withdrawal_requested，
      // 已在库件直接建撤回代回 202 withdrawal_accepted。
      if (props.submit({ ...target, operation: "withdraw_deposit" })) draft.set(null);
      return;
    }
    const amount = listingPrice(price.peek());
    if (amount === null) return;
    if (props.submit({ ...target, operation: "reprice", price: amount })) draft.set(null);
  };

  return (
    <>
      <div class="public-consignment-account__actions">
        <Show when={allowed("reprice")}>
          <button type="button" disabled={props.disabled} onClick={() => begin("reprice")}>
            {t("consignment-reprice")}
          </button>
        </Show>
        <Show when={allowed("withdraw")}>
          <button type="button" disabled={props.disabled} onClick={() => begin("withdraw")}>
            {t("consignment-withdraw")}
          </button>
        </Show>
        <Show when={allowed("relist")}>
          <Link href={relistConsignmentHref(props.listing.listing_id)}>
            {t("consignment-relist")}
          </Link>
        </Show>
      </div>
      <Show when={draft()}>
        {(current) => (
          <Dialog
            title={t(
              current().action === "withdraw"
                ? "consignment-withdraw-confirm"
                : "consignment-reprice",
            )}
            closeLabel={t("dialog-cancel")}
            onClose={() => draft.set(null)}
          >
            <form
              class="public-consignment-account__form"
              onSubmit={(event) => {
                event.preventDefault();
                submit();
              }}
            >
              <Show when={current().action === "reprice"}>
                <label>
                  {t("consignment-price")}
                  <input
                    inputmode="numeric"
                    value={price()}
                    onInput={(event) => price.set(event.currentTarget.value)}
                  />
                </label>
                <Show when={listingPrice(price()) === null}>
                  <p role="alert">{t("consignment-invalid-price")}</p>
                </Show>
              </Show>
              <button
                type="submit"
                disabled={
                  props.disabled ||
                  (current().action === "reprice" && listingPrice(price()) === null)
                }
              >
                {t(
                  current().action === "withdraw" ? "consignment-withdraw" : "consignment-reprice",
                )}
              </button>
            </form>
          </Dialog>
        )}
      </Show>
    </>
  );
}
