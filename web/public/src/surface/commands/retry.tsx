// 结果不确定命令的原样重试；只重放已提交负载，不重新应用新命令的价格/能力校验。

import type { Cell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { unreachable } from "@auction/web-shared/result";
import { formatCreditAmount } from "@auction/web-shared/ui/format";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { onSettled, Show, useContext } from "solid-js";

import {
  type CommandError,
  commandErrorOutcomeUncertain,
  createProxyAuthorization,
  lowerProxyAuthorization,
  type ProxyCommandAuthorization,
  placeBid,
} from "../../api/commands";
import { PublicRefreshGenerationContext } from "../board/refresh";
import type { BidSuccess } from "./bid";
import {
  type BidConfirmCommand,
  bidErrorMessage,
  type PendingProxyCommand,
  type ProxyConfirmCommand,
  proxyErrorMessage,
} from "./pending";

/** 只有同一 lot/用户宿主保留的已提交命令可以进入此视图；输入只读，取消后重新开始。 */
export function BidRetry(props: {
  readonly lotId: string;
  readonly lotCode: string;
  readonly command: BidConfirmCommand;
  readonly onSuccess: (success: BidSuccess) => void;
  readonly onClose: () => void;
}): JSX.Element {
  const refresh = useContext(PublicRefreshGenerationContext);
  const pending = () => props.command.command();
  const description = () => {
    const current = pending();
    return current === null
      ? ""
      : t("bid-confirm-description", {
          amount: formatCreditAmount(current.amount),
          code: props.lotCode,
        });
  };
  const error = createValueMemo(() => {
    const current = pending();
    const failure = props.command.failure();
    return current === null || failure === null
      ? null
      : retryErrorMessage(failure, (value) => bidErrorMessage(value, current.amount));
  });
  const focusRetry = (element: HTMLButtonElement) => {
    onSettled(() => {
      element.focus();
    });
  };

  return (
    <Show when={pending()}>
      {(_current) => (
        <form
          class="public-auction-bid-form"
          aria-busy={props.command.submitting() ? "true" : "false"}
          onSubmit={(event) => {
            event.preventDefault();
            if (props.command.submitting() || !props.command.uncertain()) {
              return;
            }
            const current = pending();
            if (current === null) {
              return;
            }
            const lotId = props.lotId;
            props.command.submit(
              current,
              (submitted, signal) =>
                placeBid(submitted.userId, lotId, submitted.amount, submitted.commandId, signal),
              commandErrorOutcomeUncertain,
              (result) => {
                // 与普通出价表单一致：先重读权威投影，再由宿主关闭弹窗并提示结果。
                refresh.set(nextRefreshGeneration(refresh()));
                props.onSuccess({ leading: result.leading, currentPrice: result.current_price });
              },
              () => refresh.set(nextRefreshGeneration(refresh())),
            );
          }}
        >
          <p>{t("command-retry-unchanged")}</p>
          <p>{description()}</p>
          <Show when={error() !== null}>
            <p class="action-feedback action-feedback--error" role="alert">
              {error()}
            </p>
          </Show>
          <div class="public-auction-bid-form__actions">
            <button
              type="button"
              disabled={props.command.submitting()}
              onClick={() => props.onClose()}
            >
              {t("dialog-cancel")}
            </button>
            <button
              type="submit"
              disabled={props.command.submitting()}
              // 提交期间确认按钮被禁用，焦点已退到弹窗面板；重试视图挂载时把焦点交给
              // 唯一的主操作，键盘用户不必再从关闭按钮一路 Tab 过来。
              ref={focusRetry}
            >
              {props.command.submitting() ? t("bid-working") : t("data-retry")}
            </button>
          </div>
        </form>
      )}
    </Show>
  );
}

/** 原样重试被拒绝时的文案。重试按服务端当时的价格重新判定，`bid_too_low` 只说明价格已变化：
 * 本次提交的金额不是最低价，不能代入“至少为”文案，改为提示取消后按当前价格重新操作。
 * 其余错误码沿用新命令的映射（`commandMessage` 的金额参数只影响 `bid_too_low`）。 */
function retryErrorMessage(
  error: CommandError,
  commandMessage: (error: CommandError) => string,
): string {
  if (error.code === "bid_too_low") {
    return t("command-retry-price-changed");
  }
  return commandMessage(error);
}

/** 不依赖刷新后的活动授权形状，创建已生效或上限已降低时仍可读取原命令结果。 */
export function ProxyRetry(props: {
  readonly lotId: string;
  readonly command: ProxyConfirmCommand;
  readonly savedAuthorization: Cell<ProxyCommandAuthorization | null>;
  readonly onClose: () => void;
}): JSX.Element {
  const refresh = useContext(PublicRefreshGenerationContext);
  const pending = () => props.command.command();
  const description = () => {
    const current = pending();
    if (current === null) {
      return "";
    }
    if (current.kind === "create") {
      return t("proxy-create-confirm-description", {
        maximum: formatCreditAmount(current.amount),
        mode: current.mode === "slow" ? t("proxy-mode-slow") : t("proxy-mode-fast"),
      });
    }
    return t("proxy-lower-confirm-description", {
      maximum: formatCreditAmount(current.amount),
    });
  };
  const error = createValueMemo(() => {
    const current = pending();
    const failure = props.command.failure();
    return current === null || failure === null
      ? null
      : retryErrorMessage(failure, (value) => proxyErrorMessage(value, current.amount));
  });
  const focusRetry = (element: HTMLButtonElement) => {
    onSettled(() => {
      element.focus();
    });
  };

  return (
    <Show when={pending()}>
      {(_current) => (
        <form
          class="public-auction-bid-form"
          aria-busy={props.command.submitting() ? "true" : "false"}
          onSubmit={(event) => {
            event.preventDefault();
            if (props.command.submitting() || !props.command.uncertain()) {
              return;
            }
            const current = pending();
            if (current === null) {
              return;
            }
            const lotId = props.lotId;
            props.command.submit(
              current,
              (submitted: PendingProxyCommand, signal) => {
                switch (submitted.kind) {
                  case "create":
                    return createProxyAuthorization(
                      submitted.userId,
                      lotId,
                      submitted.amount,
                      submitted.mode,
                      submitted.commandId,
                      signal,
                    );
                  case "lower":
                    return lowerProxyAuthorization(
                      submitted.userId,
                      lotId,
                      submitted.authorizationId,
                      submitted.amount,
                      submitted.commandId,
                      signal,
                    );
                  default:
                    return unreachable(submitted);
                }
              },
              commandErrorOutcomeUncertain,
              (authorization) => {
                props.savedAuthorization.set(authorization);
                refresh.set(nextRefreshGeneration(refresh()));
              },
              () => refresh.set(nextRefreshGeneration(refresh())),
            );
          }}
        >
          <p>{t("command-retry-unchanged")}</p>
          <p>{description()}</p>
          <Show when={error() !== null}>
            <p class="action-feedback action-feedback--error" role="alert">
              {error()}
            </p>
          </Show>
          <div class="public-auction-bid-form__actions">
            <button
              type="button"
              disabled={props.command.submitting()}
              onClick={() => props.onClose()}
            >
              {t("dialog-cancel")}
            </button>
            <button
              type="submit"
              disabled={props.command.submitting()}
              // 提交期间确认按钮被禁用，焦点已退到弹窗面板；重试视图挂载时把焦点交给
              // 唯一的主操作，键盘用户不必再从关闭按钮一路 Tab 过来。
              ref={focusRetry}
            >
              {props.command.submitting() ? t("bid-working") : t("data-retry")}
            </button>
          </div>
        </form>
      )}
    </Show>
  );
}

function nextRefreshGeneration(value: number): number {
  return value >= Number.MAX_SAFE_INTEGER ? Number.MAX_SAFE_INTEGER : value + 1;
}
