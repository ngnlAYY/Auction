// 外壳正文中的未知竞价命令和离开原弹窗后的回执。

import { focusElementById } from "@auction/web-shared/platform/a11y";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { createEffect, For, Show, useContext } from "solid-js";
import { CompetitionCommandsContext } from "./host";
import { commandKindLabel } from "./request";
import { CommandRecovery, commandReceiptMessage } from "./retry";

const NOTICE_TITLE = "public-command-notices-title";
const CONTENT = "public-content";

function closeId(commandId: string): string {
  return `public-command-receipt-close-${commandId}`;
}

/** 所有路由都可见；播报节点先于条目挂载，收到结果不会抢焦点。 */
export function CompetitionNotices(): JSX.Element {
  const commands = useContext(CompetitionCommandsContext);
  const store = () => commands?.current() ?? null;
  const unknown = createValueMemo(() => store()?.unknownCommands() ?? []);
  const receipts = () => store()?.receipts() ?? [];
  const latest = () => {
    const receipt = store()?.latestReceipt();
    return receipt == null ? [] : [receipt];
  };
  const title = () => t("command-unknown-count", { count: unknown().length });
  const hasAnnounced = createCell(false);
  createEffect(
    () => unknown().length,
    (count) => {
      if (count > 0 && !hasAnnounced.peek()) hasAnnounced.set(true);
    },
  );

  return (
    <>
      <div class="public-command-notices__status" role="status" aria-atomic="true">
        <Show when={unknown().length > 0 || hasAnnounced()}>
          <span>{title()}</span>
        </Show>
      </div>
      <div class="public-command-notices__status" role="status" aria-atomic="true">
        <For each={latest()} keyed={(receipt) => receipt.commandId}>
          {(entry) => (
            <span>{`${entry().target.lotName} · ${commandReceiptMessage(entry())}`}</span>
          )}
        </For>
      </div>
      <Show when={unknown().length > 0}>
        <section class="public-command-notices" aria-labelledby={NOTICE_TITLE}>
          <h2 id={NOTICE_TITLE} class="public-command-notices__title" tabindex="-1">
            {title()}
          </h2>
          <p class="public-command-notices__boundary">{t("command-memory-only")}</p>
          <ul class="public-command-notices__list">
            <For each={unknown()} keyed={(command) => command.payload.commandId}>
              {(command) => (
                <li class="public-command-notices__item">
                  <CommandRecovery command={command()} />
                </li>
              )}
            </For>
          </ul>
        </section>
      </Show>
      <Show when={receipts().length > 0}>
        <ul class="public-command-notices__receipts" aria-label={t("command-results-label")}>
          <For each={receipts()} keyed={(receipt) => receipt.commandId}>
            {(receipt, index) => (
              <li class="public-command-notices__receipt">
                <p>
                  <b>{receipt().target.lotName}</b>
                  {" · "}
                  {commandKindLabel(receipt().kind)}
                  {" · "}
                  {commandReceiptMessage(receipt())}
                </p>
                <button
                  id={closeId(receipt().commandId)}
                  class="public-command-notices__receipt-close"
                  type="button"
                  aria-label={t("command-dismiss-result", { lot: receipt().target.lotName })}
                  onClick={() => {
                    const current = commands?.current.peek();
                    if (current == null) return;
                    const remaining = receipts()[index() + 1] ?? receipts()[index() - 1];
                    focusElementById(
                      remaining === undefined
                        ? unknown().length > 0
                          ? NOTICE_TITLE
                          : CONTENT
                        : closeId(remaining.commandId),
                    );
                    current.dismiss(receipt().commandId);
                  }}
                >
                  {t("catalog-close")}
                </button>
              </li>
            )}
          </For>
        </ul>
      </Show>
    </>
  );
}
