// 代理授权创建表单及其提交状态。

import { MAX_PUBLIC_BID_AMOUNT } from "@auction/contracts/dto/public/commands";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { formatCreditAmount } from "@auction/web-shared/ui/format";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { onSettled, Show, useContext } from "solid-js";

import { parseCreditAmount } from "../../catalog/presentation";
import {
  amountHint,
  ConfirmPanel,
  initialProxyAmount,
  type PendingProxyCommand,
  type ProxyConfirmCommand,
  PublicCommandIdentityContext,
} from "../pending";

/** 代理授权创建表单；新建模式与上限只按冻结策略和当前 lot 状态计算。 */
export function ProxyCreateForm(props: {
  readonly lotId: string;
  readonly initialMinimum: number;
  readonly lotStatus: string;
  readonly allowSlow: boolean;
  /** 该场冻结策略的代理默认模式；未知时表单失败关闭。 */
  readonly defaultMode: string | null;
  readonly canSubmit: boolean;
  readonly blocked: boolean;
  readonly blockedReason: string;
  /** 由弹窗宿主注入的两阶段确认命令 hook（创建实例；见 ProxyFormInput）。 */
  readonly command: ProxyConfirmCommand;
  readonly onSubmit: (pending: PendingProxyCommand) => void;
  readonly onClose: () => void;
}): JSX.Element {
  const identity = useContext(PublicCommandIdentityContext);
  const maximumId = () => `public-proxy-maximum-${props.lotId}`;
  const modeName = () => `public-proxy-mode-${props.lotId}`;
  const fastModeId = () => `${modeName()}-fast`;
  const slowModeId = () => `${modeName()}-slow`;
  const fastHintId = () => `${modeName()}-fast-hint`;
  const slowHintId = () => `${modeName()}-slow-hint`;
  const slowReasonId = () => `${modeName()}-slow-disabled`;
  const slowDescribedBy = () =>
    props.allowSlow ? slowHintId() : `${slowHintId()} ${slowReasonId()}`;
  const maximumAmount = createCell(initialProxyAmount(props.initialMinimum));
  const initialMode = createValueMemo(() => initialProxyMode(props.defaultMode, props.allowSlow));
  // 策略未知时整张表单禁用（见下方 `blocked`），占位选项不会被提交。
  const mode = createCell(initialMode() ?? "fast");
  const validation = createCell<string | null>(null);
  // 冻结策略的默认模式未知（只有往期目录拼装的已结算场次）时不提供新建，不替玩家假设模式。
  const biddingOpen = () => props.lotStatus === "live" && initialMode() !== null;
  const limitReached = () => props.initialMinimum > MAX_PUBLIC_BID_AMOUNT;
  const blocked = () => props.blocked || !biddingOpen() || limitReached();
  const submitDisabled = () => blocked() || !props.canSubmit;
  const blockedReason = () => {
    if (!biddingOpen()) {
      return t("bid-unavailable");
    }
    if (limitReached()) {
      return t("bid-limit-note");
    }
    return props.blockedReason;
  };
  const confirmationId = () => `public-proxy-create-confirmation-${props.lotId}`;
  const confirmation = createValueMemo((): { amount: number; modeLabel: string } | null => {
    const pending = props.command.confirmation();
    if (pending === null || pending.kind !== "create") {
      return null;
    }
    return {
      amount: pending.amount,
      modeLabel: pending.mode === "slow" ? t("proxy-mode-slow") : t("proxy-mode-fast"),
    };
  });
  const confirming = () => confirmation() !== null;
  const focusMaximum = (element: HTMLInputElement) => {
    onSettled(() => {
      element.focus();
    });
  };

  return (
    <section class="public-auction-bid-form">
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
            if (currentConfirmation.kind !== "create") {
              currentCommand.reset();
              return;
            }
            if (currentConfirmation.amount > MAX_PUBLIC_BID_AMOUNT) {
              currentCommand.cancelConfirmation();
              validation.set(t("proxy-invalid-amount"));

              return;
            }
            if (currentConfirmation.amount < props.initialMinimum) {
              currentCommand.cancelConfirmation();
              validation.set(
                t("proxy-below-minimum", {
                  price: formatCreditAmount(props.initialMinimum),
                }),
              );

              return;
            }
            if (currentConfirmation.mode === "slow" && !props.allowSlow) {
              currentCommand.cancelConfirmation();
              validation.set(t("proxy-slow-forbidden"));

              return;
            }
            pending = currentConfirmation;
          } else {
            const value = parseCreditAmount(maximumAmount());
            if (value === null) {
              validation.set(t("proxy-invalid-amount"));

              return;
            }
            if (value < props.initialMinimum) {
              validation.set(
                t("proxy-below-minimum", {
                  price: formatCreditAmount(props.initialMinimum),
                }),
              );

              return;
            }
            const selectedMode = mode();
            if (selectedMode === "slow" && !props.allowSlow) {
              validation.set(t("proxy-slow-forbidden"));

              return;
            }
            currentCommand.begin(`${value}:${selectedMode}`, (commandId) => ({
              kind: "create",
              userId: identity.userId,
              amount: value,
              mode: selectedMode,
              commandId,
            }));
            validation.set(null);
            return;
          }
          validation.set(null);
          // 上面的确认守卫已保证此处必为 Create。
          if (pending.kind !== "create") {
            return;
          }
          props.onSubmit(pending);
        }}
      >
        <Show
          when={confirmation()}
          fallback={
            <>
              {/* 当前价与最低价已由弹窗顶部的拍品摘要展示，表单内不再重复。 */}
              <label for={maximumId()}>{t("proxy-create-maximum-label")}</label>
              <input
                id={maximumId()}
                name="maximum_amount"
                type="text"
                autofocus={true}
                // 代理上限允许 K/M 缩写，手机键盘必须保留字母输入入口。
                inputmode="text"
                enterkeyhint="done"
                autocapitalize="none"
                autocomplete="off"
                spellcheck="false"
                aria-invalid={validation() !== null ? "true" : "false"}
                aria-describedby={
                  validation() !== null
                    ? "public-proxy-amount-hint public-proxy-error"
                    : "public-proxy-amount-hint"
                }
                disabled={blocked()}
                value={maximumAmount()}
                ref={focusMaximum}
                onInput={(event) => {
                  maximumAmount.set(event.currentTarget.value);
                  validation.set(null);
                }}
              />
              {amountHint(maximumAmount(), "public-proxy-amount-hint", "proxy-invalid-amount")}
              <p class="public-auction-bid-form__meta">{t("proxy-maximum-hint")}</p>
              <p class="public-auction-bid-form__hint">{t("proxy-shorthand-hint")}</p>
              <fieldset disabled={blocked()}>
                <legend>{t("proxy-create-mode-label")}</legend>
                <div class="public-auction-bid-form__mode">
                  <input
                    id={fastModeId()}
                    type="radio"
                    name={modeName()}
                    value="fast"
                    checked={mode() === "fast"}
                    aria-describedby={fastHintId()}
                    onChange={() => mode.set("fast")}
                  />
                  <label for={fastModeId()}>{t("proxy-mode-fast")}</label>
                  <p id={fastHintId()} class="public-auction-bid-form__mode-hint">
                    {t("proxy-mode-fast-hint")}
                  </p>
                </div>
                <div class="public-auction-bid-form__mode">
                  <input
                    id={slowModeId()}
                    type="radio"
                    name={modeName()}
                    value="slow"
                    checked={mode() === "slow"}
                    disabled={!props.allowSlow}
                    aria-describedby={slowDescribedBy()}
                    onChange={() => mode.set("slow")}
                  />
                  <label for={slowModeId()}>{t("proxy-mode-slow")}</label>
                  <p id={slowHintId()} class="public-auction-bid-form__mode-hint">
                    {t("proxy-mode-slow-hint")}
                  </p>
                </div>
              </fieldset>
              <Show when={!props.allowSlow}>
                <p id={slowReasonId()} class="public-auction-bid-form__hint" role="note">
                  {t("proxy-slow-forbidden")}
                </p>
              </Show>
            </>
          }
        >
          {(current) => (
            <ConfirmPanel
              id={confirmationId()}
              typeLabel={`${t("bid-tab-proxy")} · ${current().modeLabel}`}
              amount={formatCreditAmount(current().amount)}
              note={t("proxy-binding-note")}
            />
          )}
        </Show>
        <Show when={blocked()}>
          <p class="action-feedback action-feedback--notice" role="note">
            {blockedReason()}
          </p>
        </Show>
        <Show when={validation() !== null}>
          <p id="public-proxy-error" class="action-feedback action-feedback--error" role="alert">
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
            // 确认视图的首选焦点是确认按钮；否则焦点陷阱会落到弹窗右上角的关闭按钮，
            // 再按一次 Enter 就成了关闭弹窗。编辑态的首选焦点仍是上限框。
            autofocus={confirming()}
            disabled={submitDisabled()}
            aria-describedby={confirming() ? confirmationId() : ""}
          >
            {confirming() ? t("proxy-create-confirm-submit") : t("proxy-create-submit")}
          </button>
        </div>
      </form>
    </section>
  );
}

/** 代理新建表单的初始模式：取该场冻结策略的默认模式；慢速当前不可新建（终阶段）时退为快速，
 * 玩家仍可切换，提交照常显式携带模式。策略未知或不在闭集内时返回 `null`，由表单失败关闭。
 * 仅供测试使用。 */
export function initialProxyMode(
  defaultMode: string | null,
  allowSlow: boolean,
): "slow" | "fast" | null {
  if (defaultMode === null) {
    return null;
  }
  switch (defaultMode) {
    case "slow":
      return allowSlow ? "slow" : "fast";
    case "fast":
      return "fast";
    default:
      return null;
  }
}
