// 普通竞价表单和命令提交。

import { MAX_PUBLIC_BID_AMOUNT } from "@auction/contracts/dto/public/commands";
import { createCell } from "@auction/web-shared/reactive/cell";
import { formatCreditAmount } from "@auction/web-shared/ui/format";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { onSettled, Show, useContext } from "solid-js";

import {
  type BidConfirmCommand,
  ConfirmPanel,
  type PendingBidCommand,
  PublicCommandIdentityContext,
} from "./pending";

const CREDIT_K = 1_000;
const BID_WHITESPACE = String.raw`[ \x09-\x0D]`;
const BID_CONFIRMATION_ID = "public-bid-confirmation";
const BID_BLOCKED_REASON_ID = "public-bid-blocked-reason";

/** 普通出价输入的原生词法约束。 */
export const BID_UNITS_PATTERN = `${BID_WHITESPACE}*[0-9]*${BID_WHITESPACE}*`;

/** 普通竞价表单参数；已确认的输入交给外壳，关闭只清理当前界面。 */
export interface BidFormInput {
  readonly lotId: string;
  readonly nextBid: number;
  readonly lotStatus: string;
  readonly canBid: boolean;
  readonly canSubmit: boolean;
  readonly blocked: boolean;
  readonly blockedReason: string;
  readonly command: BidConfirmCommand;
  readonly onSubmit: (pending: PendingBidCommand) => void;
  readonly onClose: () => void;
}

/** 普通出价输入固定使用 K 单位；向上取整保证非整千的旧起拍价不会生成低于最低价的默认值。 */
function defaultBidUnits(nextBid: number): string {
  const units = Math.trunc(nextBid / CREDIT_K) + (nextBid % CREDIT_K > 0 ? 1 : 0);
  return Math.max(units, 1).toString();
}

/** 普通出价只接受纯数字 K 单位，避免把代理表单保留的 M/K 缩写规则扩散到确认流程。 */
export function parseBidUnits(input: string): number | null {
  const units = trimBidAmount(input);
  if (units === "" || !isAsciiDigits(units)) {
    return null;
  }
  const parsed = Number(units);
  if (!Number.isSafeInteger(parsed)) {
    return null;
  }
  const amount = parsed * CREDIT_K;
  return Number.isSafeInteger(amount) && amount > 0 && amount <= MAX_PUBLIC_BID_AMOUNT
    ? amount
    : null;
}

function trimBidAmount(value: string): string {
  let start = 0;
  let end = value.length;
  // 只检查两端；数字中间的空白保留给词法校验拒绝。
  while (start < end && isBidWhitespace(value.charCodeAt(start))) {
    start += 1;
  }
  while (end > start && isBidWhitespace(value.charCodeAt(end - 1))) {
    end -= 1;
  }
  return value.slice(start, end);
}

function isBidWhitespace(code: number): boolean {
  return code === 0x20 || (code >= 0x09 && code <= 0x0d);
}

function bidAmountHint(value: string): JSX.Element {
  if (trimBidAmount(value) === "") {
    // biome-ignore lint/complexity/noUselessFragments: 源码在空值时不渲染任何节点。
    return <></>;
  }
  const amount = parseBidUnits(value);
  if (amount !== null) {
    return (
      <p id="public-bid-amount-hint" class="public-auction-bid-form__amount-hint" role="status">
        {`= ${formatCreditAmount(amount)}`}
      </p>
    );
  }
  return (
    <p
      id="public-bid-amount-hint"
      class="public-auction-bid-form__amount-hint public-auction-bid-form__amount-hint--invalid"
      role="alert"
    >
      {t("bid-invalid-amount")}
    </p>
  );
}

function isAsciiDigits(value: string): boolean {
  for (let index = 0; index < value.length; index += 1) {
    const code = value.charCodeAt(index);
    if (code < 0x30 || code > 0x39) {
      return false;
    }
  }
  return true;
}

/** 二次确认时关联金额面板，并在显示禁用原因时附上该说明。 */
function bidSubmitDescriptionIds(confirming: boolean, blocked: boolean): string | undefined {
  if (!confirming) {
    return undefined;
  }
  return blocked ? `${BID_CONFIRMATION_ID} ${BID_BLOCKED_REASON_ID}` : BID_CONFIRMATION_ID;
}

/** 为表单建立独立组件作用域，避免弹窗开关改变父组件的 Hook 顺序。 */
export function BidForm(props: { readonly input: BidFormInput }): JSX.Element {
  return bidForm(() => props.input);
}

