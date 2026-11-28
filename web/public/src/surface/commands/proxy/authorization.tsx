// 已有代理授权的降低上限表单及其提交状态。

import { MAX_PUBLIC_BID_AMOUNT } from "@auction/contracts/dto/public/commands";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { formatCreditAmount } from "@auction/web-shared/ui/format";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { createEffect, onSettled, Show, useContext } from "solid-js";

import type { OwnedProxy } from "../../../api/account";
import { parseCreditAmount } from "../../catalog/presentation";
import { proxyStatusLabel } from "../../proxy";
import {
  amountHint,
  ConfirmPanel,
  initialProxyAmount,
  type PendingProxyCommand,
  type ProxyConfirmCommand,
  PublicCommandIdentityContext,
} from "../pending";

/** 已有代理授权的降低上限表单；授权标识与下限由服务端投影决定。 */
export function ProxyAuthorizationForm(props: {
  readonly lotId: string;
  readonly currentPrice: number;
  readonly nextBid: number;
  readonly lotStatus: string;
  readonly proxy: OwnedProxy;
  readonly canSubmit: boolean;
  /** 由弹窗宿主注入的两阶段确认命令 hook（降低上限实例；见 ProxyFormInput）。 */
  readonly command: ProxyConfirmCommand;
  readonly onSubmit: (pending: PendingProxyCommand) => void;
  readonly onClose: () => void;
}): JSX.Element {
  const identity = useContext(PublicCommandIdentityContext);
  const maximumId = () => `public-proxy-lower-maximum-${props.lotId}`;
  const lowerBound = createValueMemo(() =>
    props.proxy.capabilities.waiting ? props.nextBid : props.currentPrice,
  );
  const initialMaximum = initialProxyAmount(lowerBound());
  let lastDefault = initialMaximum;
  const maximumAmount = createCell(initialMaximum);
  // 下限随实时价格更新：草稿仍是上一份默认值时重同步，玩家已改写的输入保持原样。
  createEffect(lowerBound, (bound) => {
    const next = initialProxyAmount(bound);
    if (maximumAmount.peek() === lastDefault) maximumAmount.set(next);
    lastDefault = next;
  });
  const validation = createCell<string | null>(null);
  const biddingOpen = () => props.lotStatus === "live";
  const limitReached = () => lowerBound() > MAX_PUBLIC_BID_AMOUNT;
  const canLower = () =>
    props.proxy.capabilities.can_lower &&
    props.proxy.maximum_amount > lowerBound() &&
    biddingOpen() &&
    !limitReached();
  const submitDisabled = () => !canLower() || !props.canSubmit;
  const blockedReason = () => {
    if (!biddingOpen()) {
      return t("bid-unavailable");
    }
    if (limitReached()) {
      return t("bid-limit-note");
    }
    return t("proxy-cannot-lower");
  };
  const confirmationId = () => `public-proxy-lower-confirmation-${props.lotId}`;
  const confirmation = createValueMemo((): number | null => {
    const pending = props.command.confirmation();
    return pending !== null && pending.kind === "lower" ? pending.amount : null;
  });
  const confirmationView = createValueMemo(() => {
    const amount = confirmation();
    return amount === null ? null : { amount };
  });
  const confirming = () => confirmation() !== null;
  const focusMaximum = (element: HTMLInputElement) => {
    onSettled(() => {
      element.focus();
    });
  };

  return (
    <section class="public-auction-bid-form">
      <h2>{t("proxy-current")}</h2>
      <p>{t("proxy-status", { status: proxyStatusLabel(props.proxy.status) })}</p>
      <p>{t("proxy-maximum", { maximum: formatCreditAmount(props.proxy.maximum_amount) })}</p>
      <Show
        when={canLower()}
        fallback={
          <p class="action-feedback action-feedback--notice" role="note">
            {blockedReason()}
          </p>
        }
      >
        <form
          onSubmit={(event) => {
            event.preventDefault();
            if (submitDisabled()) {
              return;
            }
            const currentCommand = props.command;
            const currentConfirmation = currentCommand.confirmation();
            let pending: PendingProxyCommand;
            if (currentConfirmation !== null) {
              if (currentConfirmation.kind !== "lower") {
                currentCommand.reset();
                return;
              }
              if (currentConfirmation.amount > MAX_PUBLIC_BID_AMOUNT) {
                currentCommand.cancelConfirmation();
                validation.set(t("proxy-invalid-amount"));

                return;
              }
              if (currentConfirmation.authorizationId !== props.proxy.authorization_id) {
                // 授权已变更：整条命令作废，彻底清空后退回编辑态。
                currentCommand.reset();
                validation.set(t("proxy-authorization-not-active"));
                return;
              }
              if (currentConfirmation.amount >= props.proxy.maximum_amount) {
                currentCommand.cancelConfirmation();
                validation.set(t("proxy-not-lower"));

                return;
              }
              if (currentConfirmation.amount < lowerBound()) {
                currentCommand.cancelConfirmation();
                validation.set(
                  t("proxy-below-minimum", {
                    price: formatCreditAmount(lowerBound()),
                  }),
                );

                return;
              }
              pending = currentConfirmation;
            } else {
              const value = parseCreditAmount(maximumAmount());
              if (value === null) {
                validation.set(t("proxy-invalid-amount"));

                return;
              }
              if (value >= props.proxy.maximum_amount) {
                validation.set(t("proxy-not-lower"));

                return;
              }
              if (value < lowerBound()) {
                validation.set(
                  t("proxy-below-minimum", {
                    price: formatCreditAmount(lowerBound()),
                  }),
                );

                return;
              }
              const authorizationId = props.proxy.authorization_id;
              currentCommand.begin(`${value}:${authorizationId}`, (commandId) => ({
                kind: "lower",
                userId: identity.userId,
                amount: value,
                authorizationId,
                commandId,
              }));
              validation.set(null);
              return;
            }
            validation.set(null);
            // 上面的确认守卫已保证此处必为 Lower。
            if (pending.kind !== "lower") {
              return;
            }
            props.onSubmit(pending);
          }}
        >
          <Show
            when={confirmationView()}
            fallback={
              <>
                <p class="public-auction-bid-form__meta">
                  {t("bid-minimum", { price: formatCreditAmount(lowerBound()) })}
                </p>
                <label for={maximumId()}>{t("proxy-lower-label")}</label>
                <input
                  id={maximumId()}
                  name="maximum_amount"
                  type="text"
                  autofocus={true}
                  // 与创建代理一致，允许在手机键盘输入 K/M 缩写。
                  inputmode="text"
                  enterkeyhint="done"
                  autocapitalize="none"
                  autocomplete="off"
                  spellcheck="false"
                  aria-invalid={validation() !== null ? "true" : "false"}
                  aria-describedby={
                    validation() !== null
                      ? "public-proxy-lower-hint public-proxy-lower-error"
                      : "public-proxy-lower-hint"
                  }
                  value={maximumAmount()}
                  ref={focusMaximum}
                  onInput={(event) => {
                    maximumAmount.set(event.currentTarget.value);
                    validation.set(null);
                  }}
                />
                {amountHint(maximumAmount(), "public-proxy-lower-hint", "proxy-invalid-amount")}
                <p class="public-auction-bid-form__hint">{t("proxy-shorthand-hint")}</p>
              </>
            }
          >
            {(amount) => (
              <ConfirmPanel
                id={confirmationId()}
                typeLabel={t("bid-type-proxy-lower")}
                amount={formatCreditAmount(amount().amount)}
                note={t("proxy-binding-note")}
              />
            )}
          </Show>
          <Show when={validation() !== null}>
            <p
              id="public-proxy-lower-error"
              class="action-feedback action-feedback--error"
              role="alert"
            >
              {validation()}
            </p>
          </Show>
          <div class="public-auction-bid-form__actions">
            <button
              type="button"
              onClick={() => {
                if (props.command.confirmation() !== null) {
                  props.command.cancelConfirmation();
                } else {
                  props.onClose();
                }
              }}
            >
              {confirming() ? t("proxy-confirm-back") : t("dialog-cancel")}
            </button>
            <button
              type="submit"
              // 确认视图的首选焦点是确认按钮，而不是弹窗右上角的关闭按钮。
              autofocus={confirming()}
              disabled={submitDisabled()}
              aria-describedby={confirming() ? confirmationId() : ""}
            >
              {confirming() ? t("proxy-lower-confirm-submit") : t("proxy-lower-submit")}
            </button>
          </div>
        </form>
      </Show>
    </section>
  );
}
