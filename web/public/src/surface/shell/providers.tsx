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
import type { WorldSwitch } from "../board/world";
import {
  type LotLinkLocation,
  LotLinkLocationContext,
  type LotViewRequest,
  LotViewRequestContext,
} from "../catalog/detail/deep-link";
import { type CompetitionCommands, CompetitionCommandsContext } from "../commands/host";
import type {
  CurrentAuctionResource,
  PublicReadinessResource,
  PublicSessionResource,
} from "../index";
import { PublicCommandOwnerContext, PublicSessionResourceContext } from "../index";
import { PublicContentLoadingContext } from "../loading";
import {
  CurrentAuctionContext,
  PageVisibilityContext,
  PublicReadinessContext,
  SelectedWorldContext,
  WorldSwitchContext,
} from "./contexts";

export interface PublicShellProvidersProps {
  readonly secondTicker: SecondTicker;
  readonly worldSwitch: WorldSwitch;
  readonly selectedWorld: Cell<string>;
  readonly refreshGeneration: Cell<number>;
  readonly bidRefresh: PublicBidRefresh;
  readonly pageRealtimeTarget: Cell<readonly [PublicRealtimePageKey, PublicRealtimeTarget] | null>;
  readonly displayedAuction: Cell<CurrentAuctionItemResponse | null>;
  readonly lotViewRequest: Cell<LotViewRequest | null>;
  readonly lotLinkLocation: Cell<LotLinkLocation | null>;
  readonly session: PublicSessionResource;
  readonly commandOwner: Cell<string | null>;
  readonly competition: CompetitionCommands;
  readonly contentLoading: Cell<ReadonlySet<number>>;
  readonly readiness: PublicReadinessResource;
  readonly currentAuction: CurrentAuctionResource;
  readonly pageVisibility: PageVisibility;
  readonly children: JSX.Element;
}

/** 外壳根部的上下文组合；值的 owner 与读写责任仍在外壳组件。 */
export function PublicShellProviders(props: PublicShellProvidersProps): JSX.Element {
  return (
    <WorldSwitchContext value={props.worldSwitch}>
      <SecondTickerContext value={props.secondTicker}>
        <SelectedWorldContext value={props.selectedWorld}>
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
                                <LotLinkLocationContext value={props.lotLinkLocation}>
                                  <CompetitionCommandsContext value={props.competition}>
                                    {props.children}
                                  </CompetitionCommandsContext>
                                </LotLinkLocationContext>
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
        </SelectedWorldContext>
      </SecondTickerContext>
    </WorldSwitchContext>
  );
}
