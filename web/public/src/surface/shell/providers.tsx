//! 外壳上下文装配：把外壳持有的状态与资源一次性提供给页面和展示树。
//! provider 树只负责组合，不建立资源、不读取地址栏，也不改变各上下文的生命周期。

import type { CurrentAuctionItemResponse } from "@auction/contracts/dto/public/catalog/auction";
import { type SecondTicker, SecondTickerContext } from "@auction/web-shared/platform/clock";
import type { PageVisibility } from "@auction/web-shared/platform/visibility";
import type { Cell } from "@auction/web-shared/reactive/cell";
import type { JSX } from "@solidjs/web";
import {
  PublicDisplayedAuctionContext,
  PublicPageRealtimeTargetContext,
  type PublicRealtimePageKey,
  type PublicRealtimeTarget,
} from "../board/realtime";
import {
  type PublicBidRefresh,
  PublicBidRefreshContext,
  PublicRefreshGenerationContext,
} from "../board/refresh";
import type { LotViewTarget } from "../catalog/detail/deep-link";
import { LotViewRequestContext } from "../catalog/detail/deep-link";
import type {
  CurrentAuctionResource,
  PublicReadinessResource,
  PublicSessionResource,
} from "../index";
import { PublicCommandOwnerContext, PublicSessionResourceContext } from "../index";
import { PublicContentLoadingContext } from "../loading";
import type { PublicBoardQuery } from "../query";
import {
  CurrentAuctionContext,
  PageVisibilityContext,
  PublicReadinessContext,
  PublicWorldQueryContext,
  SelectedWorldContext,
} from "./contexts";

export interface PublicShellProvidersProps {
  readonly secondTicker: SecondTicker;
  readonly selectedWorld: Cell<string>;
  readonly worldQuerySignal: Cell<PublicBoardQuery>;
  readonly refreshGeneration: Cell<number>;
  readonly bidRefresh: PublicBidRefresh;
  readonly pageRealtimeTarget: Cell<readonly [PublicRealtimePageKey, PublicRealtimeTarget] | null>;
  readonly displayedAuction: Cell<CurrentAuctionItemResponse | null>;
  readonly lotViewRequest: Cell<LotViewTarget | null>;
  readonly session: PublicSessionResource;
  readonly commandOwner: Cell<string | null>;
  readonly contentLoading: Cell<ReadonlySet<number>>;
  readonly readiness: PublicReadinessResource;
  readonly currentAuction: CurrentAuctionResource;
  readonly pageVisibility: PageVisibility;
  readonly children: JSX.Element;
}

/** 外壳根部的上下文组合；值的 owner 与读写责任仍在外壳组件。 */
export function PublicShellProviders(props: PublicShellProvidersProps): JSX.Element {
  return (
    <SecondTickerContext value={props.secondTicker}>
      <SelectedWorldContext value={props.selectedWorld}>
        <PublicWorldQueryContext value={props.worldQuerySignal}>
          <PublicRefreshGenerationContext value={props.refreshGeneration}>
            <PublicBidRefreshContext value={props.bidRefresh}>
              <PublicPageRealtimeTargetContext value={props.pageRealtimeTarget}>
                <PublicDisplayedAuctionContext value={props.displayedAuction}>
                  <LotViewRequestContext value={props.lotViewRequest}>
                    <PublicSessionResourceContext value={props.session}>
                      <PublicCommandOwnerContext value={props.commandOwner}>
                        <PublicContentLoadingContext value={props.contentLoading}>
                          <PublicReadinessContext value={props.readiness}>
                            <CurrentAuctionContext value={props.currentAuction}>
                              <PageVisibilityContext value={props.pageVisibility}>
                                {props.children}
                              </PageVisibilityContext>
                            </CurrentAuctionContext>
                          </PublicReadinessContext>
                        </PublicContentLoadingContext>
                      </PublicCommandOwnerContext>
                    </PublicSessionResourceContext>
                  </LotViewRequestContext>
                </PublicDisplayedAuctionContext>
              </PublicPageRealtimeTargetContext>
            </PublicBidRefreshContext>
          </PublicRefreshGenerationContext>
        </PublicWorldQueryContext>
      </SelectedWorldContext>
    </SecondTickerContext>
  );
}
