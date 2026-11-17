//! 原生装备标记按钮：忙碌、冲突与未知结果重放均可见且可用键盘操作。

import type { PublicSetLotWatchRequest } from "@auction/contracts/dto/public/watch";
import { Link } from "@auction/web-shared/router";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { Match, Show, Switch } from "solid-js";

import { loginHrefForCurrentLocation } from "../session";
import { tryUseWatchContext, useWatchContext } from ".";
import { requireWatchCommandOwner } from "./commands";

/** 目录行的“已标记”展示状态，用于行首色条。与标记按钮读取同一份整场状态，不发请求；
 * 匿名、身份不匹配或不在标记作用域内时为 false。 */
export function useLotWatched(lotId: () => string): () => boolean {
  const context = tryUseWatchContext();
  return () => {
    if (context === null) {
      return false;
    }
    return context.identityMatches() && context.displayed(lotId());
  };
}

/** 只消费整场 owner 的状态；不会在目录每行发起 GET。 */
export function LotWatchButton(props: {
  readonly lotId: string;
  readonly lotCode: string;
  readonly iconOnly?: boolean | null;
}): JSX.Element {
  const iconOnly = () => props.iconOnly ?? false;
  const context = useWatchContext();
  const submit = requireWatchCommandOwner().submit;
  const session = context.session;
  const snapshot = () => session();
  const identityMatches = () => context.identityMatches();
  const loginVisible = () => {
    const current = snapshot();
    return current?.ok && current.value === null;
  };
  const sessionFailed = () => {
    const current = snapshot();
    return current?.ok === false;
  };
  const authoritative = () => context.authoritative(props.lotId);
  const pending = () => context.commands().get(props.lotId) ?? null;
  const watched = () => context.displayed(props.lotId);
  const busy = () => pending()?.busy === true;
  const error = () => pending()?.error ?? null;
  const uncertain = () => pending()?.uncertain === true;
  const conflict = () => error()?.status === 409 && error()?.code === "watch_conflict";
  const errorMessage = () =>
    uncertain() ? t("watch-unknown") : conflict() ? t("watch-conflict") : t("watch-save-error");
  const errorId = () => `public-lot-watch-error-${props.lotId}`;

  const toggle = () => {
    if (!context.identityMatches() || context.commands.peek().has(props.lotId)) {
      return;
    }
    const current = context.authoritative(props.lotId);
    if (current === null) {
      return;
    }
    const request: PublicSetLotWatchRequest = {
      command_id: crypto.randomUUID(),
      watched: !current[0],
      channel: "mm",
      expected_version: current[1],
    };
    submit(context.auctionId, props.lotId, props.lotCode, request);
  };
  const retry = () => {
    if (!context.identityMatches()) {
      return;
    }
    const pendingCommand = context.commands.peek().get(props.lotId);
    if (pendingCommand !== undefined && !pendingCommand.busy && pendingCommand.uncertain) {
      submit(context.auctionId, props.lotId, props.lotCode, pendingCommand.payload);
    }
  };
  const refresh = () => {
    if (!context.identityMatches()) {
      return;
    }
    const commands = context.commands.peek();
    const pendingCommand = commands.get(props.lotId);
    // 即使旧刷新事件延迟到达，也不能清除正在提交或结果未知的命令。
    if (pendingCommand?.busy === true || pendingCommand?.uncertain === true) {
      return;
    }
    const next = new Map(commands);
    next.delete(props.lotId);
    context.commands.set(next);
    context.invalidate();
  };

  return (
    <Show
      when={!identityMatches()}
      fallback={
        <span class="public-lot-watch" aria-busy={busy() ? "true" : "false"}>
          <button
            class="public-lot-watch__toggle"
            type="button"
            aria-pressed={watched() ? "true" : "false"}
            aria-label={t("watch-toggle", { code: props.lotCode })}
            aria-describedby={error() !== null ? errorId() : ""}
            disabled={authoritative() === null || pending() !== null}
            onClick={toggle}
          >
            <span class="public-lot-watch__icon" aria-hidden="true">
              {/* biome-ignore lint/a11y/noSvgWithoutTitle: 装饰图形由祖先 aria-hidden 隐藏，源码不输出标题。 */}
              <svg
                viewBox="0 0 24 24"
                fill={watched() ? "currentColor" : "none"}
                stroke="currentColor"
                stroke-width="1.5"
                stroke-linecap="round"
                stroke-linejoin="round"
              >
                <path d="M6 3h12a1 1 0 0 1 1 1v17l-7-4-7 4V4a1 1 0 0 1 1-1z" />
              </svg>
            </span>
            <span class="public-lot-watch__label">
              {busy() ? t("watch-saving") : watched() ? t("watch-on") : t("watch-off")}
            </span>
          </button>
          <Show when={error() !== null}>
            <span id={errorId()} class="public-lot-watch__error" role="alert">
              {errorMessage()}
            </span>
            <Show
              when={uncertain()}
              fallback={
                <button type="button" onClick={refresh}>
                  {t("watch-refresh")}
                </button>
              }
            >
              <button type="button" onClick={retry}>
                {t("watch-retry-command")}
              </button>
            </Show>
          </Show>
        </span>
      }
    >
      <Switch fallback={null}>
        <Match when={loginVisible()}>
          <Link
            class="public-lot-watch__login"
            href={loginHrefForCurrentLocation()}
            aria-label={t("watch-login")}
          >
            <Show when={iconOnly()} fallback={t("watch-login")}>
              <span class="public-lot-watch__icon" aria-hidden="true">
                {/* biome-ignore lint/a11y/noSvgWithoutTitle: 装饰图形由祖先 aria-hidden 隐藏，源码不输出标题。 */}
                <svg
                  viewBox="0 0 24 24"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="1.5"
                  stroke-linecap="round"
                  stroke-linejoin="round"
                >
                  <path d="M6 3h12a1 1 0 0 1 1 1v17l-7-4-7 4V4a1 1 0 0 1 1-1z" />
                </svg>
              </span>
            </Show>
          </Link>
        </Match>
        <Match when={sessionFailed()}>
          <span class="public-lot-watch__error" role="alert">
            {t("session-error")}
          </span>
        </Match>
      </Switch>
    </Show>
  );
}
