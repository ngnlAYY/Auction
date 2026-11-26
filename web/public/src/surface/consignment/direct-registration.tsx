// 未登录访客的非寄售登记：自填游戏角色名，寄售模式提示登录；受理后可按命令 ID 查看进度。

import type { DirectListingProgress, ListingMode } from "@auction/contracts/dto/public/consignment";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createTaskScope } from "@auction/web-shared/reactive/tasks";
import { utf8ByteLength } from "@auction/web-shared/text";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { onCleanup, Show } from "solid-js";

import { fetchDirectCommand } from "../../api/consignment-direct";
import type { ConsignmentPrivateError } from "../../api/consignment-private";
import { useConsignmentCapability } from "./capability";
import { type DirectPending, useDirectPending } from "./direct-pending";
import { consignmentWorldLabel } from "./filters";
import { listingReasonKey, listingStateKey } from "./labels";
import { ReenterUrl } from "./pending-view";
import { consignmentErrorMessage, LoginPrompt } from "./private-page";
import { CapabilityNotice, ModeChoice } from "./registration-controls";
import { listingPrice, registrationEquipment } from "./registration-input";

const MAX_SELLER_GAME_NAME_BYTES = 128;

/** 本地只拦截明显无效的名称；规范与凭据形状由服务端裁定。 */
function sellerGameName(text: string): string | null {
  return text !== "" && text === text.trim() && utf8ByteLength(text) <= MAX_SELLER_GAME_NAME_BYTES
    ? text
    : null;
}

/** 免登录登记入口；寄售仍需账户会话。 */
export function DirectRegistration(): JSX.Element {
  const capability = useConsignmentCapability();
  const status = () => {
    const value = capability();
    return value?.kind === "ready" ? value.status : null;
  };
  const mode = createCell<ListingMode | null>(null);
  const name = createCell("");
  const url = createCell("");
  const price = createCell("");
  const error = createCell<ConsignmentPrivateError | null>(null);
  const accepted = createCell<{ command_id: string; listing_id: string } | null>(null);
  let input: HTMLInputElement | null = null;
  const clearUrl = () => {
    url.set("");
    if (input !== null) input.value = "";
  };
  onCleanup(clearUrl);

  const pending = useDirectPending((result) => {
    if (result.ok) accepted.set(result.value);
    else error.set(result.error);
  });

  const readyMode = () => {
    const selected = mode();
    return selected !== null && status()?.registration_modes.includes(selected) ? selected : null;
  };
  const equipment = () => registrationEquipment(url());
  const amount = () => listingPrice(price(), status()?.min_price, status()?.max_price);
  const availableWorld = () => {
    const world = equipment()?.world_id;
    return world !== undefined && status()?.registration_worlds.includes(world) === true;
  };
  const canSubmit = () =>
    !pending.blocked() &&
    readyMode() === "direct" &&
    sellerGameName(name()) !== null &&
    availableWorld() &&
    amount() !== null;

  return (
    <>
      <CapabilityNotice capability={capability()} refresh={capability.refresh} />
      <DirectPendingNotice pending={pending} />
      <Show when={error()}>
        {(failure) => <p role="alert">{consignmentErrorMessage(failure())}</p>}
      </Show>
      <Show when={accepted()} keyed>
        {(receipt) => (
          <DirectAccepted
            commandId={receipt.command_id}
            onAnother={() => {
              if (pending.dismiss()) accepted.set(null);
            }}
          />
        )}
      </Show>
      <Show when={accepted() === null ? status() : null} keyed={false}>
        {(current) => (
          <form
            class="public-consignment-account__form"
            autocomplete="off"
            onSubmit={(event) => {
              event.preventDefault();
              const seller = sellerGameName(name.peek());
              const identity = equipment();
              const value = amount();
              if (!canSubmit() || seller === null || identity === null || value === null) return;
              // 不变量 1、6、7：冻结本次确认的名称、装备标识与金额，链接不进入恢复记录。
              error.set(null);
              if (
                pending.submit({ seller_game_name: seller, ...identity, price: value }, url.peek())
              )
                clearUrl();
            }}
          >
            <fieldset disabled={pending.blocked()}>
              <ModeChoice status={current()} mode={readyMode()} onChange={mode.set} />
              <Show when={readyMode() === "consigned"}>
                <LoginPrompt message={t("consignment-consigned-login-required")} />
              </Show>
              <Show when={readyMode() === "direct"}>
                <label>
                  {t("consignment-seller-game-name")}
                  <input
                    autocomplete="off"
                    spellcheck={false}
                    value={name()}
                    onInput={(event) => name.set(event.currentTarget.value)}
                  />
                </label>
                <Show when={name() !== "" && sellerGameName(name()) === null}>
                  <p role="alert">{t("consignment-invalid-seller-game-name")}</p>
                </Show>
                <p>{t("consignment-seller-game-name-notice")}</p>
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
                <p>{t("consignment-source-disclosure")}</p>
                <button type="submit" disabled={!canSubmit()}>
                  {t("consignment-submit")}
                </button>
              </Show>
            </fieldset>
          </form>
        )}
      </Show>
    </>
  );
}

