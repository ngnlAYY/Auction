// Solid Public 装备登记页，能力受限时只展示说明。

import type { ListingMode } from "@auction/contracts/dto/public/consignment";
import { createCell } from "@auction/web-shared/reactive/cell";
import { Link, useRouter } from "@auction/web-shared/router";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { createEffect, onCleanup, Show } from "solid-js";

import type { ConsignmentPrivateError } from "../../api/consignment-private";
import { type ConsignmentCapability, useConsignmentCapability } from "./capability";
import { DirectRegistration } from "./direct-registration";
import { consignmentWorldLabel } from "./filters";
import { useConsignmentPending } from "./pending";
import { PendingCommand } from "./pending-view";
import {
  consignmentErrorMessage,
  FeeQuote,
  ownedConsignmentHref,
  PrivateConsignmentPage,
} from "./private-page";
import { useConsignmentRefresh } from "./refresh";
import { CapabilityNotice, ModeChoice } from "./registration-controls";
import {
  estimatedQuote,
  listingPrice,
  registrationEquipment,
  registrationFee,
} from "./registration-input";

/** 登记页入口，链接草稿由当前表单独占；未登录访客只可做非寄售登记。 */
export function RegistrationForm(): JSX.Element {
  return (
    <PrivateConsignmentPage title={t("consignment-new")} anonymous={() => <DirectRegistration />}>
      {(_userId, canUse) => <Registration canUse={canUse} />}
    </PrivateConsignmentPage>
  );
}

function Registration(props: { readonly canUse: () => boolean }): JSX.Element {
  const router = useRouter();
  const capability = useConsignmentCapability();
  const feeRejected = createCell<ConsignmentCapability | undefined>(undefined);
  const checkingFee = () => feeRejected() !== undefined && feeRejected() === capability();
  const status = () => {
    const value = capability();
    return value?.kind === "ready" && !checkingFee() ? value.status : null;
  };
  const mode = createCell<ListingMode | null>(null);
  const url = createCell("");
  const price = createCell("");
  const error = createCell<ConsignmentPrivateError | null>(null);
  let input: HTMLInputElement | null = null;
  const clearUrl = () => {
    url.set("");
    if (input !== null) input.value = "";
  };
  onCleanup(clearUrl);

  const pending = useConsignmentPending(props.canUse, (result, record) => {
    if (record.operation !== "register") return;
    if (result.ok) {
      if (result.value.listing_id !== null)
        router.push(ownedConsignmentHref(result.value.listing_id));
      return;
    }
    error.set(result.error);
    if (result.error.code === "fee_changed") {
      feeRejected.set(capability());
      capability.refresh();
    }
  });
  // 费率拒绝触发能力重读；刷新落定后提示已不再是当前输入的错误，随表单重新出现一并清除。
  createEffect(checkingFee, (checking) => {
    if (!checking && error.peek()?.code === "fee_changed") error.set(null);
  });
  useConsignmentRefresh(() => false, capability.refresh);

  const equipment = () => registrationEquipment(url());
  const amount = () => listingPrice(price(), status()?.min_price, status()?.max_price);
  const readyMode = () => {
    const selected = mode();
    return selected !== null && status()?.registration_modes.includes(selected) ? selected : null;
  };
  const quote = () => {
    const current = status();
    const selected = readyMode();
    const value = amount();
    return current === null || selected === null || value === null
      ? null
      : estimatedQuote(value, registrationFee(selected, current));
  };
  const availableWorld = () => {
    const world = equipment()?.world_id;
    return world !== undefined && status()?.registration_worlds.includes(world) === true;
  };
  const canSubmit = () =>
    props.canUse() &&
    !pending.blocked() &&
    readyMode() !== null &&
    availableWorld() &&
    amount() !== null;

  return (
    <>
      <Link href="/consignment/me">{t("consignment-mine")}</Link>
      <CapabilityNotice capability={capability()} refresh={capability.refresh} />
      <PendingCommand
        pending={pending}
        canUse={props.canUse()}
        registration
        canRetryRegistration={status() !== null}
      />
      <Show when={error()}>
        {(failure) => <p role="alert">{consignmentErrorMessage(failure())}</p>}
      </Show>
      <Show when={status()} keyed={false}>
        {(current) => (
          <form
            class="public-consignment-account__form"
            autocomplete="off"
            onSubmit={(event) => {
              event.preventDefault();
              const snapshot = status();
              const selected = readyMode();
              const identity = equipment();
              const value = amount();
              if (
                !canSubmit() ||
                snapshot === null ||
                selected === null ||
                identity === null ||
                value === null
              )
                return;
              // 不变量 1、6、7、10：冻结本次确认的身份、金额和费率，链接不进入恢复记录。
              error.set(null);
              if (
                pending.submit(
                  {
                    operation: "register",
                    ...identity,
                    mode: selected,
                    price: value,
                    expected_fee_bps: registrationFee(selected, snapshot),
                  },
                  url.peek(),
                )
              )
                clearUrl();
            }}
          >
            <fieldset disabled={pending.blocked() || !props.canUse()}>
              <ModeChoice status={current()} mode={readyMode()} onChange={mode.set} />
              <label>
                {t("consignment-equipment-url")}
                <input
                  type="password"
                  autocomplete="off"
                  spellcheck={false}
                  ref={(element) => {
                    input = element;
                  }}
                  value={url()}
                  onInput={(event) => url.set(event.currentTarget.value)}
                />
              </label>
              <Show
                when={equipment()}
                fallback={
                  <Show when={url() !== ""}>
                    <p role="alert">{t("consignment-invalid-equipment-url")}</p>
                  </Show>
                }
              >
                {(identity) => (
                  <p>
                    {t("consignment-world")} {consignmentWorldLabel(identity().world_id)}
                    {" · "}
                    {t("consignment-eid")} {identity().eid}
                  </p>
                )}
              </Show>
              <Show when={equipment() !== null && !availableWorld()}>
                <p role="alert">{t("consignment-world-unavailable")}</p>
              </Show>
              <label>
                {t("consignment-price")}
                <input
                  inputmode="numeric"
                  value={price()}
                  onInput={(event) => price.set(event.currentTarget.value)}
                />
              </label>
              <Show when={price() !== "" && amount() === null}>
                <p role="alert">{t("consignment-invalid-price")}</p>
              </Show>
              <Show when={quote()}>
                {(value) => <FeeQuote fee={value().fee} net={value().net} />}
              </Show>
              <p>{t("consignment-source-disclosure")}</p>
              <button type="submit" disabled={!canSubmit()}>
                {t("consignment-submit")}
              </button>
            </fieldset>
          </form>
        )}
      </Show>
      <Show when={checkingFee()}>
        <AsyncState kind="loading" title={t("data-loading")} detail="" />
      </Show>
    </>
  );
}
