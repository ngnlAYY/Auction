//! 外壳的展示树：只读取外壳传入的状态与回调，不建立资源或业务副作用。

import { focusElementById } from "@auction/web-shared/platform/a11y";
import { ThemePicker } from "@auction/web-shared/platform/theme";
import type { Cell } from "@auction/web-shared/reactive/cell";
import { Link, type RouteMatch, RouteOutlet } from "@auction/web-shared/router";
import { reportAppFailure } from "@auction/web-shared/ui/bootstrap";
import { reloadAfterPageLoadError } from "@auction/web-shared/ui/bootstrap/page-load-recovery";
import { LanguagePicker, t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";

import { PublicRealtimeOwner, type PublicRealtimeTarget } from "../board/realtime";
import { ConsoleCountdown, ConsoleSidebar } from "../presentation/console";
import { Navigation } from "../presentation/mobile";
import type { LiveConnectionState } from "../query";
import { PUBLIC_LOCALE_CATALOG } from "../router/locale-catalog";
import { PUBLIC_PAGES } from "../router/route";
import { SessionMenu } from "../session";

export interface PublicShellViewProps {
  readonly navigationOpen: Cell<boolean>;
  readonly sidebarCollapsed: Cell<boolean>;
  readonly manualRefreshBusy: Cell<boolean>;
  readonly manualRefreshError: Cell<boolean>;
  readonly sessionEndedElsewhere: Cell<boolean>;
  readonly contentLoading: Cell<ReadonlySet<number>>;
  readonly refreshGeneration: Cell<number>;
  readonly auctionAfter: Cell<readonly [string, number] | null>;
  readonly realtimeState: Cell<LiveConnectionState>;
  readonly currentRoute: () => RouteMatch;
  readonly auctionHomeHref: () => string;
  readonly scopedHref: (path: string) => string;
  readonly auctionHomeNeedsQueryReset: () => boolean;
  readonly connectionClass: () => string;
  readonly connectionStateName: () => string;
  readonly connectionTitle: () => string;
  readonly connectionMessage: () => string;
  readonly tickerTitle: () => string;
  readonly tickerStatus: () => string;
  readonly tickerWorld: () => string;
  readonly tickerCountdown: () => {
    readonly label: string;
    readonly targetAt: string;
    readonly expiredLabel: string;
  } | null;
  readonly realtimeUserId: () => string | null;
  readonly realtimeTarget: () => PublicRealtimeTarget;
  readonly onToggleNavigation: () => void;
  readonly onCloseNavigation: () => void;
  readonly onHome: () => void;
  readonly onManualRefresh: () => void;
  readonly onToggleSidebar: () => void;
}

export function PublicShellView(props: PublicShellViewProps): JSX.Element {
  return (
    <div
      class={
        props.sidebarCollapsed()
          ? "public-shell public-console is-sidebar-collapsed"
          : "public-shell public-console"
      }
    >
      <PublicRealtimeOwner
        userId={props.realtimeUserId()}
        target={props.realtimeTarget()}
        refreshGeneration={props.refreshGeneration}
        auctionAfter={props.auctionAfter}
        realtimeState={props.realtimeState}
      />
      <a class="console-skip" href="#public-content">
        {t("skip")}
      </a>
      <header class="console-ticker">
        <div class="console-ticker__left">
          <Navigation
            auctionHomeHref={props.auctionHomeHref()}
            currentRoute={props.currentRoute()}
            scopedHref={props.scopedHref}
            resetHomeQuery={props.auctionHomeNeedsQueryReset()}
            open={props.navigationOpen()}
            onToggle={props.onToggleNavigation}
            onClose={props.onCloseNavigation}
            onHome={props.onHome}
          />
          <span class="console-ticker__brand">{t("app-title")}</span>
          <span class="console-ticker__sep" aria-hidden="true" />
          <div class="console-ticker__context">
            <Show
              when={props.contentLoading().size > 0}
              fallback={
                <>
                  <Show when={props.tickerStatus() !== ""}>
                    <span class="console-ticker__live">{props.tickerStatus()}</span>
                    <span class="console-ticker__sep" aria-hidden="true" />
                  </Show>
                  <span class="console-ticker__auction">{props.tickerTitle()}</span>
                  {/* 整段写成一个文本节点：拆成“ / ”与世界名两个节点后，字形排布会差约 0.016px。 */}
                  <span class="console-ticker__world">{` / ${props.tickerWorld()}`}</span>
                  <Show when={props.tickerCountdown()}>
                    {(countdown) => (
                      <>
                        <span class="console-ticker__sep" aria-hidden="true" />
                        <span class="console-ticker__label">{countdown().label}</span>
                        <ConsoleCountdown
                          targetAt={countdown().targetAt}
                          expiredLabel={countdown().expiredLabel}
                        />
                      </>
                    )}
                  </Show>
                </>
              }
            >
              <span
                class="console-ticker__loading"
                role="status"
                aria-busy="true"
                aria-label={t("data-loading")}
              >
                <span class="console-ticker__spinner" aria-hidden="true">
                  ↻
                </span>
                {t("common-loading")}
              </span>
            </Show>
          </div>
        </div>
        <div class="console-ticker__right">
          <div class="public-shell__connection">
            <span
              class={["public-connection-badge", props.connectionClass()]}
              data-state={props.connectionStateName()}
              role="status"
              title={props.connectionTitle()}
            >
              <span class="public-connection-badge__label">{props.connectionMessage()}</span>
            </span>
            <button
              class="public-connection-refresh"
              type="button"
              disabled={props.manualRefreshBusy()}
              aria-busy={props.manualRefreshBusy() ? "true" : "false"}
              title={t("realtime-refresh")}
              aria-label={t("realtime-refresh")}
              onClick={props.onManualRefresh}
            >
              <span
                class={props.manualRefreshBusy() ? "public-connection-refresh__spinner" : ""}
                aria-hidden="true"
              >
                ↻
              </span>
            </button>
            <Show when={props.manualRefreshError()}>
              <span class="public-connection-refresh__error" role="alert">
                {t("data-error")}
              </span>
            </Show>
          </div>
          <span class="console-ticker__sep" aria-hidden="true" />
          <div class="public-shell__preferences">
            <LanguagePicker label={t("language-label")} catalog={PUBLIC_LOCALE_CATALOG} />
            <ThemePicker
              label={t("theme-label")}
              darkLabel={t("theme-dark")}
              lightLabel={t("theme-light")}
            />
          </div>
          <span class="console-ticker__sep" aria-hidden="true" />
          <div class="console-ticker__session">
            <SessionMenu scopedHref={props.scopedHref} />
          </div>
        </div>
      </header>
      <div class={props.sessionEndedElsewhere() ? "public-shell__session-ended-banner" : ""}>
        <span class="public-shell__session-ended-text" role="status">
          {props.sessionEndedElsewhere() ? t("session-ended-elsewhere") : ""}
        </span>
        <Show when={props.sessionEndedElsewhere()}>
          <Link class="public-shell__session-ended-login" href={props.scopedHref("/login")}>
            {t("session-ended-relogin")}
          </Link>
          <button
            class="public-shell__session-ended-dismiss"
            type="button"
            aria-label={t("catalog-close")}
            onClick={(event) => {
              if (document.activeElement === event.currentTarget) {
                focusElementById("public-content");
              }
              props.sessionEndedElsewhere.set(false);
            }}
          >
            ✕
          </button>
        </Show>
      </div>
      <div class="console-body">
        <ConsoleSidebar
          currentRoute={props.currentRoute()}
          scopedHref={props.scopedHref}
          collapsed={props.sidebarCollapsed()}
          onToggle={props.onToggleSidebar}
          onNavigate={props.onCloseNavigation}
        />
        <main id="public-content" class="console-main" tabindex="-1">
          <RouteOutlet
            pages={PUBLIC_PAGES}
            notFound={null}
            onLoadError={(error) => {
              // 页面代码加载失败多半是部署后旧标签页引用的分块已不存在：先整页重载一次，
              // 同一地址短时间内已经重载过才显示错误横幅。
              if (!reloadAfterPageLoadError()) {
                reportAppFailure(error);
              }
            }}
          />
        </main>
      </div>
    </div>
  );
}