/** 显示普通竞价表单；只由 BidForm 无条件调用，最低价与 lot 状态的最终判断留在服务端。 */
function bidForm(input: () => BidFormInput): JSX.Element {
  const amountId = () => `public-bid-amount-${input().lotId}`;
  const identity = useContext(PublicCommandIdentityContext);
  const amount = createCell(defaultBidUnits(input().nextBid));
  const validation = createCell<string | null>(null);
  const command = () => input().command;
  const confirming = () => command().confirmation() !== null;
  const biddingOpen = () => input().canBid && input().lotStatus === "live";
  const limitReached = () => input().nextBid > MAX_PUBLIC_BID_AMOUNT;
  const blocked = () => input().blocked || !biddingOpen() || limitReached();
  const submitDisabled = () => blocked() || !input().canSubmit;
  const blockedReason = () => {
    if (!biddingOpen()) {
      return t("bid-unavailable");
    }
    if (limitReached()) {
      return t("bid-limit-note");
    }
    return input().blockedReason;
  };
  // 说明文字为空（如已有代理授权）时不渲染说明节点，按钮也不引用它。
  const showBlockedReason = () => blocked() && blockedReason() !== "";
  const focusAmount = (element: HTMLInputElement) => {
    onSettled(() => {
      element.focus();
    });
  };

  return (
    <form
      class="public-auction-bid-form"
      onSubmit={(event) => {
        event.preventDefault();
        if (submitDisabled()) {
          return;
        }
        const current = input();
        const currentCommand = current.command;
        // 两阶段确认：签名取金额序列化，同一金额重试复用同一 command_id（幂等）。
        let pending: PendingBidCommand;
        const confirmation = currentCommand.confirmation();
        if (confirmation !== null) {
          if (confirmation.amount > MAX_PUBLIC_BID_AMOUNT) {
            currentCommand.cancelConfirmation();
            validation.set(t("bid-invalid-amount"));

            return;
          }
          if (confirmation.amount < current.nextBid) {
            currentCommand.cancelConfirmation();
            validation.set(t("bid-below-minimum", { price: formatCreditAmount(current.nextBid) }));

            return;
          }
          pending = confirmation;
        } else {
          const value = parseBidUnits(amount());
          if (value === null) {
            validation.set(t("bid-invalid-amount"));

            return;
          }
          if (value < current.nextBid) {
            validation.set(t("bid-below-minimum", { price: formatCreditAmount(current.nextBid) }));

            return;
          }
          currentCommand.begin(value.toString(), (commandId) => ({
            userId: identity.userId,
            amount: value,
            commandId,
          }));
          validation.set(null);
          return;
        }
        validation.set(null);
        current.onSubmit(pending);
      }}
    >
      <Show
        when={command().confirmation()}
        fallback={
          <>
            {/* 当前价与最低价已由弹窗顶部的拍品摘要展示，表单内不再重复。 */}
            <label for={amountId()}>{t("bid-amount-label")}</label>
            <input
              id={amountId()}
              name="amount"
              type="text"
              autofocus={true}
              inputmode="numeric"
              enterkeyhint="done"
              pattern={BID_UNITS_PATTERN}
              autocomplete="off"
              spellcheck="false"
              aria-invalid={validation() !== null ? "true" : "false"}
              aria-describedby={
                validation() !== null
                  ? "public-bid-amount-hint public-bid-unit-hint public-bid-error"
                  : "public-bid-amount-hint public-bid-unit-hint"
              }
              disabled={blocked()}
              value={amount()}
              ref={focusAmount}
              onInput={(event) => {
                amount.set(event.currentTarget.value);
                validation.set(null);
              }}
            />
            {bidAmountHint(amount())}
            <p id="public-bid-unit-hint" class="public-auction-bid-form__hint">
              {t("bid-shorthand-hint")}
            </p>
          </>
        }
      >
        {(pending) => (
          <ConfirmPanel
            id={BID_CONFIRMATION_ID}
            typeLabel={t("bid-tab-direct")}
            amount={formatCreditAmount(pending().amount)}
            note={t("bid-binding-note")}
          />
        )}
      </Show>
      <Show when={showBlockedReason()}>
        <p id={BID_BLOCKED_REASON_ID} class="action-feedback action-feedback--notice" role="note">
          {blockedReason()}
        </p>
      </Show>
      <Show when={validation() !== null}>
        <p id="public-bid-error" class="action-feedback action-feedback--error" role="alert">
          {validation()}
        </p>
      </Show>
      <div class="public-auction-bid-form__actions">
        <button
          type="button"
          onClick={() => {
            if (command().confirmation() !== null) {
              command().cancelConfirmation();
            } else {
              input().onClose();
            }
          }}
        >
          {confirming() ? t("bid-confirm-back") : t("dialog-cancel")}
        </button>
        <button
          type="submit"
          // 用 Enter 提交金额后输入框被确认面板替换；确认视图的首选焦点是确认按钮，
          // 而不是弹窗右上角的关闭按钮。编辑态的首选焦点仍是金额框。
          autofocus={confirming()}
          disabled={submitDisabled()}
          aria-describedby={bidSubmitDescriptionIds(confirming(), showBlockedReason())}
        >
          {confirming() ? t("bid-confirm-submit") : t("bid-submit")}
        </button>
      </div>
    </form>
  );
}