/** 结果未知时只能查原决定；查无记录后才可重新输入同一装备的链接原样重送。 */
function DirectPendingNotice(props: { readonly pending: DirectPending }): JSX.Element {
  return (
    <>
      <Show when={props.pending.record()}>
        <section class="public-consignment-account__pending" aria-label={t("consignment-pending")}>
          <p role="status">{t("consignment-pending")}</p>
          <button
            type="button"
            disabled={props.pending.busy()}
            onClick={() => props.pending.check()}
          >
            {t("data-retry")}
          </button>
          <Show when={props.pending.missing()}>
            <ReenterUrl disabled={props.pending.busy()} onRetry={props.pending.retry} />
          </Show>
        </section>
      </Show>
      <Show when={props.pending.error()}>
        {(error) => <p role="alert">{consignmentErrorMessage(error())}</p>}
      </Show>
    </>
  );
}

/** 受理回执：免登录挂单没有本人页，凭命令 ID 读取当前进度。 */
function DirectAccepted(props: {
  readonly commandId: string;
  readonly onAnother: () => void;
}): JSX.Element {
  const progress = createCell<DirectListingProgress | null>(null);
  const error = createCell<ConsignmentPrivateError | null>(null);
  const busy = createCell(false);
  const tasks = createTaskScope();
  const refresh = () => {
    if (busy.peek() || !tasks.alive()) return;
    busy.set(true);
    error.set(null);
    tasks.spawn(async (context) => {
      const result = await fetchDirectCommand(props.commandId, context.signal);
      if (!context.alive()) return;
      busy.set(false);
      if (result.ok) progress.set(result.value.listing);
      else error.set(result.error);
    });
  };
  // 聚合拒绝态没有独立文案，只显示原因。
  const stateLabel = (current: DirectListingProgress) => {
    const key = listingStateKey(current.effective_state);
    return key === null ? "" : t(key);
  };
  return (
    <section
      class="public-consignment-account__pending"
      aria-label={t("consignment-direct-accepted")}
    >
      <p role="status">{t("consignment-direct-accepted")}</p>
      <Show when={progress()}>
        {(current) => (
          <p>
            {stateLabel(current())}
            <Show when={current().effective_reason}>
              {(reason) => <> {t(listingReasonKey(reason()))}</>}
            </Show>
          </p>
        )}
      </Show>
      <button type="button" disabled={busy()} onClick={refresh}>
        {t("consignment-direct-progress")}
      </button>
      <button type="button" onClick={() => props.onAnother()}>
        {t("consignment-direct-register-another")}
      </button>
      <Show when={error()}>
        {(failure) => <p role="alert">{consignmentErrorMessage(failure())}</p>}
      </Show>
    </section>
  );
}
