// 普通竞价表单和命令提交。

import { MAX_PUBLIC_BID_AMOUNT } from "@auction/contracts/dto/public/commands";
import { createCell } from "@auction/web-shared/reactive/cell";
import { formatCreditAmount } from "@auction/web-shared/ui/format";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { onSettled, Show, useContext } from "solid-js";

import { commandErrorOutcomeUncertain, placeBid } from "../../api/commands";
import { PublicRefreshGenerationContext } from "../board/refresh";
import { rustTrim } from "../query/text";
import {
  type BidConfirmCommand,
  bidErrorMessage,
  ConfirmPanel,
  type PendingBidCommand,
  PublicCommandIdentityContext,
} from "./pending";

const CREDIT_K = 1_000;

/** 普通竞价成功后的浏览器展示投影；只保留成功提示所需字段。 */
export interface BidSuccess {
  readonly leading: boolean;
  readonly currentPrice: number;
}

/** 普通竞价表单参数；关闭与成功动作分开表达，避免调用方误用弹窗生命周期。 */
export interface BidFormInput {
  readonly lotId: string;
  readonly nextBid: number;
  readonly lotStatus: string;
  readonly canBid: boolean;
  readonly blocked: boolean;
  readonly blockedReason: string;
  /** 由弹窗宿主注入的两阶段确认命令 hook；宿主据其 `submitting` 在提交中禁用遮罩/Escape/关闭按钮。 */
  readonly command: BidConfirmCommand;
  /** 命令成功后把结果交给弹窗宿主：宿主关闭弹窗并显示页面级提示条。 */
  readonly onSuccess: (success: BidSuccess) => void;
  readonly onClose: () => void;
}

/** 普通出价输入固定使用 K 单位；向上取整保证非整千的旧起拍价不会生成低于最低价的默认值。 */
function defaultBidUnits(nextBid: number): string {
  const units = Math.trunc(nextBid / CREDIT_K) + (nextBid % CREDIT_K > 0 ? 1 : 0);
  return Math.max(units, 1).toString();
}

/** 普通出价只接受纯数字 K 单位，避免把代理表单保留的 M/K 缩写规则扩散到确认流程。 */
function parseBidUnits(input: string): number | null {
  const units = rustTrim(input);
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

function bidAmountHint(value: string): JSX.Element {
  if (rustTrim(value) === "") {
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
  const submitting = () => command().submitting();
  const refreshGeneration = useContext(PublicRefreshGenerationContext);
  const confirming = () => command().confirmation() !== null;
  const biddingOpen = () => input().canBid && input().lotStatus === "live";
  const limitReached = () => input().nextBid > MAX_PUBLIC_BID_AMOUNT;
  const blocked = () => input().blocked || !biddingOpen() || limitReached();
  const blockedReason = () => {
    if (!biddingOpen()) {
      return t("bid-unavailable");
    }
    if (limitReached()) {
      return t("bid-limit-note");
    }
    return input().blockedReason;
  };
  const focusAmount = (element: HTMLInputElement) => {
    onSettled(() => {
      element.focus();
    });
  };

  return (
    <form
      class="public-auction-bid-form"
      aria-busy={submitting() ? "true" : "false"}
      onSubmit={(event) => {
        event.preventDefault();
        if (submitting() || blocked()) {
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
            currentCommand.failure.set(null);
            return;
          }
          if (confirmation.amount < current.nextBid) {
            currentCommand.cancelConfirmation();
            validation.set(t("bid-below-minimum", { price: formatCreditAmount(current.nextBid) }));
            currentCommand.failure.set(null);
            return;
          }
          pending = confirmation;
        } else {
          const value = parseBidUnits(amount());
          if (value === null) {
            validation.set(t("bid-invalid-amount"));
            currentCommand.failure.set(null);
            return;
          }
          if (value < current.nextBid) {
            validation.set(t("bid-below-minimum", { price: formatCreditAmount(current.nextBid) }));
            currentCommand.failure.set(null);
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
        const lotId = current.lotId;
        currentCommand.submit(
          pending,
          (submitted, signal) =>
            placeBid(submitted.userId, lotId, submitted.amount, submitted.commandId, signal),
          commandErrorOutcomeUncertain,
          (result) => {
            // 先重读权威价格与领先者，再把结果交给宿主；宿主随即关闭弹窗，
            // 所以这里之后不能再依赖表单自身的状态。
            refreshGeneration.set(nextRefreshGeneration(refreshGeneration()));
            current.onSuccess({ leading: result.leading, currentPrice: result.current_price });
          },
          () => {
            // 失败也重读权威价格与领先者。
            refreshGeneration.set(nextRefreshGeneration(refreshGeneration()));
          },
        );
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
              pattern="[0-9]*"
              autocomplete="off"
              spellcheck="false"
              aria-invalid={validation() !== null ? "true" : "false"}
              aria-describedby={
                validation() !== null
                  ? "public-bid-amount-hint public-bid-unit-hint public-bid-error"
                  : "public-bid-amount-hint public-bid-unit-hint"
              }
              disabled={submitting() || blocked()}
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
            id="public-bid-confirmation"
            typeLabel={t("bid-tab-direct")}
            amount={formatCreditAmount(pending().amount)}
            note={t("bid-binding-note")}
          />
        )}
      </Show>
      <Show when={blocked()}>
        <p class="action-feedback action-feedback--notice" role="note">
          {blockedReason()}
        </p>
      </Show>
      <Show when={validation() !== null}>
        <p id="public-bid-error" class="action-feedback action-feedback--error" role="alert">
          {validation()}
        </p>
      </Show>
      <Show when={command().failure()}>
        {(error) => (
          <p class="action-feedback action-feedback--error" role="alert">
            {bidErrorMessage(error(), input().nextBid)}
          </p>
        )}
      </Show>
      <div class="public-auction-bid-form__actions">
        <button
          type="button"
          disabled={submitting()}
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
          disabled={submitting() || blocked()}
          aria-describedby={confirming() ? "public-bid-confirmation" : ""}
        >
          {submitting()
            ? t("bid-working")
            : confirming()
              ? t("bid-confirm-submit")
              : t("bid-submit")}
        </button>
      </div>
    </form>
  );
}

function nextRefreshGeneration(value: number): number {
  return value >= Number.MAX_SAFE_INTEGER ? Number.MAX_SAFE_INTEGER : value + 1;
}
