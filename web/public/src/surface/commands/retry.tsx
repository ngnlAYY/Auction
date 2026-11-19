// 未结命令的统一恢复入口、放弃确认与安全回执文案。

import { createCell } from "@auction/web-shared/reactive/cell";
import { Link, useRouter } from "@auction/web-shared/router";
import { Dialog } from "@auction/web-shared/ui/components/dialog";
import { formatCreditAmount } from "@auction/web-shared/ui/format";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { Show, useContext } from "solid-js";
import { type CommandError, commandNotSent } from "../../api/commands";
import { LotLinkLocationContext } from "../catalog/detail/lot-view";
import { type CommandDialogOrigin, CompetitionCommandsContext } from "./host";
import { bidErrorMessage, proxyErrorMessage } from "./pending";
import {
  type CommandReceipt,
  type CommandTarget,
  commandBusy,
  commandKindLabel,
  replayConflict,
  type SentCommand,
} from "./request";

/** 原样重试不能用已提交金额替代当前最低价。 */
export function retryErrorMessage(
  error: CommandError,
  commandMessage: (error: CommandError) => string,
): string {
  return error.code === "bid_too_low" ? t("command-retry-price-changed") : commandMessage(error);
}

function commandErrorMessage(kind: CommandReceipt["kind"], error: CommandError): string {
  if (commandNotSent(error)) return t("command-not-sent");
  return retryErrorMessage(error, (value) =>
    kind === "bid" ? bidErrorMessage(value, 0) : proxyErrorMessage(value, 0),
  );
}

/** 回执只确认命令结果；实际价格与代理授权另由权威读取取得。 */
export function commandReceiptMessage(receipt: CommandReceipt): string {
  return receipt.error === null
    ? t(receipt.kind === "bid" ? "bid-success" : "proxy-saved")
    : commandErrorMessage(receipt.kind, receipt.error);
}

function ViewCommandLot(props: { readonly target: CommandTarget }): JSX.Element {
  const location = useContext(LotLinkLocationContext);
  const router = useRouter();
  const href = () => `/lots/${props.target.lotId}`;
  return (
    <Link
      href={href()}
      onClickOnly={true}
      aria-label={t("catalog-view-lot", { code: props.target.lotCode })}
      onClick={() => {
        const { lotId, auctionId, world } = props.target;
        const next = href();
        router.push(next);
        if (router.currentLocationString() === next) {
          location.set({ lotId, auctionId, world, route: router.current() });
        }
      }}
    >
      {t("command-view-lot")}
    </Link>
  );
}

/** 弹窗与常驻提示区共用同一条命令的只读状态和恢复操作。 */
export function CommandRecovery(props: {
  readonly command: SentCommand;
  readonly origin?: CommandDialogOrigin;
}): JSX.Element {
  const commands = useContext(CompetitionCommandsContext);
  const confirming = createCell(false);
  const command = () => props.command;
  const target = () => command().target;
  const payload = () => command().payload;
  const busy = () => commandBusy(command());
  const unknown = () => command().status.kind === "unknown";
  const paused = () => unknown() && commands?.current()?.canSubmit() !== true;
  const inDialog = props.origin !== undefined;
  const summaryId = () => `public-command-${inDialog ? "dialog" : "notice"}-${target().lotId}`;
  const messageId = () => `${summaryId()}-status`;
  const pauseId = () => `${summaryId()}-pause`;
  const description = () =>
    paused() ? `${summaryId()} ${messageId()} ${pauseId()}` : `${summaryId()} ${messageId()}`;
  const message = () => {
    const { status } = command();
    if (status.kind === "submitting") return t("bid-working");
    if (status.retrying) return t("command-retrying");
    return replayConflict(status.error)
      ? t("command-replay-conflict")
      : commandErrorMessage(payload().kind, status.error);
  };

  return (
    <div class="public-command-notices__entry">
      <p id={summaryId()} class="public-command-notices__summary">
        <b>{target().lotName}</b>
        <span>{commandKindLabel(payload().kind)}</span>
        <span>{formatCreditAmount(payload().amount)}</span>
      </p>
      <Show when={inDialog && unknown()}>
        <p>{t("command-retry-unchanged")}</p>
        <p>{t("command-memory-only")}</p>
      </Show>
      {/* 状态节点随条目常驻；操作组忙碌期间也能播报进度、拒绝原因和复核暂停。 */}
      <div
        class="public-command-notices__entry-status"
        role="status"
        aria-atomic="true"
        aria-labelledby={summaryId()}
      >
        <p id={messageId()} class="public-command-notices__message">
          {message()}
        </p>
        <Show when={paused()}>
          <p id={pauseId()} class="public-command-notices__message">
            {t("command-retry-session-pending")}
          </p>
        </Show>
      </div>
      <Show when={unknown()}>
        <div class="public-command-notices__actions" aria-busy={busy() ? "true" : "false"}>
          <button
            type="button"
            disabled={busy() || paused()}
            aria-describedby={description()}
            onClick={() =>
              commands?.current
                .peek()
                ?.retry(target().lotId, payload().commandId, props.origin ?? null)
            }
          >
            {t("data-retry")}
          </button>
          <Show when={!inDialog}>
            <ViewCommandLot target={target()} />
          </Show>
          <button
            type="button"
            disabled={busy()}
            aria-haspopup="dialog"
            aria-describedby={summaryId()}
            onClick={() => {
              if (!busy()) confirming.set(true);
            }}
          >
            {t("command-abandon")}
          </button>
        </div>
        <Show when={confirming()}>
          <Dialog
            title={t("command-abandon-title")}
            closeLabel={t("catalog-close")}
            onClose={() => confirming.set(false)}
          >
            <div class="public-command-notices__confirmation">
              <p>{t("command-abandon-description")}</p>
              <div class="public-command-notices__actions">
                <button type="button" autofocus={true} onClick={() => confirming.set(false)}>
                  {t("dialog-cancel")}
                </button>
                <button
                  type="button"
                  onClick={() => {
                    commands?.current.peek()?.abandon(target().lotId, payload().commandId);
                    confirming.set(false);
                  }}
                >
                  {t("command-abandon")}
                </button>
              </div>
            </div>
          </Dialog>
        </Show>
      </Show>
    </div>
  );
}
